//! The approval of a Realms join ("OTP override") and the one-time pass it buys.
//!
//! A join answer from Realms names the server the game is to connect to. Code in the game must
//! not be able to use that on its own, so the broker does not hand it over. It shows a prompt
//! **outside the game** — in the launcher's own output and as an event for the front end — with
//! the Realm, its address and, in `code` mode, a six-digit code the launcher made up. The user
//! types the code (or, in `totp` mode, the one their authenticator app shows) into the phone's
//! Realms app; only then does the next join request from the game get an answer, and in it an
//! address of the launcher's own: the **pass**. The game may connect through it exactly once,
//! within a short window, and the relay connects that one connection to exactly the address
//! Realms named.
//!
//! Bounds: a prompt lives 120 s and takes 3 attempts; one prompt at a time; at most 20 prompts
//! per session with 10 s between a refusal and the next prompt; in `totp` mode at most 10 wrong
//! codes per session, and a step that was used is never accepted again (see `totp`). While a
//! prompt waits, the game's join request is answered `503` with `Retry-After: 3` — the Realms
//! client (every family from 1.20.1 to 26.3) retries that up to 40 times, which is the 120 s.
//!
//! What this does **not** prove: that the person who typed the code meant this Realm. In `code`
//! mode the code is bound to the one prompt it was shown with (world and address); in `totp`
//! mode a code approves whatever join is waiting when it arrives — code in the game that reads
//! what you type into the phone could cancel your prompt, start a join to another of your
//! Realms and spend your code on that. And the prompt is only as good as your reading it: the
//! launcher shows where the game would connect; it cannot know whether you wanted to go there.

use std::{
    collections::HashMap,
    net::SocketAddrV4,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::{Approve, table::Address, totp};
use crate::{secret::Secret, util};

pub const PROMPT_TTL: Duration = Duration::from_secs(120);
pub const ATTEMPTS: u8 = 3;
/// What the game is told to wait between join requests while the user decides.
pub const RETRY_AFTER_SECS: u64 = 3;
/// How long the game has to use the pass once it has been given it.
pub const PASS_WINDOW: Duration = Duration::from_secs(120);
pub const MAX_PROMPTS: u32 = 20;
pub const COOLDOWN: Duration = Duration::from_secs(10);
pub const MAX_TOTP_FAILURES: u32 = 10;
/// Realm names remembered from the Realms list (for the prompt).
const MAX_NAMES: usize = 64;

/// How a prompt ended, for the phone (`last`) and the front end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Approved,
    Refused,
    Expired,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Approved => "approved",
            Outcome::Refused => "refused",
            Outcome::Expired => "expired",
        }
    }
}

/// What the user is shown, outside the game, when a join waits or a prompt ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub world: String,
    pub address: String,
    pub mode: Approve,
    /// The code to type, in `code` mode, while the prompt waits.
    pub code: Option<String>,
    pub expires_in: u64,
    pub attempts_left: u8,
    /// `None` while waiting.
    pub outcome: Option<Outcome>,
}

pub type Announcer = Arc<dyn Fn(&Announcement) + Send + Sync>;

/// What the broker does with a join request from the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinStep {
    /// Ask Realms (with the real credential) where the server is, then call [`Approvals::prompt`].
    Forward,
    /// A prompt for this Realm waits: answer 503 with this `Retry-After`.
    Wait(u64),
    /// Approved: the answer for the game (with the pass's address). The pass is armed.
    Deliver(Vec<u8>),
    /// Refused, with the reason for the game's screen.
    Refuse(String),
}

/// What became of a code the phone sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Approved,
    Wrong {
        attempts_left: u8,
    },
    Refused,
    NoPrompt,
    /// Too many wrong codes in this session: approvals are off until the next launch.
    Locked,
}

struct Prompt {
    world: u64,
    name: String,
    target: Address,
    code: Option<String>,
    expires: Instant,
    attempts_left: u8,
    reply: Vec<u8>,
    approved: bool,
}

struct Pass {
    target: Address,
    world: u64,
    until: Instant,
    used: bool,
}

/// The TOTP secret and the record of the last step used.
pub struct TotpState {
    pub secret: Secret,
    pub last_used: Option<u64>,
    /// Where the last used step is kept between launches (in the cache).
    pub record: Option<PathBuf>,
}

struct Inner {
    prompt: Option<Prompt>,
    pass: Option<Pass>,
    last: Option<Outcome>,
    prompts: u32,
    failures: u32,
    next_prompt: Option<Instant>,
    names: HashMap<u64, String>,
    totp: Option<TotpState>,
}

/// The approvals of one launch. Shared by the broker (join requests), the relay (the pass) and
/// the phone's Realms app (codes).
pub struct Approvals {
    mode: Approve,
    pass_address: SocketAddrV4,
    inner: Mutex<Inner>,
    announce: Announcer,
    /// Called whenever what the phone shows changes.
    changed: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Test support: how far the clock has been moved forward.
    clock: Mutex<Duration>,
}

fn random_code() -> String {
    loop {
        let Ok(bytes) = util::random_bytes::<4>() else { continue };
        let n = u32::from_le_bytes(bytes);
        // Rejection sampling: 4294000000 is the largest multiple of 10^6 below 2^32.
        if n < 4_294_000_000 {
            return format!("{:06}", n % 1_000_000);
        }
    }
}

impl Approvals {
    /// `mode` is `Code` or `Totp` (with `Off` the broker builds none). `pass_address` is the
    /// address inside the sandbox that the relay serves the pass on.
    pub fn new(mode: Approve, totp: Option<TotpState>, pass_address: SocketAddrV4, announce: Announcer) -> Arc<Self> {
        Arc::new(Approvals {
            mode,
            pass_address,
            inner: Mutex::new(Inner {
                prompt: None,
                pass: None,
                last: None,
                prompts: 0,
                failures: 0,
                next_prompt: None,
                names: HashMap::new(),
                totp,
            }),
            announce,
            changed: Mutex::new(None),
            clock: Mutex::new(Duration::ZERO),
        })
    }

    pub fn mode(&self) -> Approve {
        self.mode
    }

    pub fn pass_address(&self) -> SocketAddrV4 {
        self.pass_address
    }

    /// Whoever wants to hear about changes (the phone's app, to send its state).
    pub fn on_change(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        *self.changed.lock().expect("approvals") = Some(notify);
    }

    #[doc(hidden)]
    pub fn advance_clock(&self, by: Duration) {
        *self.clock.lock().expect("approvals clock") += by;
    }

    fn now(&self) -> Instant {
        Instant::now() + *self.clock.lock().expect("approvals clock")
    }

    fn unix_now(&self) -> u64 {
        let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        (wall + *self.clock.lock().expect("approvals clock")).as_secs()
    }

    fn notify(&self) {
        let notify = self.changed.lock().expect("approvals").clone();
        if let Some(notify) = notify {
            notify();
        }
    }

    fn announcement(&self, prompt: &Prompt, now: Instant, outcome: Option<Outcome>) -> Announcement {
        Announcement {
            world: prompt.name.clone(),
            address: prompt.target.to_string(),
            mode: self.mode,
            code: if outcome.is_none() { prompt.code.clone() } else { None },
            expires_in: prompt.expires.saturating_duration_since(now).as_secs(),
            attempts_left: prompt.attempts_left,
            outcome,
        }
    }

    /// End a waiting prompt whose time is up. Returns the announcement to make (outside the lock).
    fn expire(&self, inner: &mut Inner, now: Instant) -> Option<Announcement> {
        let expired = inner.prompt.as_ref().is_some_and(|p| !p.approved && now >= p.expires);
        if !expired {
            return None;
        }
        let prompt = inner.prompt.take().expect("checked");
        inner.last = Some(Outcome::Expired);
        inner.next_prompt = Some(now + COOLDOWN);
        Some(self.announcement(&prompt, now, Some(Outcome::Expired)))
    }

    fn finish(&self, announcements: Vec<Announcement>) {
        for a in &announcements {
            (self.announce)(a);
        }
        if !announcements.is_empty() {
            self.notify();
        }
    }

    /// Remember Realm names from a Realms list answer (`{"servers": [{"id", "name"}, …]}`).
    pub fn note_names(&self, list: &[u8]) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(list) else { return };
        let Some(servers) = value.get("servers").and_then(serde_json::Value::as_array) else { return };
        let mut inner = self.inner.lock().expect("approvals");
        for server in servers.iter().take(MAX_NAMES) {
            let (Some(id), Some(name)) = (
                server.get("id").and_then(serde_json::Value::as_u64),
                server.get("name").and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            if inner.names.len() < MAX_NAMES || inner.names.contains_key(&id) {
                inner.names.insert(id, util::sanitize_line(name.as_bytes(), 64));
            }
        }
    }

    /// The game asks to join Realm `world`.
    pub fn join_requested(&self, world: u64) -> JoinStep {
        let now = self.now();
        let mut announcements = Vec::new();
        let step = {
            let mut inner = self.inner.lock().expect("approvals");
            announcements.extend(self.expire(&mut inner, now));
            match &inner.prompt {
                Some(prompt) if prompt.world == world && prompt.approved => {
                    let prompt = inner.prompt.take().expect("matched");
                    inner.pass = Some(Pass { target: prompt.target, world, until: now + PASS_WINDOW, used: false });
                    JoinStep::Deliver(prompt.reply)
                }
                Some(prompt) if prompt.world == world => JoinStep::Wait(RETRY_AFTER_SECS),
                Some(prompt) => JoinStep::Refuse(format!(
                    "a join to another Realm ({}) is waiting for your approval; finish or cancel it in the phone's Realms app first",
                    prompt.name
                )),
                None if inner.totp_locked(self.mode) => JoinStep::Refuse(
                    "too many wrong codes in this session: Realms joins are refused until the next launch".into(),
                ),
                None if inner.prompts >= MAX_PROMPTS => JoinStep::Refuse(format!(
                    "{MAX_PROMPTS} Realms joins were asked for in this session; no more until the next launch"
                )),
                None if inner.next_prompt.is_some_and(|t| now < t) => JoinStep::Refuse(
                    "the last approval was refused or ran out a moment ago; try again in a few seconds".into(),
                ),
                None => JoinStep::Forward,
            }
        };
        self.finish(announcements);
        step
    }

    /// Realms answered where Realm `world` is: show the prompt. `reply` is the answer the game
    /// will be given once the user approves (with the pass's address in it).
    pub fn prompt(&self, world: u64, target: Address, reply: Vec<u8>) -> JoinStep {
        let now = self.now();
        let announcement = {
            let mut inner = self.inner.lock().expect("approvals");
            if let Some(prompt) = &inner.prompt {
                // Two requests for the same join raced; the first one shows the prompt.
                return if prompt.world == world {
                    JoinStep::Wait(RETRY_AFTER_SECS)
                } else {
                    JoinStep::Refuse("another join is waiting".into())
                };
            }
            let name = inner.names.get(&world).cloned().unwrap_or_else(|| format!("Realm {world}"));
            let prompt = Prompt {
                world,
                name,
                target,
                code: (self.mode == Approve::Code).then(random_code),
                expires: now + PROMPT_TTL,
                attempts_left: ATTEMPTS,
                reply,
                approved: false,
            };
            let announcement = self.announcement(&prompt, now, None);
            inner.prompts += 1;
            inner.prompt = Some(prompt);
            announcement
        };
        self.finish(vec![announcement]);
        JoinStep::Wait(RETRY_AFTER_SECS)
    }

    /// The phone sent a code.
    pub fn answer(&self, digits: &str) -> Answer {
        let now = self.now();
        let unix = self.unix_now();
        let mut announcements = Vec::new();
        let mut record = None;
        let answer = {
            let mut inner = self.inner.lock().expect("approvals");
            announcements.extend(self.expire(&mut inner, now));
            if inner.totp_locked(self.mode) {
                Answer::Locked
            } else if inner.prompt.as_ref().is_none_or(|p| p.approved) {
                Answer::NoPrompt
            } else {
                let right = match self.mode {
                    Approve::Code => {
                        let code = inner.prompt.as_ref().and_then(|p| p.code.clone()).unwrap_or_default();
                        digits.len() == 6 && util::ct_eq(code.as_bytes(), digits.as_bytes())
                    }
                    Approve::Totp => match inner.totp.as_mut() {
                        Some(state) => match totp::verify(state.secret.expose(), digits, unix, state.last_used) {
                            Some(step) => {
                                state.last_used = Some(step);
                                record = state.record.clone().map(|path| (path, step));
                                true
                            }
                            None => false,
                        },
                        None => false,
                    },
                    Approve::Off => false,
                };
                let prompt = inner.prompt.as_mut().expect("checked");
                if right {
                    prompt.approved = true;
                    let a = self.announcement(prompt, now, Some(Outcome::Approved));
                    inner.last = Some(Outcome::Approved);
                    announcements.push(a);
                    Answer::Approved
                } else {
                    prompt.attempts_left = prompt.attempts_left.saturating_sub(1);
                    let left = prompt.attempts_left;
                    inner.failures += 1;
                    if left == 0 {
                        let prompt = inner.prompt.take().expect("checked");
                        inner.last = Some(Outcome::Refused);
                        inner.next_prompt = Some(now + COOLDOWN);
                        announcements.push(self.announcement(&prompt, now, Some(Outcome::Refused)));
                        Answer::Refused
                    } else {
                        Answer::Wrong { attempts_left: left }
                    }
                }
            }
        };
        if let Some((path, step)) = record {
            // Best effort: the step is already spent in this session whatever happens here.
            let _ = totp::save_last_used(&path, step);
        }
        self.finish(announcements);
        if matches!(answer, Answer::Wrong { .. }) {
            self.notify();
        }
        answer
    }

    /// The phone cancelled the waiting prompt.
    pub fn cancel(&self) -> bool {
        let now = self.now();
        let announcement = {
            let mut inner = self.inner.lock().expect("approvals");
            match inner.prompt.take() {
                Some(prompt) => {
                    inner.last = Some(Outcome::Refused);
                    inner.next_prompt = Some(now + COOLDOWN);
                    self.announcement(&prompt, now, Some(Outcome::Refused))
                }
                None => return false,
            }
        };
        self.finish(vec![announcement]);
        true
    }

    /// A connection to the pass's address arrived: where it goes, if the pass is armed, unused
    /// and in its window. Spends the pass.
    pub fn take_pass(&self) -> Result<(Address, u64), &'static str> {
        let now = self.now();
        let mut inner = self.inner.lock().expect("approvals");
        let Some(pass) = inner.pass.as_mut() else { return Err("no Realms join was approved") };
        if pass.used {
            return Err("the approved Realms join was already used (one connection per approval)");
        }
        if now >= pass.until {
            inner.pass = None;
            return Err("the approved Realms join was not used in time");
        }
        pass.used = true;
        Ok((pass.target.clone(), pass.world))
    }

    /// The phone's view (contract: `{"mode", "prompt", "last"}`).
    pub fn state(&self) -> serde_json::Value {
        let now = self.now();
        let announcements;
        let state = {
            let mut inner = self.inner.lock().expect("approvals");
            announcements = self.expire(&mut inner, now);
            let prompt = inner.prompt.as_ref().filter(|p| !p.approved).map(|p| {
                serde_json::json!({
                    "world": p.name,
                    "address": p.target.to_string(),
                    "expires_in": p.expires.saturating_duration_since(now).as_secs(),
                    "attempts_left": p.attempts_left,
                })
            });
            serde_json::json!({
                "mode": self.mode.as_str(),
                "prompt": prompt,
                "last": inner.last.map(Outcome::as_str),
                "locked": inner.totp_locked(self.mode),
            })
        };
        if let Some(a) = announcements {
            (self.announce)(&a);
        }
        state
    }
}

impl Inner {
    fn totp_locked(&self, mode: Approve) -> bool {
        mode == Approve::Totp && self.failures >= MAX_TOTP_FAILURES
    }
}

#[cfg(test)]
mod tests;
