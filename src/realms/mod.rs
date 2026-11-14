//! Realms in filtered mode, through an approval the user gives outside the game.
//!
//! **Why Realms did not work before.** The game reaches Realms at `pc.realms.minecraft.net`
//! (and, from 1.21.8 on, at `*.realms.minecraft-services.net` when Realms asks it to). Realms
//! does not take the access token as `Authorization: Bearer` like Mojang's other services but
//! in a cookie (`sid=token:<access token>:<profile>`), and the credential broker neither
//! terminated those hosts nor knew that form: the game's cookie, holding the placeholder, went
//! to Realms as it was and was refused. And the answer to a join is a bare `IP:port` — no
//! allowlisted server, on a port nobody knew before the game started, while the confined
//! launcher may connect only to the ports fixed before it confined itself.
//!
//! **What this module adds** (`[realms] approve = "code" | "totp"`; `"off"`, the default,
//! leaves everything as it was):
//!
//! * the broker terminates the Realms hosts too and rebuilds the session cookie around the real
//!   token for the requests on a fixed table ([`table`]) — listing Realms, invitations,
//!   notifications, and the join; nothing an owner does to a Realm;
//! * a join is held until the user approves it ([`approval`]): the launcher shows a prompt in
//!   its own output and as an event for the front end — never through the game — and the user
//!   types the launcher's six-digit code, or their authenticator's TOTP code ([`totp`]), into
//!   the phone's Realms app;
//! * the approved answer gives the game the address of a **one-time pass**: an address inside
//!   the sandbox that the relay serves, good for one connection within a short window, which
//!   it connects to exactly the address Realms named — through a **dialer** started before the
//!   launcher confined itself ([`dial`]), because the confined launcher cannot connect to a port
//!   it did not know in advance. The dialer runs in a sandbox of its own and connects to public
//!   addresses only, at most a few times per session.
//!
//! Without the phone (a vanilla instance, or the companion mod switched off) there is nowhere
//! to type the code: Realms joins are then refused, as before.

pub mod approval;
pub mod dial;
pub mod table;
pub mod totp;

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
};

use serde::Deserialize;

use crate::phone::{Command, Outbox, PhoneApp, Service, VerbLimit};

/// `[realms] approve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Approve {
    /// Realms joins are refused (and Realms is not brokered at all): as before.
    Off,
    /// The launcher shows a six-digit code outside the game; the user types it into the phone.
    Code,
    /// The user types the code of their authenticator app (`bastion-mc realms totp-setup`).
    Totp,
}

impl Approve {
    pub fn as_str(self) -> &'static str {
        match self {
            Approve::Off => "off",
            Approve::Code => "code",
            Approve::Totp => "totp",
        }
    }
}

/// `[realms]` in config.toml.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RealmsConfig {
    /// How a Realms join is approved. Off by default: approving needs an account, the phone
    /// (Fabric with the companion mod) and a decision to let the game use the account for
    /// Realms at all.
    pub approve: Approve,
}

impl Default for RealmsConfig {
    fn default() -> Self {
        RealmsConfig { approve: Approve::Off }
    }
}

/// Where the pass is served inside the sandbox: the top of the range of synthetic addresses
/// (the resolver hands them out from the bottom and never gets this far) and the default game
/// port. A connection there is a connection to the approved Realm, or to nothing.
pub const PASS_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(127, 127, 255, 254), 25565);

/// The name of the token-store entry that holds the TOTP secret.
pub const TOTP_SLOT: crate::auth::store::Slot = crate::auth::store::Slot::RealmsTotp;

/// `bastion-mc realms totp-setup`: make a TOTP secret, keep it in the token store and return
/// the text that shows it — once, on the terminal (the command is not available with
/// `--events`, so it never reaches a front end). An existing secret is replaced only with
/// `replace` (the authenticator entry for it stops working).
pub fn totp_setup(store: &crate::auth::store::TokenStore, replace: bool) -> crate::Result<String> {
    if !replace && store.load_slot(TOTP_SLOT)?.is_some() {
        return Err(crate::Error::Usage(
            "a Realms TOTP secret is already stored. `bastion-mc realms totp-setup --replace` makes a new one (the \
             entry in your authenticator app then stops working)"
                .into(),
        ));
    }
    let secret = totp::new_secret()?;
    store.save_slot(TOTP_SLOT, &secret)?;
    let uri = totp::otpauth_uri(secret.expose(), "Realms");
    let base32 = totp::base32_encode(secret.expose());
    Ok(format!(
        "A new secret for approving Realms joins is stored in {}.\n\nAdd it to your authenticator app — scan this \
         URI as a QR code or paste it:\n\n  {uri}\n\nor type the secret by hand (time-based, SHA-1, 6 digits, 30 \
         seconds):\n\n  {base32}\n\nThis is the only time it is shown. Then set\n\n  [realms]\n  approve = \"totp\"\n\nin \
         config.toml. When a Realms join waits for approval, type the app's current code into the phone's Realms app.\n",
        store.describe()
    ))
}

/// The last TOTP step used, kept between launches.
pub fn totp_record(paths: &crate::paths::Paths) -> std::path::PathBuf {
    paths.cache.join("realms-totp-last-step")
}

/// For the robustness tests and fuzz targets: the Realms table, the cookie rebuild, the join
/// answer and the dialer's request reader on arbitrary bytes. Whatever the bytes are, a
/// credential goes only to a canonical path on the table, a rebuilt cookie carries the real
/// token and the session's profile and nothing of the game's but a user name and a version of
/// fixed shapes, and the answer handed to the game names the pass and nothing else.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    use hyper::Method;
    const PLACEHOLDER: &str = "bastion-placeholder-fuzz";
    const PROFILE: &str = "069a79f444e94726a5befca90e38aaf5";
    const TOKEN: &[u8] = b"FUZZ.REAL.TOKEN";
    let text = String::from_utf8_lossy(data);
    for host in table::API_HOSTS.iter().chain([&table::FRONTEND_V2, &"api.minecraftservices.com"]) {
        for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
            for query in [false, true] {
                match table::classify(host, &method, &text, query) {
                    table::Verdict::Cookie(_) | table::Verdict::MojangAuth(_) | table::Verdict::Join(_) => {
                        assert!(crate::egress::broker::path_is_canonical(&text), "credential for {text:?}");
                        assert!(!query, "credential for a target with a query");
                        assert!(table::is_realms_host(host));
                    }
                    table::Verdict::Anonymous => assert_eq!(method, Method::GET),
                    table::Verdict::Refuse { status, .. } => assert!((400..500).contains(&status)),
                }
            }
        }
    }
    if let Ok(cookie) = table::rebuild_cookie(data, PLACEHOLDER, TOKEN, PROFILE) {
        let cookie = String::from_utf8(cookie).expect("a rebuilt cookie is text");
        let rest =
            cookie.strip_prefix(&format!("sid=token:FUZZ.REAL.TOKEN:{PROFILE}")).expect("the real session first");
        assert!(!cookie.contains(PLACEHOLDER), "the placeholder never goes out");
        for part in rest.split(';').filter(|p| !p.is_empty()) {
            let (name, value) = part.split_once('=').expect("name=value");
            assert!(name == "user" || name == "version", "{cookie:?}");
            assert!(value.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_ +".contains(&b)), "{cookie:?}");
        }
        assert!(text.contains(&format!("token:{PLACEHOLDER}:")), "accepted without the placeholder: {text:?}");
    }
    if let Ok((address, object)) = table::parse_join_reply(data) {
        assert!(address.port != 0 && !address.host.is_empty());
        let answer: serde_json::Value =
            serde_json::from_slice(&table::rewrite_join_reply(object, PASS_ADDRESS)).expect("JSON");
        assert_eq!(answer["address"], PASS_ADDRESS.to_string());
        assert!(table::parse_address(&address.to_string()).is_some(), "an address it read prints as one it reads");
    }
    let _ = table::parse_address(&text);
    dial::fuzz_request(data);
    // Names from a Realms list are bounded and printable however the list looks.
    let approvals = approval::Approvals::new(Approve::Code, None, PASS_ADDRESS, Arc::new(|_| {}));
    approvals.note_names(data);
    if approvals.join_requested(1) == approval::JoinStep::Forward {
        approvals.prompt(1, table::Address { host: "203.0.113.9".into(), port: 1 }, Vec::new());
        let state = approvals.state();
        let world = state["prompt"]["world"].as_str().expect("a prompt");
        assert!(world.chars().count() <= 80 && !world.chars().any(char::is_control), "{world:?}");
    }
}

// ---------------------------------------------------------------- the phone's Realms app

const LIMITS: &[VerbLimit] =
    &[VerbLimit { verb: "code", per_minute: 10, burst: 3 }, VerbLimit { verb: "cancel", per_minute: 10, burst: 3 }];

/// The phone's `realms` app (contract: verbs `code -- <digits>`, `cancel`).
pub struct RealmsService {
    approvals: Option<Arc<approval::Approvals>>,
    mode: Approve,
    /// Why approvals are not available in this launch, when they are not.
    off_because: Option<String>,
}

impl RealmsService {
    pub fn new(approvals: Arc<approval::Approvals>) -> Self {
        let mode = approvals.mode();
        RealmsService { approvals: Some(approvals), mode, off_because: None }
    }

    /// The app without approvals (mode off, demo mode, no broker): it says so and does nothing.
    pub fn off(mode: Approve, why: impl Into<String>) -> Self {
        RealmsService { approvals: None, mode, off_because: Some(why.into()) }
    }
}

impl Service for RealmsService {
    fn name(&self) -> &'static str {
        "realms"
    }

    fn describe(&self) -> String {
        match (&self.approvals, &self.off_because) {
            (Some(a), _) => format!(
                "phone: realms app — joins need your approval ({}); the pass is served at {}",
                a.mode().as_str(),
                a.pass_address()
            ),
            (None, Some(why)) => format!("phone: realms app — Realms joins are refused: {why}"),
            (None, None) => "phone: realms app — Realms joins are refused".into(),
        }
    }

    fn confinement_needs(&self, _keep: &mut crate::confine::Confinement) {
        // Nothing: the broker's connections to Realms use port 443, which the relay already
        // has, and the pass's connection is made by the dialer, which was started before.
        // The TOTP record lives in the cache, which the launcher keeps anyway.
    }

    fn into_app(self: Box<Self>) -> Box<dyn PhoneApp> {
        Box::new(RealmsApp { approvals: self.approvals, mode: self.mode, off_because: self.off_because })
    }
}

pub struct RealmsApp {
    approvals: Option<Arc<approval::Approvals>>,
    mode: Approve,
    off_because: Option<String>,
}

impl RealmsApp {
    fn state(&self) -> serde_json::Value {
        match &self.approvals {
            Some(a) => a.state(),
            None => serde_json::json!({
                "mode": if self.off_because.is_some() && self.mode != Approve::Off { "off" } else { self.mode.as_str() },
                "prompt": null,
                "last": null,
                "error": self.off_because,
            }),
        }
    }
}

impl PhoneApp for RealmsApp {
    fn name(&self) -> &'static str {
        "realms"
    }

    fn limits(&self) -> &'static [VerbLimit] {
        LIMITS
    }

    fn hello(&mut self, out: &Outbox) {
        if let Some(a) = &self.approvals {
            // From now on every change (a new prompt, its end) is sent as it happens.
            let (out, weak) = (out.clone(), Arc::downgrade(a));
            a.on_change(Arc::new(move || {
                if let Some(a) = weak.upgrade() {
                    out.state("realms", &a.state());
                }
            }));
        }
        out.state("realms", &self.state());
    }

    fn command(&mut self, command: Command, out: &Outbox) {
        let Some(approvals) = &self.approvals else {
            out.notice("realms", "Realms joins are refused in this launch");
            out.state("realms", &self.state());
            return;
        };
        match (command.verb.as_str(), command.args.is_empty(), command.text.as_deref()) {
            ("code", true, Some(text)) => {
                let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                let answer = if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit()) {
                    approvals.answer(&digits)
                } else {
                    // Malformed input is an attempt like any other (a guess is a guess).
                    approvals.answer("x")
                };
                let notice = match answer {
                    approval::Answer::Approved => "Approved — joining the Realm".to_string(),
                    approval::Answer::Wrong { attempts_left } => {
                        format!("Wrong code ({attempts_left} attempt(s) left)")
                    }
                    approval::Answer::Refused => "Wrong code — the join was refused".to_string(),
                    approval::Answer::NoPrompt => "No Realms join is waiting for a code".to_string(),
                    approval::Answer::Locked => {
                        "Too many wrong codes: Realms joins are off until the next launch".to_string()
                    }
                };
                out.notice("realms", &notice);
                out.state("realms", &approvals.state());
            }
            ("cancel", true, None) => {
                if approvals.cancel() {
                    out.notice("realms", "Realms join cancelled");
                }
                out.state("realms", &approvals.state());
            }
            _ => {
                out.notice("realms", "realms: use `code -- <six digits>` or `cancel`");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone::Outbound;

    fn drain(rx: &mut tokio::sync::mpsc::Receiver<Outbound>) -> Vec<Outbound> {
        let mut out = Vec::new();
        while let Ok(m) = rx.try_recv() {
            out.push(m);
        }
        out
    }

    fn states(messages: &[Outbound]) -> Vec<serde_json::Value> {
        messages
            .iter()
            .filter_map(|m| match m {
                Outbound::State { app: "realms", json } => Some(serde_json::from_str(json).unwrap()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_phone_app_answers_prompts_and_never_sees_the_code() {
        let shown: Arc<std::sync::Mutex<Vec<approval::Announcement>>> = Arc::default();
        let sink = shown.clone();
        let approvals = approval::Approvals::new(
            Approve::Code,
            None,
            PASS_ADDRESS,
            Arc::new(move |a: &approval::Announcement| sink.lock().unwrap().push(a.clone())),
        );
        let mut app = Box::new(RealmsService::new(approvals.clone())).into_app();
        assert_eq!(app.name(), "realms");
        assert!(app.limits().iter().any(|l| l.verb == "code") && app.limits().iter().any(|l| l.verb == "cancel"));
        let (out, mut rx) = Outbox::new(64);
        app.hello(&out);
        let first = states(&drain(&mut rx));
        assert_eq!(first[0]["mode"], "code");
        assert!(first[0]["prompt"].is_null() && first[0]["last"].is_null());

        // A join arrives: the phone is told at once (on_change), without the code.
        assert_eq!(approvals.join_requested(42), approval::JoinStep::Forward);
        approvals.prompt(42, table::Address { host: "203.0.113.9".into(), port: 25565 }, b"{}".to_vec());
        let code = shown.lock().unwrap()[0].code.clone().unwrap();
        let pushed = drain(&mut rx);
        let state = &states(&pushed)[0];
        assert_eq!(state["prompt"]["world"], "Realm 42");
        assert_eq!(state["prompt"]["address"], "203.0.113.9:25565");
        assert!(!format!("{pushed:?}").contains(&code));

        let parse = |line: &str| Command::parse(line.as_bytes()).unwrap();
        app.command(parse("realms code -- 12 34"), &out); // four digits: an attempt, and wrong
        app.command(parse(&format!("realms code -- {} {}", &code[..3], &code[3..])), &out);
        let messages = drain(&mut rx);
        let notices: Vec<&str> = messages
            .iter()
            .filter_map(|m| if let Outbound::Notice { text, .. } = m { Some(text.as_str()) } else { None })
            .collect();
        assert_eq!(notices, ["Wrong code (2 attempt(s) left)", "Approved — joining the Realm"]);
        assert_eq!(states(&messages).last().unwrap()["last"], "approved");
        // Arguments where none belong, or no text: refused by the app.
        app.command(parse("realms code 123456"), &out);
        app.command(parse("realms cancel now"), &out);
        assert_eq!(drain(&mut rx).len(), 2, "two usage notices");
    }

    #[test]
    fn without_approvals_the_app_says_why() {
        let mut app = Box::new(RealmsService::off(Approve::Code, "demo mode: no account")).into_app();
        let (out, mut rx) = Outbox::new(8);
        app.hello(&out);
        let state = &states(&drain(&mut rx))[0];
        assert_eq!((state["mode"].as_str(), state["error"].as_str()), (Some("off"), Some("demo mode: no account")));
        app.command(Command::parse(b"realms code -- 123456").unwrap(), &out);
        assert!(drain(&mut rx).iter().any(|m| matches!(m, Outbound::Notice { .. })));
        let off = Box::new(RealmsService::off(Approve::Off, "[realms] approve = \"off\""));
        assert!(off.describe().contains("refused"));
    }

    #[test]
    fn totp_setup_needs_a_store_that_keeps_secrets() {
        let err = totp_setup(&crate::auth::store::TokenStore::None, false).unwrap_err().to_string();
        assert!(err.contains("keeps no secrets"), "{err}");
    }

    #[test]
    fn the_configuration_names_the_three_modes_and_nothing_else() {
        #[derive(Deserialize)]
        struct Wrapper {
            realms: RealmsConfig,
        }
        let parse = |text: &str| toml::from_str::<Wrapper>(text).map(|w| w.realms);
        assert_eq!(parse("[realms]").unwrap().approve, Approve::Off);
        assert_eq!(parse("[realms]\napprove = \"code\"").unwrap().approve, Approve::Code);
        assert_eq!(parse("[realms]\napprove = \"totp\"").unwrap().approve, Approve::Totp);
        assert!(parse("[realms]\napprove = \"yes\"").is_err());
        assert!(parse("[realms]\napprove = \"code\"\nport = 25565").is_err(), "unknown keys are errors");
        assert!(crate::egress::FakeIps::contains(*PASS_ADDRESS.ip()), "the pass lies in the synthetic range");
    }
}
