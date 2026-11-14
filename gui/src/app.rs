//! Application state and the logic that changes it.
//!
//! This is the whole "controller": [`App::update`] is the only place state changes, and it
//! never blocks. Anything slow is a [`Request`] handed to the backend worker; its
//! [`Update`]s come back as [`Message::Backend`] and are folded into the state here. The
//! view (`crate::view`) only reads — including the clock: what moves on screen is a
//! function of [`App::now`], which only a [`Message::Tick`] advances.
//!
//! One launcher command runs at a time ([`App::job`]). That is what makes the launch
//! control's behaviour trivial to get right: it is enabled exactly when there is no job.
//!
//! The one exception is the emergency stop ([`Emergency`]): a scram runs beside whatever
//! job there is, because stopping a running game is part of what it does. Three rules
//! keep it honest:
//!
//! * it is only ever started as the answer to a question ([`Message::AskScram`], then
//!   [`Message::ConfirmScram`]);
//! * a sign-in in progress is ended *first* — one that completed afterwards would store a
//!   fresh token on the machine that was just cleared — and no sign-in or launch can be
//!   started until the scram is over;
//! * its result is "complete" only if the launcher exited with status 0 and named nothing
//!   it could not remove. Everything else is shown as incomplete, with the launcher's own
//!   lines.
//!
//! And one more follows from the second: between the confirmation and the moment the
//! launcher doing the scram exists, closing the window would drop the scram that was just
//! confirmed. A window asked to close in that time stays until the launcher is running
//! (from then on it needs no window), or says that it could not be started.

use std::{
    fmt,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

use bastion_mc_proto::{Account, Event, Instance, PROTOCOL, Phase, Protection, Source, Unit, Version, runbook};
use iced::{
    Subscription, Task,
    futures::{StreamExt, stream},
    window,
};

use crate::{
    backend::{self, Handle, Outcome, Request, ScramEnd, ScramUpdate, Update, clean, clean_to, fit},
    console::{Console, Level},
};

/// Accounts and instances shown at most. The launcher's lists are bounded by the length of
/// an event line only; a window with thousands of cards would be of no use to anyone.
pub const MAX_ACCOUNTS: usize = 64;
pub const MAX_INSTANCES: usize = 256;
/// Versions offered in the "new instance" form. The launcher is asked for 400.
pub const MAX_VERSIONS: usize = 1000;
/// The longest name a version may have to be offered. Mojang's are short ("26.3",
/// "26.4-snapshot-8").
pub const MAX_VERSION_CHARS: usize = 48;
/// Lines of a scram report kept for the emergency panel (all of them go to the console).
pub const MAX_SCRAM_LINES: usize = 400;

/// What is kept of one line for the console. The launcher caps a line of game output at
/// 4096 characters and every other text at 6000, so nothing it sends is cut by this.
pub const CONSOLE_CHARS: usize = 6000;
/// What is kept of a line the launcher says about what it is doing: enough for the two or
/// three lines there are for it on screen. The console has the line in full.
pub const STATUS_CHARS: usize = 160;
/// What is kept of a device code. Microsoft's are nine characters; the launcher accepts up
/// to 32.
pub const CODE_CHARS: usize = 32;
/// What is kept of the address a device code is entered at (it is shown, as text).
pub const CODE_ADDRESS_CHARS: usize = 200;
/// What is kept of the address of a browser sign-in (it is never shown, only opened or
/// copied). Longer than this it could not be opened anyway: see [`is_sign_in_link`].
pub const SIGN_IN_ADDRESS_CHARS: usize = 4096;
/// The longest a sign-in prompt is counted down from. The launcher waits half an hour at
/// most; a larger number is not a time anyone waits, and it must not overflow the clock
/// it is added to.
pub const MAX_PROMPT_SECS: u64 = 3600;

/// How long a sign-in gets to end after a scram was confirmed, before it is killed.
const SIGN_IN_ABORT_GRACE: Duration = Duration::from_secs(3);
/// After this long the scram starts even if the sign-in's end was never reported.
const SCRAM_START_DEADLINE: Duration = Duration::from_secs(6);

/// The clock's steps while something on screen depends on it. Nothing ticks otherwise.
pub const SWEEP_INTERVAL: Duration = Duration::from_millis(40);
pub const PULSE_INTERVAL: Duration = Duration::from_millis(200);
pub const CLOCK_INTERVAL: Duration = Duration::from_secs(1);

/// The sections of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Launch,
    Accounts,
    Instances,
    Console,
    Security,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::Launch, Page::Accounts, Page::Instances, Page::Console, Page::Security];

    pub const fn title(self) -> &'static str {
        match self {
            Page::Launch => "Launch",
            Page::Accounts => "Accounts",
            Page::Instances => "Instances",
            Page::Console => "Console",
            Page::Security => "Security",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Refresh,
    SignIn,
    SignOut,
    Versions,
    CreateInstance,
    Launch,
}

/// The launcher command currently running.
#[derive(Debug)]
pub struct Job {
    id: u64,
    pub kind: JobKind,
    /// What the command is about: the account of a sign-in or sign-out, the instance of a
    /// launch. Empty when it is about nothing in particular.
    pub subject: String,
    handle: Option<Handle>,
    /// Where a launch is; `None` until the launcher says so.
    pub phase: Option<Phase>,
    /// Every phase the launcher has announced, one bit each.
    seen: u8,
    /// A stop was requested; the command is winding down.
    pub stopping: bool,
    /// How long the launcher gets to comply, where that is not the worker's default.
    stop_grace: Option<Duration>,
    /// The launcher explained its own failure, so its exit status needs no second notice.
    reported_error: bool,
    pub started: Instant,
    /// When the game began to run.
    pub running_since: Option<Instant>,
}

impl Job {
    pub fn saw(&self, phase: Phase) -> bool {
        self.seen & (1 << phase as u8) != 0
    }
}

/// The stages of a launch, in the order the launcher goes through them.
pub const SEQUENCE: [Phase; 5] =
    [Phase::Preparing, Phase::Verifying, Phase::Authenticating, Phase::Starting, Phase::Running];

/// Where one stage of the launch in progress stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Pending,
    Active,
    Done,
    /// The launcher went past it without announcing it (demo mode has no sign-in).
    Skipped,
}

/// What the user has to do to finish signing in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginPrompt {
    DeviceCode { user_code: String, verification_uri: String, expires_at: Instant },
    Browser { url: String, expires_at: Instant },
}

impl LoginPrompt {
    pub fn expires_at(&self) -> Instant {
        match self {
            LoginPrompt::DeviceCode { expires_at, .. } | LoginPrompt::Browser { expires_at, .. } => *expires_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub label: String,
    pub done: u64,
    pub total: u64,
    pub unit: Unit,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 1.0 } else { (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32 }
    }

    pub fn percent(&self) -> u32 {
        (self.fraction() * 100.0).floor() as u32
    }

    /// How much of how much, as a read-out: "156.0 / 400.0 MiB", "2114 / 5149 files". It
    /// stands on one line, so it is never longer than [`Progress::AMOUNT_CHARS`] whatever
    /// the numbers: bytes go up a unit before they take a fifth digit, and a count of a
    /// million or more is given by its magnitude.
    pub fn amount(&self) -> String {
        match self.unit {
            Unit::Bytes => {
                const UNITS: [&str; 5] = ["MiB", "GiB", "TiB", "PiB", "EiB"];
                let most = self.done.max(self.total) as f64;
                let (mut unit, mut scale) = (0, 1048576.0);
                while most / scale >= 9999.95 && unit + 1 < UNITS.len() {
                    unit += 1;
                    scale *= 1024.0;
                }
                format!("{:.1} / {:.1} {}", self.done as f64 / scale, self.total as f64 / scale, UNITS[unit])
            }
            Unit::Files => {
                let count = |n: u64| if n < 1_000_000 { n.to_string() } else { format!("{:.1e}", n as f64) };
                format!("{} / {} files", count(self.done), count(self.total))
            }
        }
    }

    /// The longest [`Progress::amount`] gets: "999999 / 999999 files".
    pub const AMOUNT_CHARS: usize = 21;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub severity: Severity,
    pub text: String,
}

/// An instance, as an entry of a list or a drop-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceChoice(pub Instance);

impl InstanceChoice {
    pub fn launchable(&self) -> bool {
        self.0.error.is_none()
    }

    /// The instance on one line of at most `chars` characters, for a drop-down or a
    /// sentence: as much of its name as fits, and a mark on one that cannot be launched.
    /// Everything else about it is shown next to the drop-down, as text that can wrap.
    pub fn label(&self, chars: usize) -> String {
        const BROKEN: &str = " (broken)";
        if self.launchable() {
            fit(&self.0.id, chars)
        } else {
            format!("{}{BROKEN}", fit(&self.0.id, chars.saturating_sub(BROKEN.len())))
        }
    }
}

/// An entry of the version drop-down in the "new instance" form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionChoice(pub Version);

impl fmt::Display for VersionChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.released.is_empty() {
            f.write_str(&self.0.id)
        } else {
            write!(f, "{}  ({})", self.0.id, self.0.released)
        }
    }
}

impl VersionChoice {
    /// The version on one line of at most `chars` characters: its name, and the day it
    /// was released where there is room for that as well.
    pub fn label(&self, chars: usize) -> String {
        let whole = self.to_string();
        if whole.chars().count() <= chars { whole } else { fit(&self.0.id, chars) }
    }
}

#[derive(Debug, Default)]
pub struct NewInstance {
    pub open: bool,
    pub id: String,
    pub versions: Vec<VersionChoice>,
    pub version: Option<VersionChoice>,
    pub snapshots: bool,
}

/// One local account. The launcher's summary of it is kept as it was last reported, so that
/// the accounts page can show every account without asking again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountEntry {
    pub label: String,
    /// `None` for a label the launcher has not described (yet): one that was just added
    /// here, or any label after a scram, until the launcher has been asked again. Nothing
    /// is assumed about such an account.
    pub summary: Option<Account>,
}

/// Where an emergency stop is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScramState {
    #[default]
    Idle,
    /// Confirmed; the sign-in that was in progress is being ended first.
    StoppingSignIn,
    /// The launcher is removing things.
    Running,
}

/// How an emergency stop ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScramOutcome {
    /// The launcher exited with status 0 and named nothing it could not remove.
    Complete,
    /// The launcher ran, and it cannot be said that everything is gone. Says why.
    Incomplete(String),
    /// The launcher could not be started: this attempt removed nothing.
    NotRun(String),
}

/// One line of what a scram reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScramLine {
    pub text: String,
    /// The launcher said it could not remove something (or this is its error message).
    pub failed: bool,
}

/// The emergency stop and what it reported.
#[derive(Debug, Default)]
pub struct Emergency {
    /// "Scram" was pressed once and waits for the confirmation.
    pub confirming: bool,
    pub state: ScramState,
    /// When the current (or the last) scram was confirmed.
    pub since: Option<Instant>,
    /// Identifies the current run; anything that refers to an earlier one is stale.
    run: u64,
    /// The launcher doing the current scram is running: it no longer needs this window.
    started: bool,
    /// What the launcher has reported so far, in order.
    pub lines: Vec<ScramLine>,
    /// Lines that did not fit here; the console has them.
    pub omitted: usize,
    /// How many things the launcher said it could not remove, whether or not their lines
    /// fitted.
    pub failures: usize,
    /// `None` until a scram has ended.
    pub outcome: Option<ScramOutcome>,
}

impl Emergency {
    /// A scram was confirmed and is not over. Nothing that stores a token may start.
    pub fn active(&self) -> bool {
        self.state != ScramState::Idle
    }

    /// A scram was confirmed, and the launcher that is to do it is not running yet: were
    /// this program to end now, there would be no scram.
    pub fn awaiting_start(&self) -> bool {
        self.active() && !self.started
    }

    /// The lines that name what could not be removed.
    pub fn failed_lines(&self) -> impl Iterator<Item = &ScramLine> {
        self.lines.iter().filter(|line| line.failed)
    }

    fn push(&mut self, text: String, failed: bool) {
        if failed {
            self.failures += 1;
        }
        // A failure line is never the one that is left out.
        if self.lines.len() < MAX_SCRAM_LINES || failed && self.failures <= MAX_SCRAM_LINES {
            self.lines.push(ScramLine { text, failed });
        } else {
            self.omitted += 1;
        }
    }
}

/// The launcher's contract for an incomplete scram: exit status 1, and one line starting
/// like this for each thing that is still there.
fn is_failure_line(line: &str) -> bool {
    line.trim_start().starts_with("COULD NOT")
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Something from the backend worker, tagged with the job it belongs to.
    Backend(u64, Update),
    /// Show another section of the window.
    Navigate(Page),
    Refresh,
    SignIn {
        account: String,
        device_code: bool,
    },
    SignOut {
        account: String,
    },
    /// Use another local account for launching (an existing label, or a new one).
    AccountSelected(String),
    NewAccountName(String),
    AddAccount,
    /// Make this account the launcher's default.
    MakeDefaultAccount(String),
    /// Stop the game, or abandon a sign-in.
    Stop,
    InstanceSelected(InstanceChoice),
    DemoToggled(bool),
    Play,
    ClearConsole,
    CopyConsole,
    Copy(String),
    OpenLink(String),
    LinkOpened(Result<(), String>),
    /// Open one of the runbook's links (only those; see `runbook::LINKS`).
    OpenRunbookLink(String),
    /// First press of "Scram": ask. Second (`ConfirmScram`): do it.
    AskScram,
    CancelScram,
    ConfirmScram,
    /// Something from the launcher that is running the scram, tagged with the run.
    Scram(u64, ScramUpdate),
    /// The sign-in that had to end before the scram has had its time.
    ScramStartDeadline(u64),
    DismissNotice,
    ToggleNewInstance,
    NewInstanceId(String),
    VersionSelected(VersionChoice),
    SnapshotsToggled(bool),
    CreateInstance,
    Tick(Instant),
    CloseRequested(window::Id),
}

#[derive(Debug)]
pub struct App {
    launcher: PathBuf,
    pub page: Page,
    /// The account used for launching.
    pub account_name: String,
    /// What the launcher said about that account, since it was last chosen. `None` until
    /// then: the launch control goes by this, never by an older summary.
    pub account: Option<Account>,
    /// Every local account the launcher knows, plus the current one; sorted by label.
    pub accounts: Vec<AccountEntry>,
    /// Accounts the launcher listed beyond [`MAX_ACCOUNTS`].
    pub accounts_hidden: usize,
    /// The launcher's default account, once it has said.
    pub default_account: Option<String>,
    /// Text of the "new account" field.
    pub new_account: String,
    pub login: Option<LoginPrompt>,
    pub instances: Vec<InstanceChoice>,
    /// Instances the launcher listed beyond [`MAX_INSTANCES`].
    pub instances_hidden: usize,
    pub selected: Option<InstanceChoice>,
    pub demo: bool,
    pub job: Option<Job>,
    next_job: u64,
    pub progress: Option<Progress>,
    /// One line about the launch in progress, or how the last one ended (shown by the
    /// launch control).
    pub launch_status: String,
    /// One line about everything else: sign-in, instance management (shown at the bottom).
    pub status: String,
    pub notice: Option<Notice>,
    pub console: Console,
    pub new_instance: NewInstance,
    /// What the launch in progress (or the last one) runs with.
    pub protection: Vec<Protection>,
    pub emergency: Emergency,
    /// The launcher's version, once it has said hello.
    pub launcher_version: Option<String>,
    /// The time on screen. Only [`Message::Tick`] and the events that start a clock move it.
    pub now: Instant,
    /// The window was asked to close while a game was running: stop it, then exit.
    closing: bool,
    /// Instance to select as soon as it shows up in the list (it was just created).
    select_when_listed: Option<String>,
    /// The account state on screen is known to be out of date: ask the launcher as soon as
    /// it is free.
    refresh_wanted: bool,
}

/// Same rule the launcher enforces for instance and account names; checked here only to
/// give immediate feedback. The launcher has the final word.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && !id.contains("..")
}

/// May this link be handed to the desktop's URL opener? Only HTTPS links to Microsoft's
/// sign-in hosts, made of printable ASCII, with a bare host name (no port, no user part).
pub fn is_sign_in_link(url: &str) -> bool {
    const HOSTS: [&str; 4] = ["login.microsoftonline.com", "www.microsoft.com", "microsoft.com", "login.live.com"];
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    url.len() <= 4096 && url.bytes().all(|b| b.is_ascii_graphic()) && HOSTS.contains(&authority)
}

/// The desktop's URL opener, by absolute path: like the launcher, `$PATH` is not consulted.
const URL_OPENER: &str = "/usr/bin/xdg-open";

async fn open_link(url: String) -> Result<(), String> {
    let status = tokio::process::Command::new(URL_OPENER)
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("xdg-open: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("xdg-open ended with {status}")) }
}

fn phase_text(phase: Phase) -> &'static str {
    match phase {
        Phase::Preparing => "Checking game files…",
        Phase::Verifying => "Verifying every file before it is mounted…",
        Phase::Authenticating => "Signing in…",
        Phase::Starting => "Starting the sandbox…",
        Phase::Running => "The game is running.",
        Phase::Stopping => "Stopping the game…",
    }
}

/// An account summary as it is shown: the label has been checked by the caller; the free
/// text is made printable and bounded here.
fn presentable(account: Account) -> Account {
    Account {
        player: account.player.map(|player| clean_to(&player, 48)).filter(|player| !player.is_empty()),
        store: clean_to(&account.store, 240),
        ..account
    }
}

/// An instance as it is shown and used. One whose name could not be a launcher argument is
/// not kept at all (the name becomes one when the instance is launched); the free text is
/// made printable and bounded.
fn presentable_instance(instance: Instance) -> Option<Instance> {
    is_valid_id(&instance.id).then(|| Instance {
        minecraft: instance.minecraft.map(|version| clean_to(&version, 48)).filter(|version| !version.is_empty()),
        kind: clean_to(&instance.kind, 24),
        detail: instance.detail.map(|detail| clean_to(&detail, 240)).filter(|detail| !detail.is_empty()),
        // Kept even when empty: it is what says that the instance cannot be launched.
        error: instance.error.map(|error| clean_to(&error, 400)),
        ..instance
    })
}

/// A version as it is offered. Its name becomes a launcher argument when an instance is
/// created, so a name that cleaning would change — too long, or with something in it that
/// cannot be shown — is not offered under another name: it is not offered at all. Nor is
/// one that would read as an option.
fn presentable_version(version: Version) -> Option<Version> {
    let id = version.id;
    let usable = clean_to(&id, MAX_VERSION_CHARS) == id && !id.trim().is_empty() && !id.starts_with('-');
    usable.then(|| Version { id, kind: clean_to(&version.kind, 24), released: clean_to(&version.released, 16) })
}

impl App {
    pub fn new(launcher: PathBuf, account_name: String) -> Self {
        let mut app = App {
            launcher,
            page: Page::Launch,
            account_name,
            account: None,
            accounts: Vec::new(),
            accounts_hidden: 0,
            default_account: None,
            new_account: String::new(),
            login: None,
            instances: Vec::new(),
            instances_hidden: 0,
            selected: None,
            demo: false,
            job: None,
            next_job: 1,
            progress: None,
            launch_status: String::new(),
            status: String::new(),
            notice: None,
            console: Console::default(),
            new_instance: NewInstance::default(),
            protection: Vec::new(),
            emergency: Emergency::default(),
            launcher_version: None,
            now: Instant::now(),
            closing: false,
            select_when_listed: None,
            refresh_wanted: false,
        };
        app.remember_current_account();
        app
    }

    /// The state the window opens with, and the first thing to do: ask the launcher who is
    /// signed in and which instances exist.
    pub fn boot(launcher: PathBuf, account_name: String) -> (Self, Task<Message>) {
        let mut app = App::new(launcher, account_name);
        let task = app.start(JobKind::Refresh, String::new(), app.status_request());
        (app, task)
    }

    fn status_request(&self) -> Request {
        Request::Status { account: self.account_name.clone() }
    }

    pub fn can_add_account(&self) -> bool {
        !self.busy() && is_valid_id(&self.new_account) && self.new_account != self.account_name
    }

    /// Use another account for launching. Nothing is signed in or out by this; each label
    /// has its own stored token, and the launcher is asked afresh about the one now chosen.
    fn switch_account(&mut self, name: String) -> Task<Message> {
        if self.busy() || !is_valid_id(&name) || name == self.account_name {
            return Task::none();
        }
        self.account_name = name;
        self.account = None;
        self.login = None;
        self.notice = None;
        self.remember_current_account();
        self.start(JobKind::Refresh, String::new(), self.status_request())
    }

    /// The current label has an entry even if the launcher does not list it (a label is
    /// only known to the launcher once something was stored under it).
    fn remember_current_account(&mut self) {
        if !self.account_name.is_empty() && !self.accounts.iter().any(|entry| entry.label == self.account_name) {
            self.accounts.push(AccountEntry { label: self.account_name.clone(), summary: None });
            self.accounts.sort_by(|a, b| a.label.cmp(&b.label));
        }
    }

    /// Take the launcher's word about one account into the list.
    fn record_account(&mut self, account: &Account) {
        if let Some(entry) = self.accounts.iter_mut().find(|entry| entry.label == account.account) {
            entry.summary = Some(account.clone());
        } else if self.accounts.len() < MAX_ACCOUNTS {
            self.accounts.push(AccountEntry { label: account.account.clone(), summary: Some(account.clone()) });
            self.accounts.sort_by(|a, b| a.label.cmp(&b.label));
        }
    }

    fn set_accounts(&mut self, default: String, accounts: Vec<Account>) {
        let mut entries: Vec<AccountEntry> = accounts
            .into_iter()
            .filter(|account| is_valid_id(&account.account))
            .map(|account| AccountEntry { label: account.account.clone(), summary: Some(presentable(account)) })
            .collect();
        entries.sort_by(|a, b| a.label.cmp(&b.label));
        entries.dedup_by(|later, earlier| later.label == earlier.label);
        self.accounts_hidden = entries.len().saturating_sub(MAX_ACCOUNTS);
        if self.accounts_hidden > 0 {
            // The account in use is never among the ones left out.
            let current = entries.iter().position(|entry| entry.label == self.account_name);
            let keep = current.filter(|at| *at >= MAX_ACCOUNTS).map(|at| entries.swap_remove(at));
            entries.truncate(MAX_ACCOUNTS - usize::from(keep.is_some()));
            entries.extend(keep);
        }
        self.accounts = entries;
        self.default_account = Some(default).filter(|name| is_valid_id(name));
        // What the list says about the account in use is the launcher's word on it as much
        // as a report about that account alone: the rail and its card never disagree.
        let current = self.accounts.iter().find(|entry| entry.label == self.account_name);
        if let Some(summary) = current.and_then(|entry| entry.summary.clone()) {
            self.account = Some(summary);
        }
        self.remember_current_account();
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn job_kind(&self) -> Option<JobKind> {
        self.job.as_ref().map(|job| job.kind)
    }

    /// The launch in progress, from the first press to the launcher's exit.
    pub fn launch(&self) -> Option<&Job> {
        self.job.as_ref().filter(|job| job.kind == JobKind::Launch)
    }

    pub fn signed_in(&self) -> bool {
        self.account.as_ref().is_some_and(|a| a.signed_in)
    }

    /// Why the launch control cannot be pressed right now, if it cannot.
    pub fn play_blocker(&self) -> Option<&'static str> {
        if self.emergency.active() {
            return Some("A scram is in progress.");
        }
        match &self.selected {
            None if self.instances.is_empty() => return Some("Create an instance first."),
            None => return Some("Choose an instance."),
            Some(choice) if !choice.launchable() => return Some("This instance cannot be launched."),
            Some(_) => {}
        }
        if !self.demo && !self.signed_in() {
            return Some("Sign in, or switch on demo mode.");
        }
        None
    }

    pub fn can_play(&self) -> bool {
        !self.busy() && self.play_blocker().is_none()
    }

    /// May a sign-in be started for the account `entry`? Needs a launcher with nothing to
    /// do, no scram under way, an OAuth client, and an account that is not signed in.
    pub fn can_sign_in(&self, entry: &AccountEntry) -> bool {
        !self.busy()
            && !self.emergency.active()
            && entry.summary.as_ref().is_some_and(|summary| summary.client_id_configured && !summary.signed_in)
    }

    pub fn can_sign_out(&self, entry: &AccountEntry) -> bool {
        !self.busy() && entry.summary.as_ref().is_some_and(|summary| summary.signed_in)
    }

    pub fn can_create_instance(&self) -> bool {
        !self.busy()
            && self.new_instance.version.is_some()
            && is_valid_id(&self.new_instance.id)
            && !self.instances.iter().any(|i| i.0.id == self.new_instance.id)
    }

    /// Where each stage of the launch in progress stands. All pending when nothing is
    /// being launched.
    pub fn sequence(&self) -> [Stage; 5] {
        let Some(job) = self.launch() else { return [Stage::Pending; 5] };
        let reached = |phase: Phase| job.saw(phase);
        let mut stages = [Stage::Pending; 5];
        for (at, phase) in SEQUENCE.into_iter().enumerate() {
            let later_reached = SEQUENCE[at + 1..].iter().any(|later| reached(*later)) || reached(Phase::Stopping);
            stages[at] = if job.phase == Some(phase) {
                Stage::Active
            } else if reached(phase) {
                Stage::Done
            } else if later_reached {
                Stage::Skipped
            } else {
                Stage::Pending
            };
        }
        stages
    }

    /// Seconds since the launch in progress was started; what moves on screen is a
    /// function of this.
    pub fn launch_clock(&self) -> f32 {
        self.launch().map_or(0.0, |job| self.now.saturating_duration_since(job.started).as_secs_f32())
    }

    /// How long the game has been running.
    pub fn session_time(&self) -> Option<Duration> {
        let since = self.launch()?.running_since?;
        Some(self.now.saturating_duration_since(since))
    }

    /// How long the scram under way has been going.
    pub fn scram_time(&self) -> Option<Duration> {
        let since = self.emergency.since.filter(|_| self.emergency.active())?;
        Some(self.now.saturating_duration_since(since))
    }

    /// Record what is going on, next to the control it is about.
    fn say(&mut self, text: impl Into<String>) {
        if self.job_kind() == Some(JobKind::Launch) {
            self.launch_status = text.into();
        } else {
            self.status = text.into();
        }
    }

    fn notify(&mut self, severity: Severity, text: impl Into<String>) {
        self.notice = Some(Notice { severity, text: text.into() });
    }

    /// Run a launcher command in the background. The stream is lazy: the process is spawned
    /// by the executor, not here, so this function never blocks (and needs no runtime).
    fn start(&mut self, kind: JobKind, subject: String, request: Request) -> Task<Message> {
        let id = self.next_job;
        self.next_job += 1;
        self.now = Instant::now();
        self.job = Some(Job {
            id,
            kind,
            subject,
            handle: None,
            phase: None,
            seen: 0,
            stopping: false,
            stop_grace: None,
            reported_error: false,
            started: self.now,
            running_since: None,
        });
        let launcher = self.launcher.clone();
        let updates = stream::once(async move { backend::spawn(&launcher, &request) }).flat_map(|receiver| {
            stream::unfold(
                receiver,
                |mut receiver| async move { receiver.recv().await.map(|update| (update, receiver)) },
            )
        });
        Task::run(updates, move |update| Message::Backend(id, update))
    }

    /// A command the user asked for. Ignored while another one is running (the buttons are
    /// disabled then; this is the same rule for anything that slips through).
    fn request(&mut self, kind: JobKind, subject: String, request: Request) -> Task<Message> {
        if self.busy() { Task::none() } else { self.start(kind, subject, request) }
    }

    /// Ask the launcher about the current account now, or as soon as it is free.
    fn refresh_when_free(&mut self) -> Task<Message> {
        if self.busy() {
            self.refresh_wanted = true;
            Task::none()
        } else {
            self.refresh_wanted = false;
            self.start(JobKind::Refresh, String::new(), self.status_request())
        }
    }

    fn stop_job(&mut self, grace: Option<Duration>) {
        let Some(job) = &mut self.job else { return };
        job.stopping = true;
        if grace.is_some() {
            job.stop_grace = grace;
        }
        if let Some(handle) = &job.handle {
            match job.stop_grace {
                Some(grace) => handle.stop_within(grace),
                None => handle.stop(),
            }
        }
        let text = match job.kind {
            JobKind::Launch => "Stopping the game…",
            JobKind::SignIn => "Cancelling the sign-in…",
            _ => "Stopping…",
        };
        self.say(text);
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Backend(id, update) => return self.on_update(id, update),
            Message::Navigate(page) => {
                self.page = page;
                // A question that was left unanswered is not kept waiting behind the user's back.
                self.emergency.confirming = false;
            }
            Message::Refresh => return self.request(JobKind::Refresh, String::new(), self.status_request()),
            Message::SignIn { account, device_code } => {
                // Not while a scram runs: the token this stores would outlive it.
                if !is_valid_id(&account) || self.emergency.active() || self.busy() {
                    return Task::none();
                }
                self.notice = None;
                let request = Request::SignIn { account: account.clone(), device_code };
                return self.start(JobKind::SignIn, account, request);
            }
            Message::SignOut { account } => {
                if !is_valid_id(&account) {
                    return Task::none();
                }
                let request = Request::SignOut { account: account.clone() };
                return self.request(JobKind::SignOut, account, request);
            }
            Message::AccountSelected(name) => return self.switch_account(name),
            Message::NewAccountName(text) => self.new_account = text.to_ascii_lowercase().chars().take(64).collect(),
            Message::AddAccount => {
                if self.can_add_account() {
                    let name = std::mem::take(&mut self.new_account);
                    return self.switch_account(name);
                }
            }
            Message::MakeDefaultAccount(account) => {
                if is_valid_id(&account) {
                    let request = Request::DefaultAccount { account: account.clone() };
                    return self.request(JobKind::Refresh, account, request);
                }
            }
            Message::Stop => self.stop_job(None),
            Message::InstanceSelected(choice) => {
                // The instance of a launch in progress is shown, not changed.
                if self.launch().is_none() {
                    self.selected = Some(choice);
                }
            }
            Message::DemoToggled(on) => {
                if self.launch().is_none() {
                    self.demo = on;
                }
            }
            Message::Play => {
                if !self.can_play() {
                    return Task::none();
                }
                let Some(choice) = &self.selected else { return Task::none() };
                let instance = choice.0.id.clone();
                let request =
                    Request::Launch { instance: instance.clone(), account: self.account_name.clone(), demo: self.demo };
                self.notice = None;
                self.progress = None;
                self.protection.clear();
                self.status.clear();
                self.console.new_session();
                self.launch_status = "Starting…".into();
                return self.start(JobKind::Launch, instance, request);
            }
            Message::ClearConsole => self.console.clear(),
            Message::CopyConsole => return iced::clipboard::write(self.console.to_text()),
            Message::Copy(text) => return iced::clipboard::write(text),
            Message::OpenLink(url) => {
                if is_sign_in_link(&url) {
                    return Task::perform(open_link(url), Message::LinkOpened);
                }
                self.notify(Severity::Warning, "That link is not a Microsoft sign-in page; it was not opened.");
            }
            Message::LinkOpened(result) => {
                if let Err(why) = result {
                    self.notify(Severity::Warning, format!("Could not open a browser ({why}). Copy the link instead."));
                }
            }
            Message::OpenRunbookLink(url) => {
                if runbook::is_runbook_link(&url) {
                    return Task::perform(open_link(url), Message::LinkOpened);
                }
                self.notify(Severity::Warning, "That address is not one of the runbook's links; it was not opened.");
            }
            Message::AskScram => self.emergency.confirming = !self.emergency.active(),
            Message::CancelScram => self.emergency.confirming = false,
            Message::ConfirmScram => return self.confirm_scram(),
            Message::Scram(run, update) => return self.on_scram(run, update),
            Message::ScramStartDeadline(run) => {
                if run == self.emergency.run && self.emergency.state == ScramState::StoppingSignIn {
                    let note = "the sign-in in progress did not end in time and may still store a token: \
                                check the accounts afterwards, and scram again if one is signed in";
                    self.emergency.push(note.into(), true);
                    self.console.push_with(Source::Launcher, Level::Error, format!("scram: {note}"));
                    return self.start_scram();
                }
            }
            Message::DismissNotice => self.notice = None,
            Message::ToggleNewInstance => {
                self.new_instance.open = !self.new_instance.open;
                if self.new_instance.open && self.new_instance.versions.is_empty() {
                    let request = Request::Versions { snapshots: self.new_instance.snapshots };
                    return self.request(JobKind::Versions, String::new(), request);
                }
            }
            Message::NewInstanceId(id) => self.new_instance.id = id.chars().take(64).collect(),
            Message::VersionSelected(version) => self.new_instance.version = Some(version),
            Message::SnapshotsToggled(on) => {
                if self.busy() {
                    return Task::none();
                }
                self.new_instance.snapshots = on;
                self.new_instance.versions.clear();
                self.new_instance.version = None;
                return self.start(JobKind::Versions, String::new(), Request::Versions { snapshots: on });
            }
            Message::CreateInstance => {
                if !self.can_create_instance() {
                    return Task::none();
                }
                let Some(version) = &self.new_instance.version else { return Task::none() };
                let id = self.new_instance.id.clone();
                let request = Request::CreateInstance { id: id.clone(), minecraft: version.0.id.clone() };
                self.select_when_listed = Some(id.clone());
                return self.start(JobKind::CreateInstance, id, request);
            }
            Message::Tick(now) => self.now = now,
            Message::CloseRequested(_) => {
                // A running game is shut down properly before the window goes away. A second
                // request does not wait: the worker closes the launcher's control pipe on
                // exit and the launcher finishes the shutdown by itself.
                //
                // A scram that is running is not waited for, and needs no waiting: the
                // launcher doing it is not tied to this window and carries on (see
                // `backend::scram`). One that was confirmed and has not started is: it
                // would not happen at all.
                let game = self.job_kind() == Some(JobKind::Launch);
                if self.closing || !(game || self.emergency.awaiting_start()) {
                    return iced::exit();
                }
                self.closing = true;
                if game {
                    self.stop_job(None);
                    self.say("Stopping the game before closing…");
                } else {
                    self.status = "Closing as soon as the scram has started…".into();
                }
            }
        }
        Task::none()
    }

    // ------------------------------------------------------------------ the emergency stop

    fn confirm_scram(&mut self) -> Task<Message> {
        // Only as the answer to the question, and only once at a time.
        if !self.emergency.confirming || self.emergency.active() {
            return Task::none();
        }
        self.now = Instant::now();
        self.emergency = Emergency { since: Some(self.now), run: self.emergency.run + 1, ..Emergency::default() };
        if self.job_kind() != Some(JobKind::SignIn) {
            return self.start_scram();
        }
        // A sign-in that completes after the scram would store a fresh token. It is ended
        // first, and the scram follows the moment the launcher doing it is gone
        // (`on_finished`) — or after a deadline, should that never be reported.
        self.emergency.state = ScramState::StoppingSignIn;
        self.stop_job(Some(SIGN_IN_ABORT_GRACE));
        let run = self.emergency.run;
        // The timer is created by the executor, not here: this function needs no runtime.
        Task::perform(async { tokio::time::sleep(SCRAM_START_DEADLINE).await }, move |()| {
            Message::ScramStartDeadline(run)
        })
    }

    fn start_scram(&mut self) -> Task<Message> {
        let run = self.emergency.run;
        self.emergency.state = ScramState::Running;
        let launcher = self.launcher.clone();
        let updates = stream::once(async move { backend::scram(&launcher) }).flat_map(|receiver| {
            stream::unfold(
                receiver,
                |mut receiver| async move { receiver.recv().await.map(|update| (update, receiver)) },
            )
        });
        Task::run(updates, move |update| Message::Scram(run, update))
    }

    fn on_scram(&mut self, run: u64, update: ScramUpdate) -> Task<Message> {
        if run != self.emergency.run || self.emergency.state != ScramState::Running {
            return Task::none();
        }
        match update {
            ScramUpdate::Started => {
                self.emergency.started = true;
                // The window was only kept open for this. (A game that is being stopped
                // for the same close is still waited for; `on_finished` exits then.)
                if self.closing && self.launch().is_none() {
                    return iced::exit();
                }
            }
            ScramUpdate::Line(text) => {
                let failed = is_failure_line(&text);
                if !text.trim().is_empty() {
                    let level = if failed { Level::Error } else { Level::Info };
                    self.console.push_with(Source::Launcher, level, format!("scram: {text}"));
                }
                self.emergency.push(text, failed);
            }
            ScramUpdate::Error(message) => {
                self.console.push_with(Source::Launcher, Level::Error, format!("scram: error: {message}"));
                self.emergency.push(format!("error: {message}"), true);
            }
            ScramUpdate::Finished(end) => return self.on_scram_finished(end),
        }
        Task::none()
    }

    fn on_scram_finished(&mut self, end: ScramEnd) -> Task<Message> {
        self.emergency.state = ScramState::Idle;
        let said_something = self.emergency.lines.iter().any(|line| !line.text.trim().is_empty());
        let outcome = match end {
            ScramEnd::FailedToStart(why) => {
                ScramOutcome::NotRun(format!("cannot run the launcher at {}: {why}", self.launcher.display()))
            }
            // Status 0 is not taken at its word: a line that says something is still there
            // counts, and so does a launcher that said nothing at all.
            ScramEnd::Exited(0) if self.emergency.failures > 0 => {
                ScramOutcome::Incomplete("the launcher named things it could not remove".into())
            }
            ScramEnd::Exited(0) if !said_something => ScramOutcome::Incomplete("the launcher reported nothing".into()),
            ScramEnd::Exited(0) => ScramOutcome::Complete,
            ScramEnd::Exited(code) => ScramOutcome::Incomplete(format!("the launcher ended with status {code}")),
            ScramEnd::Killed => ScramOutcome::Incomplete("the launcher was killed before it finished".into()),
            ScramEnd::StillRunning => ScramOutcome::Incomplete(format!(
                "the launcher has not finished after {} minutes. It was left running and may still be removing \
                 things; this window no longer waits for it",
                backend::SCRAM_WAIT_LIMIT.as_secs() / 60
            )),
        };
        match &outcome {
            ScramOutcome::Complete => self.notify(
                Severity::Warning,
                "Scram complete: the sign-ins stored on this machine are removed. Nothing is revoked at Microsoft \
                 yet — follow the runbook, from another device if this one may be compromised.",
            ),
            ScramOutcome::Incomplete(why) => {
                self.console.push_with(Source::Launcher, Level::Error, format!("scram: INCOMPLETE: {why}"));
                self.notify(
                    Severity::Error,
                    format!(
                        "SCRAM INCOMPLETE: {why}. Sign-ins may still be stored on this machine; Security lists what \
                         was reported. Run `bastion-mc scram` in a terminal, and follow the runbook from another \
                         device."
                    ),
                );
            }
            ScramOutcome::NotRun(why) => {
                self.console.push_with(Source::Launcher, Level::Error, format!("scram: did not run: {why}"));
                self.notify(
                    Severity::Error,
                    format!("Scram did not run: {why}. Nothing was removed. Run `bastion-mc scram` in a terminal."),
                );
            }
        }
        let ran = !matches!(outcome, ScramOutcome::NotRun(_));
        self.emergency.outcome = Some(outcome);
        if !ran {
            // A window that was asked to close waited to see the scram start. It did not:
            // that has to be read, so the window stays until it is asked again. (A game
            // that was being stopped for the close is stopped all the same; its line no
            // longer promises that the window closes.)
            if self.closing && self.launch().is_some() {
                self.say("Stopping the game…");
            }
            self.closing = false;
            return Task::none();
        }
        // What was known about the accounts is out of date, whatever was removed: none of
        // it is shown until the launcher has been asked again.
        self.account = None;
        self.login = None;
        self.default_account = None;
        for entry in &mut self.accounts {
            entry.summary = None;
        }
        self.refresh_when_free()
    }

    // ------------------------------------------------------------------ the backend

    fn on_update(&mut self, id: u64, update: Update) -> Task<Message> {
        // Anything from a job that is no longer current is stale.
        if self.job.as_ref().map(|job| job.id) != Some(id) {
            return Task::none();
        }
        match update {
            Update::Started(handle) => {
                if let Some(job) = &mut self.job {
                    if job.stopping {
                        match job.stop_grace {
                            Some(grace) => handle.stop_within(grace),
                            None => handle.stop(),
                        }
                    }
                    job.handle = Some(handle);
                }
            }
            Update::Event(event) => self.on_event(event),
            Update::Stray(text) => self.console.push(Source::Launcher, text),
            Update::Finished(outcome) => return self.on_finished(outcome),
        }
        Task::none()
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Hello { protocol, version } => {
                if protocol != PROTOCOL {
                    self.notify(
                        Severity::Error,
                        format!(
                            "The launcher speaks event protocol {protocol}, this window understands {PROTOCOL}. \
                             Update bastion-mc and bastion-mc-gui together."
                        ),
                    );
                    if let Some(job) = &mut self.job {
                        job.reported_error = true;
                    }
                    self.stop_job(None);
                } else {
                    self.launcher_version = Some(clean_to(&version, 24)).filter(|version| !version.is_empty());
                }
            }
            Event::Status { text } => {
                self.say(clean_to(&text, STATUS_CHARS));
                self.console.push(Source::Launcher, clean_to(&text, CONSOLE_CHARS));
            }
            Event::Phase { phase } => {
                if let Some(job) = &mut self.job {
                    job.phase = Some(phase);
                    job.seen |= 1 << phase as u8;
                    if phase == Phase::Running && job.running_since.is_none() {
                        self.now = Instant::now();
                        job.running_since = Some(self.now);
                    }
                }
                if phase != Phase::Preparing {
                    self.progress = None;
                }
                self.say(phase_text(phase));
            }
            Event::Progress { label, done, total, unit } => {
                self.progress = Some(Progress { label: clean_to(&label, 48), done, total, unit });
            }
            Event::DeviceCode { user_code, verification_uri, expires_in } => {
                // An address that was cut is no longer one of the sign-in addresses (the
                // mark of the cut is not ASCII), so it gets no control that opens it.
                let verification_uri = clean_to(&verification_uri, CODE_ADDRESS_CHARS);
                self.say("Waiting for you to enter the code…");
                self.console
                    .push(Source::Launcher, format!("sign-in: enter the code shown above at {verification_uri}"));
                self.now = Instant::now();
                self.login = Some(LoginPrompt::DeviceCode {
                    user_code: clean_to(&user_code, CODE_CHARS),
                    verification_uri,
                    expires_at: self.now + Duration::from_secs(expires_in.min(MAX_PROMPT_SECS)),
                });
            }
            Event::BrowserLogin { url, timeout_secs } => {
                self.say("Waiting for you to sign in in your browser…");
                self.now = Instant::now();
                self.login = Some(LoginPrompt::Browser {
                    url: clean_to(&url, SIGN_IN_ADDRESS_CHARS),
                    expires_at: self.now + Duration::from_secs(timeout_secs.min(MAX_PROMPT_SECS)),
                });
            }
            Event::Account(account) => {
                if !is_valid_id(&account.account) {
                    return;
                }
                let account = presentable(account);
                if self.account_name.is_empty() {
                    // Started without a label: this is the launcher's default account.
                    self.account_name = account.account.clone();
                }
                self.record_account(&account);
                if account.account == self.account_name {
                    self.account = Some(account);
                }
            }
            Event::Accounts { default, accounts } => self.set_accounts(default, accounts),
            Event::Protection { items } => {
                // Five aspects exist; a few more are tolerated, a flood is not laid out.
                self.protection = items
                    .into_iter()
                    .take(12)
                    .map(|item| Protection {
                        aspect: clean_to(&item.aspect, 24),
                        shield: item.shield,
                        // They wrap; a "partly" detail names each protection that is not in
                        // force and can run to a few hundred characters.
                        detail: clean_to(&item.detail, 1000),
                    })
                    .collect();
            }
            Event::Instances { instances } => self.set_instances(instances),
            Event::Versions { versions, .. } => {
                self.new_instance.versions = versions
                    .into_iter()
                    .take(MAX_VERSIONS)
                    .filter_map(presentable_version)
                    .map(VersionChoice)
                    .collect();
                self.new_instance.version = self.new_instance.versions.first().cloned();
            }
            Event::Log { source, line } => self.console.push(source, clean_to(&line, CONSOLE_CHARS)),
            Event::Exited { code } => {
                let level = if code == 0 || code == 130 { Level::Info } else { Level::Warn };
                self.console.push_with(Source::Launcher, level, format!("the game exited with status {code}"));
                if code != 0 && code != 130 {
                    self.notify(
                        Severity::Warning,
                        format!("The game ended with status {code}. The console has the details."),
                    );
                }
            }
            Event::Error { message, code } => {
                if let Some(job) = &mut self.job {
                    job.reported_error = true;
                }
                if code == 130 {
                    self.say("Cancelled.");
                } else {
                    let message = clean(&message);
                    self.console.push_with(Source::Launcher, Level::Error, format!("error: {message}"));
                    self.notify(Severity::Error, message);
                }
            }
            Event::Unknown => {}
        }
    }

    fn set_instances(&mut self, instances: Vec<Instance>) {
        let mut usable = instances.into_iter().filter_map(presentable_instance);
        self.instances = usable.by_ref().take(MAX_INSTANCES).map(InstanceChoice).collect();
        self.instances_hidden = usable.count();
        let by_id = |id: &str| self.instances.iter().find(|i| i.0.id == id).cloned();
        let wanted = self.select_when_listed.as_deref().and_then(by_id);
        if wanted.is_some() {
            self.select_when_listed = None;
        }
        self.selected = wanted
            .or_else(|| self.selected.as_ref().and_then(|current| by_id(&current.0.id)))
            .or_else(|| self.instances.iter().find(|i| i.launchable()).cloned())
            .or_else(|| self.instances.first().cloned());
    }

    fn on_finished(&mut self, outcome: Outcome) -> Task<Message> {
        let Some(job) = self.job.take() else { return Task::none() };
        self.progress = None;
        self.login = None;
        let ok = outcome == Outcome::Exited(0);
        match &outcome {
            Outcome::FailedToStart(why) => {
                self.notify(
                    Severity::Error,
                    format!("Could not run the launcher at {}: {why}", self.launcher.display()),
                );
            }
            // Ended for or by the scram under way (a sign-in this window stopped, a launch the
            // launcher killed): the scram's own report covers it.
            Outcome::Killed if self.emergency.active() => {}
            Outcome::Killed => self.notify(Severity::Warning, "The launcher was killed before it finished."),
            // A launch's status is the game's; `Event::Exited` has already covered it.
            Outcome::Exited(code) if !ok && *code != 130 && !job.reported_error && job.kind != JobKind::Launch => {
                self.notify(Severity::Error, format!("The launcher ended with status {code}; see the console."));
            }
            Outcome::Exited(_) => {}
        }
        match job.kind {
            JobKind::Launch => {
                // Did the game ever run? If not, say so instead of "the game has ended".
                let ran = matches!(job.phase, Some(Phase::Running | Phase::Stopping));
                self.launch_status = match outcome {
                    Outcome::Exited(130) if ran => "The game was stopped.",
                    Outcome::Exited(130) => "The launch was cancelled.",
                    Outcome::Exited(0) => "The game has exited.",
                    _ if ran => "The game has ended.",
                    _ => "The launch did not complete.",
                }
                .into();
            }
            JobKind::CreateInstance if ok => {
                self.new_instance.open = false;
                self.new_instance.id.clear();
            }
            JobKind::CreateInstance => self.select_when_listed = None,
            JobKind::SignIn if ok => self.status = format!("Signed in ({}).", job.subject),
            JobKind::SignOut if ok => self.status = format!("Signed out ({}).", job.subject),
            JobKind::Refresh | JobKind::Versions if ok => self.status.clear(),
            _ => {}
        }
        // The window is closing and this was what it waited for — unless a scram that was
        // confirmed has yet to start: then that is waited for (see `on_scram`).
        if self.closing && !self.emergency.awaiting_start() {
            return iced::exit();
        }
        // The sign-in a scram was waiting for is gone: now nothing can store a token behind
        // its back. (Whatever else became due below waits its turn; none of it stores one.)
        let scram = if job.kind == JobKind::SignIn && self.emergency.state == ScramState::StoppingSignIn {
            if !self.closing {
                self.status = "The sign-in was ended for the scram.".into();
            }
            self.start_scram()
        } else {
            Task::none()
        };
        // A launch refreshes the stored token and may have learned the player's name.
        let follow_up = if job.kind == JobKind::Launch || self.refresh_wanted {
            self.refresh_when_free()
        } else if self.new_instance.open && self.new_instance.versions.is_empty() && job.kind != JobKind::Versions {
            // The "new instance" form was opened while something else was running: fetch its
            // version list now. (Not after a failed fetch — that would retry forever.)
            self.start(JobKind::Versions, String::new(), Request::Versions { snapshots: self.new_instance.snapshots })
        } else {
            Task::none()
        };
        Task::batch([scram, follow_up])
    }

    /// How often the clock has to advance for what is on screen, if at all. An idle window
    /// has no timer and is not woken up.
    pub fn tick_interval(&self) -> Option<Duration> {
        // The gauge moves only on its own page, and only while a launch is moving.
        let gauge = self.launch().filter(|_| self.page == Page::Launch).and_then(|job| match job.phase {
            _ if job.stopping => None,
            Some(Phase::Stopping) => None,
            Some(Phase::Running) => Some(PULSE_INTERVAL),
            _ => Some(SWEEP_INTERVAL),
        });
        // A countdown, or the time a scram has been running: once a second.
        let clock = (self.login.is_some() || self.emergency.active()).then_some(CLOCK_INTERVAL);
        gauge.or(clock)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut all = vec![window::close_requests().map(Message::CloseRequested)];
        if let Some(interval) = self.tick_interval() {
            all.push(iced::time::every(interval).map(Message::Tick));
        }
        Subscription::batch(all)
    }
}

/// Fixtures shared by the logic tests here and the headless view tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub fn instance(id: &str, error: Option<&str>) -> Instance {
        Instance {
            id: id.into(),
            minecraft: Some("26.3".into()),
            kind: "vanilla".into(),
            detail: None,
            error: error.map(str::to_string),
        }
    }

    pub fn account(signed_in: bool) -> Account {
        Account {
            account: "default".into(),
            signed_in,
            player: signed_in.then(|| "Alex".to_string()),
            store: "kernel keyring (RAM only, cleared at reboot)".into(),
            client_id_configured: true,
            device_flow: false,
        }
    }

    pub fn named(name: &str, signed_in: bool) -> Account {
        Account { account: name.into(), ..account(signed_in) }
    }

    /// Feed backend updates for the current job, the way the worker would.
    pub fn feed(app: &mut App, updates: impl IntoIterator<Item = Update>) {
        let id = app.job.as_ref().expect("a job is running").id;
        for update in updates {
            let _ = app.update(Message::Backend(id, update));
        }
    }

    pub fn events(events: impl IntoIterator<Item = Event>) -> Vec<Update> {
        events.into_iter().map(Update::Event).collect()
    }

    pub fn finish(app: &mut App, code: i32) {
        feed(app, [Update::Finished(Outcome::Exited(code))]);
    }

    /// An app that has finished its first status refresh.
    pub fn ready(signed_in: bool) -> App {
        let (mut app, _task) = App::boot(PathBuf::from("/nonexistent/bastion-mc"), "default".into());
        assert_eq!(app.job_kind(), Some(JobKind::Refresh));
        feed(
            &mut app,
            events([
                Event::Hello { protocol: PROTOCOL, version: "0.1.0".into() },
                Event::Account(account(signed_in)),
                Event::Instances {
                    instances: vec![instance("broken", Some("bad signature")), instance("vanilla", None)],
                },
            ]),
        );
        finish(&mut app, 0);
        assert!(!app.busy());
        app
    }

    pub fn stop_handle(sender: tokio::sync::mpsc::Sender<Duration>) -> Handle {
        backend::test_handle(sender)
    }

    /// Feed what the launcher running the current scram says.
    pub fn scram(app: &mut App, updates: impl IntoIterator<Item = ScramUpdate>) {
        let run = app.emergency.run;
        for update in updates {
            let _ = app.update(Message::Scram(run, update));
        }
    }

    pub fn scram_lines(app: &mut App, lines: &[&str]) {
        scram(app, lines.iter().map(|line| ScramUpdate::Line(line.to_string())));
    }

    /// Does `task` end the program? Only to be asked of a task that is this or nothing: a
    /// task that starts a launcher would start it here.
    pub fn exits(task: Task<Message>) -> bool {
        use iced::futures::FutureExt;
        use iced_test::runtime::{Action, task::into_stream};
        assert!(task.units() <= 1, "more than one thing to do");
        let first = into_stream(task).and_then(|mut actions| actions.next().now_or_never().flatten());
        matches!(first, Some(Action::Exit))
    }

    fn close(app: &mut App) -> Task<Message> {
        app.update(Message::CloseRequested(window::Id::unique()))
    }

    /// Ask the window to close: does it?
    pub fn closes(app: &mut App) -> bool {
        exits(close(app))
    }

    /// Ask and confirm: the two presses.
    pub fn confirm_scram(app: &mut App) {
        let _ = app.update(Message::AskScram);
        let _ = app.update(Message::ConfirmScram);
    }

    /// Put the clock `after` the start of the current job, the way ticks would.
    pub fn at(app: &mut App, after: Duration) {
        let started = app.job.as_ref().expect("a job is running").started;
        let _ = app.update(Message::Tick(started + after));
    }

    /// Put the clock at the moment the game has been running for `time`.
    pub fn running_for(app: &mut App, time: Duration) {
        let since = app.job.as_ref().and_then(|job| job.running_since).expect("a game is running");
        let _ = app.update(Message::Tick(since + time));
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};

    #[test]
    fn first_refresh_fills_in_account_and_instances() {
        let app = ready(true);
        assert_eq!(app.account.as_ref().unwrap().player.as_deref(), Some("Alex"));
        assert_eq!(app.instances.len(), 2);
        assert_eq!(app.selected.as_ref().unwrap().0.id, "vanilla", "the first launchable instance is preselected");
        assert!(app.can_play());
        assert_eq!(app.page, Page::Launch, "the window opens on the launch page");
        assert_eq!(app.launcher_version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn play_is_disabled_while_launching_and_running_and_reenabled_after_exit() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        assert_eq!(app.job_kind(), Some(JobKind::Launch));
        assert_eq!(app.job.as_ref().unwrap().subject, "vanilla");
        assert!(!app.can_play(), "disabled the moment it is pressed");
        let _ = app.update(Message::Play);
        assert_eq!(app.next_job, 3, "a second press does not start a second launcher");

        feed(&mut app, events([Event::Phase { phase: Phase::Preparing }]));
        feed(&mut app, events([Event::Progress { label: "assets".into(), done: 50, total: 200, unit: Unit::Bytes }]));
        let progress = app.progress.clone().expect("progress is shown during downloads");
        assert_eq!((progress.fraction(), progress.percent()), (0.25, 25));
        assert_eq!(progress.amount(), "0.0 / 0.0 MiB", "50 of 200 bytes");
        assert!(!app.can_play());

        feed(&mut app, events([Event::Phase { phase: Phase::Verifying }]));
        assert!(app.progress.is_none(), "the bar goes away once downloads are over");
        feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
        assert_eq!(app.job.as_ref().unwrap().phase, Some(Phase::Running));
        assert!(!app.can_play(), "still disabled while the game runs");

        feed(
            &mut app,
            events([Event::Log { source: Source::Game, line: "Stopping!".into() }, Event::Exited { code: 0 }]),
        );
        feed(&mut app, [Update::Finished(Outcome::Exited(0))]);
        assert_eq!(app.launch_status, "The game has exited.");
        assert_eq!(app.job_kind(), Some(JobKind::Refresh), "a launch is followed by a status refresh");
        feed(&mut app, [Update::Finished(Outcome::Exited(0))]);
        assert!(app.can_play(), "enabled again");
        assert!(app.notice.is_none());
        assert!(app.console.lines().any(|l| l.source == Source::Game && l.text == "Stopping!"));
    }

    #[test]
    fn play_needs_an_account_or_demo_mode_and_a_launchable_instance() {
        let mut app = ready(false);
        assert_eq!(app.play_blocker(), Some("Sign in, or switch on demo mode."));
        let _ = app.update(Message::Play);
        assert!(!app.busy(), "nothing is started");
        let _ = app.update(Message::DemoToggled(true));
        assert!(app.can_play());

        let broken = app.instances.iter().find(|i| !i.launchable()).cloned().unwrap();
        let _ = app.update(Message::InstanceSelected(broken));
        assert!(!app.can_play());
        assert!(app.play_blocker().unwrap().contains("cannot be launched"));

        let mut empty = app;
        empty.set_instances(Vec::new());
        assert_eq!(empty.play_blocker(), Some("Create an instance first."));
    }

    #[test]
    fn the_instance_and_demo_mode_of_a_launch_under_way_cannot_be_changed() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        let other = app.instances.iter().find(|i| i.0.id == "broken").cloned().unwrap();
        let _ = app.update(Message::InstanceSelected(other));
        assert_eq!(app.selected.as_ref().unwrap().0.id, "vanilla", "what is shown is what is being launched");
        let _ = app.update(Message::DemoToggled(true));
        assert!(!app.demo);
    }

    #[test]
    fn stop_reaches_the_launcher_even_if_pressed_before_it_has_started() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        let _ = app.update(Message::Stop); // no handle yet
        assert!(app.job.as_ref().unwrap().stopping);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        feed(&mut app, [Update::Started(stop_handle(sender))]);
        assert!(receiver.try_recv().is_ok(), "the stop is forwarded as soon as the handle arrives");
        assert_eq!(app.launch_status, "Stopping the game…");

        feed(&mut app, events([Event::Error { message: "cancelled".into(), code: 130 }]));
        feed(&mut app, [Update::Finished(Outcome::Exited(130))]);
        assert!(app.notice.is_none(), "stopping on request is not an error");
        assert_eq!(app.launch_status, "The launch was cancelled.", "the game never ran");
    }

    #[test]
    fn device_code_is_shown_until_sign_in_ends() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
        assert_eq!(app.job_kind(), Some(JobKind::SignIn));
        feed(
            &mut app,
            events([Event::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://www.microsoft.com/link".into(),
                expires_in: 900,
            }]),
        );
        let Some(LoginPrompt::DeviceCode { user_code, verification_uri, expires_at }) = &app.login else {
            panic!("no prompt: {:?}", app.login)
        };
        assert_eq!((user_code.as_str(), verification_uri.as_str()), ("ABCD-EFGH", "https://www.microsoft.com/link"));
        let left = expires_at.duration_since(app.now).as_secs();
        assert!((895..=900).contains(&left), "{left}");

        feed(&mut app, events([Event::Account(account(true))]));
        feed(&mut app, [Update::Finished(Outcome::Exited(0))]);
        assert!(app.login.is_none(), "the code disappears once the sign-in is over");
        assert!(app.signed_in() && app.can_play());
        assert_eq!(app.status, "Signed in (default).");
    }

    #[test]
    fn a_cancelled_sign_in_is_not_an_error() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: false });
        feed(
            &mut app,
            events([Event::BrowserLogin { url: "https://login.microsoftonline.com/x".into(), timeout_secs: 300 }]),
        );
        assert!(matches!(app.login, Some(LoginPrompt::Browser { .. })));
        let _ = app.update(Message::Stop);
        feed(&mut app, events([Event::Error { message: "cancelled".into(), code: 130 }]));
        feed(&mut app, [Update::Finished(Outcome::Exited(130))]);
        assert!(app.notice.is_none() && app.login.is_none() && !app.busy());
        assert_eq!(app.status, "Cancelled.");
    }

    #[test]
    fn launcher_errors_become_one_notice() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        feed(
            &mut app,
            events([Event::Error { message: "INTEGRITY VIOLATION: client jar does not match".into(), code: 4 }]),
        );
        feed(&mut app, [Update::Finished(Outcome::Exited(4))]);
        assert_eq!(app.launch_status, "The launch did not complete.");
        let notice = app.notice.clone().unwrap();
        assert_eq!(notice.severity, Severity::Error);
        assert!(notice.text.starts_with("INTEGRITY VIOLATION"), "the launcher's own words, not a generic status");
        let _ = app.update(Message::DismissNotice);
        assert!(app.notice.is_none());
    }

    #[test]
    fn a_launcher_that_cannot_start_or_dies_is_reported() {
        let mut app = ready(true);
        let _ = app.update(Message::Refresh);
        feed(&mut app, [Update::Finished(Outcome::FailedToStart("No such file or directory".into()))]);
        assert!(app.notice.as_ref().unwrap().text.contains("/nonexistent/bastion-mc"));
        assert!(!app.busy());

        let _ = app.update(Message::Refresh);
        feed(&mut app, [Update::Stray("thread 'main' panicked".into()), Update::Finished(Outcome::Exited(101))]);
        assert!(app.notice.as_ref().unwrap().text.contains("status 101"));
        assert!(app.console.lines().any(|l| l.text.contains("panicked")));
    }

    #[test]
    fn stale_updates_from_an_earlier_job_are_ignored() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        let stale = app.job.as_ref().unwrap().id - 1;
        let _ = app.update(Message::Backend(stale, Update::Finished(Outcome::Exited(0))));
        assert_eq!(app.job_kind(), Some(JobKind::Launch), "the running job is untouched");
        let _ = app.update(Message::Backend(stale, Update::Event(Event::Account(account(false)))));
        assert!(app.signed_in());
    }

    #[test]
    fn a_protocol_mismatch_stops_the_job() {
        let mut app = ready(true);
        let _ = app.update(Message::Refresh);
        feed(&mut app, events([Event::Hello { protocol: PROTOCOL + 1, version: "9.9.9".into() }]));
        assert!(app.job.as_ref().unwrap().stopping);
        assert!(app.notice.as_ref().unwrap().text.contains("protocol"));
        assert_eq!(app.launcher_version.as_deref(), Some("0.1.0"), "a launcher that is not understood is not quoted");
    }

    #[test]
    fn creating_an_instance_selects_it_and_closes_the_form() {
        let mut app = ready(true);
        let _ = app.update(Message::ToggleNewInstance);
        assert_eq!(app.job_kind(), Some(JobKind::Versions), "opening the form fetches the version list");
        let version = Version { id: "26.3".into(), kind: "release".into(), released: "2026-09-15".into() };
        feed(
            &mut app,
            events([Event::Versions {
                latest_release: "26.3".into(),
                latest_snapshot: "26.4-snapshot-8".into(),
                versions: vec![version.clone()],
            }]),
        );
        feed(&mut app, [Update::Finished(Outcome::Exited(0))]);
        assert_eq!(app.new_instance.version, Some(VersionChoice(version)), "the newest version is preselected");

        assert!(!app.can_create_instance(), "no name yet");
        for bad in ["Has Space", "../up", "UPPER", "vanilla"] {
            let _ = app.update(Message::NewInstanceId(bad.into()));
            assert!(!app.can_create_instance(), "{bad:?} (invalid, or already exists)");
        }
        let _ = app.update(Message::NewInstanceId("survival".into()));
        assert!(app.can_create_instance());
        let _ = app.update(Message::CreateInstance);
        assert_eq!(app.job_kind(), Some(JobKind::CreateInstance));
        feed(
            &mut app,
            events([Event::Instances { instances: vec![instance("survival", None), instance("vanilla", None)] }]),
        );
        feed(&mut app, [Update::Finished(Outcome::Exited(0))]);
        assert_eq!(app.selected.as_ref().unwrap().0.id, "survival");
        assert!(!app.new_instance.open && app.new_instance.id.is_empty());
    }

    #[test]
    fn the_version_list_is_fetched_even_if_the_form_was_opened_mid_job() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
        let _ = app.update(Message::ToggleNewInstance);
        assert_eq!(app.job_kind(), Some(JobKind::SignIn), "the sign-in is not disturbed");
        assert!(app.new_instance.open && app.new_instance.versions.is_empty());
        feed(&mut app, events([Event::Error { message: "cancelled".into(), code: 130 }]));
        feed(&mut app, [Update::Finished(Outcome::Exited(130))]);
        assert_eq!(app.job_kind(), Some(JobKind::Versions), "fetched as soon as the launcher is free");

        // A fetch that fails is reported once, not retried in a loop.
        feed(&mut app, events([Event::Error { message: "network error: DNS lookup failed".into(), code: 7 }]));
        feed(&mut app, [Update::Finished(Outcome::Exited(7))]);
        assert!(!app.busy());
        assert!(app.notice.as_ref().unwrap().text.contains("DNS lookup failed"));
    }

    #[test]
    fn closing_the_window_stops_a_running_game_first() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        feed(&mut app, [Update::Started(stop_handle(sender))]);
        feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
        assert!(!closes(&mut app), "the window stays until the game is down");
        assert!(app.closing && app.busy());
        assert!(receiver.try_recv().is_ok(), "the game was asked to stop");
        // The game is down: now it goes.
        let id = app.job.as_ref().unwrap().id;
        assert!(exits(app.update(Message::Backend(id, Update::Finished(Outcome::Exited(130))))));

        // Asked twice, it does not wait for the game.
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        assert!(!closes(&mut app));
        assert!(closes(&mut app));

        // With nothing running there is nothing to wait for.
        assert!(closes(&mut ready(true)));
        let mut refreshing = ready(true);
        let _ = refreshing.update(Message::Refresh);
        assert!(closes(&mut refreshing));
    }

    #[test]
    fn only_microsoft_sign_in_links_are_opened() {
        for ok in [
            "https://www.microsoft.com/link",
            "https://microsoft.com/devicelogin",
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?client_id=x&state=y",
        ] {
            assert!(is_sign_in_link(ok), "{ok}");
        }
        for bad in [
            "http://www.microsoft.com/link",
            "https://www.microsoft.com.evil.example/link",
            "https://evil.example/?https://www.microsoft.com/link",
            "https://www.microsoft.com@evil.example/",
            "https://www.microsoft.com:8443/link",
            "https://www.microsoft.com/link with space",
            "https://www.microsoft.com/\u{202e}link",
            "file:///etc/passwd",
            "-https://www.microsoft.com/link",
            "",
        ] {
            assert!(!is_sign_in_link(bad), "{bad:?}");
        }
        let mut app = ready(false);
        let _ = app.update(Message::OpenLink("https://evil.example/".into()));
        assert_eq!(app.notice.as_ref().unwrap().severity, Severity::Warning);
    }

    #[test]
    fn instance_ids_follow_the_launchers_rule() {
        for good in ["vanilla-26.3", "a", "fabric_perf.2", "0abc"] {
            assert!(is_valid_id(good), "{good}");
        }
        for bad in ["", ".hidden", "../x", "a/b", "UPPER", "a..b", "-lead", "sp ace"] {
            assert!(!is_valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_drop_down_names_an_instance_in_the_room_it_has() {
        assert_eq!(InstanceChoice(instance("vanilla", None)).label(28), "vanilla");
        assert_eq!(InstanceChoice(instance("vanilla", Some("x"))).label(28), "vanilla (broken)");
        let long = InstanceChoice(instance(&"a".repeat(64), None));
        let unusable = InstanceChoice(instance(&"a".repeat(64), Some("x")));
        for chars in 12..80 {
            let (whole, marked) = (long.label(chars), unusable.label(chars));
            assert!(whole.chars().count() <= chars && marked.chars().count() <= chars, "{chars}: {whole} {marked}");
            assert_eq!(whole.contains('…'), chars < 64, "{chars}: {whole}");
            assert_eq!(marked.contains('…'), chars < 64 + " (broken)".len(), "{chars}: {marked}");
            assert!(marked.ends_with(" (broken)"), "the mark is never what is cut off: {marked}");
        }

        let version = |id: &str, released: &str| {
            VersionChoice(Version { id: id.into(), kind: "release".into(), released: released.into() })
        };
        assert_eq!(version("26.3", "2026-09-15").label(30), "26.3  (2026-09-15)");
        assert_eq!(version("26.3", "").label(30), "26.3");
        assert_eq!(version("26.4-snapshot-12", "2026-10-02").label(20), "26.4-snapshot-12", "the name before the day");
        assert_eq!(version("26.4-snapshot-12", "2026-10-02").label(10), "26.4-snap…");
    }

    #[test]
    fn progress_edge_cases() {
        let p = Progress { label: "assets".into(), done: 0, total: 0, unit: Unit::Files };
        assert_eq!(p.fraction(), 1.0, "nothing to do is complete, not a division by zero");
        let p = Progress { label: "assets".into(), done: 9, total: 4, unit: Unit::Files };
        assert_eq!(p.fraction(), 1.0);
        assert_eq!((p.amount().as_str(), p.percent()), ("9 / 4 files", 100));
        let p = Progress { label: "assets".into(), done: 163577856, total: 419430400, unit: Unit::Bytes };
        assert_eq!((p.amount().as_str(), p.percent()), ("156.0 / 400.0 MiB", 39));
    }

    #[test]
    fn the_amount_read_out_stays_on_its_line_whatever_the_numbers() {
        let amount = |done: u64, total: u64, unit: Unit| Progress { label: "x".into(), done, total, unit }.amount();
        assert_eq!(amount(0, 9999 << 20, Unit::Bytes), "0.0 / 9999.0 MiB");
        assert_eq!(amount(5 << 30, 10 << 30, Unit::Bytes), "5.0 / 10.0 GiB", "a fifth digit goes up a unit");
        assert_eq!(amount(u64::MAX - 1, u64::MAX, Unit::Bytes), "16.0 / 16.0 EiB");
        assert_eq!(amount(u64::MAX, 1, Unit::Bytes), "16.0 / 0.0 EiB", "more done than there was to do");
        assert_eq!(amount(2114, 5149, Unit::Files), "2114 / 5149 files");
        assert_eq!(amount(999_999, 1_000_000, Unit::Files), "999999 / 1.0e6 files");
        assert_eq!(amount(u64::MAX, u64::MAX, Unit::Files), "1.8e19 / 1.8e19 files");
        let numbers = [0, 1, 999, 9999, 99_999, 999_999, 1_000_000, 1 << 20, 1 << 30, 1 << 40, 1 << 50, 1 << 60];
        for done in numbers.into_iter().chain([u64::MAX - 1, u64::MAX, 10484711424, 10485235712]) {
            for total in numbers.into_iter().chain([u64::MAX, 10484711424, 10485235712]) {
                for unit in [Unit::Bytes, Unit::Files] {
                    let shown = amount(done, total, unit);
                    assert!(shown.chars().count() <= Progress::AMOUNT_CHARS, "{done} of {total}: {shown}");
                }
            }
        }
    }

    #[test]
    fn navigation_changes_the_page_and_nothing_else() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
        for page in Page::ALL {
            let _ = app.update(Message::Navigate(page));
            assert_eq!(app.page, page);
            assert_eq!(app.job_kind(), Some(JobKind::Launch), "looking around does not disturb the game");
        }
        // A question left open is withdrawn, not answered later by a stray press.
        let _ = app.update(Message::Navigate(Page::Security));
        let _ = app.update(Message::AskScram);
        assert!(app.emergency.confirming);
        let _ = app.update(Message::Navigate(Page::Launch));
        let _ = app.update(Message::Navigate(Page::Security));
        assert!(!app.emergency.confirming);
        let _ = app.update(Message::ConfirmScram);
        assert!(!app.emergency.active());
    }

    #[test]
    fn the_stages_of_a_launch_follow_what_the_launcher_announces() {
        use Stage::*;
        let mut app = ready(true);
        assert_eq!(app.sequence(), [Pending; 5], "nothing is being launched");
        let _ = app.update(Message::Play);
        assert_eq!(app.sequence(), [Pending; 5], "started, nothing announced yet");
        feed(&mut app, events([Event::Phase { phase: Phase::Preparing }]));
        assert_eq!(app.sequence(), [Active, Pending, Pending, Pending, Pending]);
        feed(&mut app, events([Event::Phase { phase: Phase::Verifying }]));
        assert_eq!(app.sequence(), [Done, Active, Pending, Pending, Pending]);
        // Demo mode: the launcher never announces a sign-in.
        feed(&mut app, events([Event::Phase { phase: Phase::Starting }]));
        assert_eq!(app.sequence(), [Done, Done, Skipped, Active, Pending]);
        assert!(app.session_time().is_none());
        feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
        assert_eq!(app.sequence(), [Done, Done, Skipped, Done, Active]);
        running_for(&mut app, Duration::from_secs(3600));
        assert_eq!(app.session_time(), Some(Duration::from_secs(3600)));
        feed(&mut app, events([Event::Phase { phase: Phase::Stopping }]));
        assert_eq!(app.sequence(), [Done, Done, Skipped, Done, Done]);
        finish(&mut app, 0);
        assert_eq!(app.sequence(), [Pending; 5]);

        // With an account every stage is gone through.
        finish(&mut app, 0); // the refresh after the launch
        let _ = app.update(Message::Play);
        for phase in [Phase::Preparing, Phase::Verifying, Phase::Authenticating] {
            feed(&mut app, events([Event::Phase { phase }]));
        }
        assert_eq!(app.sequence(), [Done, Done, Active, Pending, Pending]);
    }

    #[test]
    fn the_clock_only_ticks_while_something_on_screen_depends_on_it() {
        let mut app = ready(false);
        assert_eq!(app.tick_interval(), None, "an idle window is not woken up");
        for page in Page::ALL {
            let _ = app.update(Message::Navigate(page));
            assert_eq!(app.tick_interval(), None, "{page:?}");
        }

        // A countdown: once a second, on every page.
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
        assert_eq!(app.tick_interval(), None, "nothing to count down yet");
        feed(
            &mut app,
            events([Event::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://www.microsoft.com/link".into(),
                expires_in: 900,
            }]),
        );
        assert_eq!(app.tick_interval(), Some(CLOCK_INTERVAL));
        feed(&mut app, events([Event::Account(account(true))]));
        finish(&mut app, 0);
        assert_eq!(app.tick_interval(), None);

        // A launch: the gauge sweeps while it is prepared, breathes while the game runs,
        // and stands still while it stops — and only while the gauge is on screen.
        let _ = app.update(Message::Navigate(Page::Launch));
        let _ = app.update(Message::Play);
        assert_eq!(app.tick_interval(), Some(SWEEP_INTERVAL));
        let _ = app.update(Message::Navigate(Page::Console));
        assert_eq!(app.tick_interval(), None, "the gauge is not on this page");
        let _ = app.update(Message::Navigate(Page::Launch));
        feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
        assert_eq!(app.tick_interval(), Some(PULSE_INTERVAL));
        assert!(PULSE_INTERVAL >= Duration::from_millis(100), "a running game is not competed with for frames");
        let _ = app.update(Message::Stop);
        assert_eq!(app.tick_interval(), None);
        feed(&mut app, events([Event::Phase { phase: Phase::Stopping }]));
        assert_eq!(app.tick_interval(), None);
        finish(&mut app, 130);
        finish(&mut app, 0);
        assert_eq!(app.tick_interval(), None, "idle again");
    }

    #[test]
    fn what_the_launcher_says_is_made_printable_and_bounded_before_it_is_kept() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        feed(
            &mut app,
            events([
                Event::Protection {
                    items: (0..40)
                        .map(|i| Protection {
                            aspect: format!("aspect\u{202e}{i}\u{7}"),
                            shield: bastion_mc_proto::Shield::On,
                            detail: "x".repeat(5000),
                        })
                        .collect(),
                },
                Event::Progress { label: "assets\u{1b}[31m".repeat(50), done: 1, total: 2, unit: Unit::Files },
            ]),
        );
        assert_eq!(app.protection.len(), 12);
        assert_eq!(app.protection[3].aspect, "aspect3");
        assert!(app.protection[0].detail.chars().count() <= 1001);
        assert!(app.progress.as_ref().unwrap().label.chars().count() <= 49);
        assert!(!app.progress.as_ref().unwrap().label.contains('\u{1b}'));
    }

    #[test]
    fn a_sign_in_prompt_is_bounded_in_what_it_says_and_in_how_long_it_counts() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
        feed(
            &mut app,
            events([Event::DeviceCode {
                user_code: format!("AB\u{1b}CD-{}", "X".repeat(500)),
                verification_uri: format!("https://www.microsoft.com/link\u{202e}?{}", "y".repeat(5000)),
                // Added to the clock as it is, this would overflow it (and end the program).
                expires_in: u64::MAX,
            }]),
        );
        let Some(LoginPrompt::DeviceCode { user_code, verification_uri, expires_at }) = &app.login else {
            panic!("no prompt: {:?}", app.login)
        };
        assert!(user_code.starts_with("ABCD-XXX") && user_code.chars().count() == CODE_CHARS + 1, "{user_code}");
        assert_eq!(verification_uri.chars().count(), CODE_ADDRESS_CHARS + 1);
        assert!(verification_uri.starts_with("https://www.microsoft.com/link?yyy"), "{verification_uri}");
        assert!(!is_sign_in_link(verification_uri), "an address that had to be cut is not one to open");
        assert_eq!(expires_at.duration_since(app.now), Duration::from_secs(MAX_PROMPT_SECS));
        assert!(app.console.lines().all(|line| !line.text.contains('\u{202e}')));

        feed(
            &mut app,
            events([Event::BrowserLogin {
                url: format!("https://login.microsoftonline.com/\u{7}{}", "a".repeat(10_000)),
                timeout_secs: u64::MAX,
            }]),
        );
        let Some(LoginPrompt::Browser { url, expires_at }) = &app.login else { panic!("no prompt: {:?}", app.login) };
        assert_eq!(url.chars().count(), SIGN_IN_ADDRESS_CHARS + 1);
        assert!(url.starts_with("https://login.microsoftonline.com/aaa") && !is_sign_in_link(url));
        assert_eq!(expires_at.duration_since(app.now), Duration::from_secs(MAX_PROMPT_SECS));

        // What the launcher really sends is kept as it is.
        let real = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?client_id=x&state=y";
        feed(&mut app, events([Event::BrowserLogin { url: real.into(), timeout_secs: 300 }]));
        assert!(matches!(&app.login, Some(LoginPrompt::Browser { url, .. }) if url == real && is_sign_in_link(url)));
    }

    #[test]
    fn what_the_launcher_reports_is_bounded_where_it_is_shown_and_longer_in_the_console() {
        let long = format!("checking\u{1b}[31m \u{202e}files {}", "z".repeat(9000));
        let mut app = ready(true);
        let _ = app.update(Message::Refresh);
        feed(&mut app, events([Event::Status { text: long.clone() }]));
        assert!(app.status.starts_with("checking[31m files zzz"), "{}", &app.status[..40]);
        assert_eq!(app.status.chars().count(), STATUS_CHARS + 1);
        feed(&mut app, events([Event::Error { message: long.clone(), code: 3 }]));
        finish(&mut app, 3);
        let notice = app.notice.as_ref().expect("the error is shown");
        assert!(notice.text.starts_with("checking[31m files zzz") && notice.text.chars().count() <= 2001);

        let _ = app.update(Message::Play);
        feed(
            &mut app,
            events([Event::Status { text: long.clone() }, Event::Log { source: Source::Game, line: long.clone() }]),
        );
        assert_eq!(app.launch_status.chars().count(), STATUS_CHARS + 1);
        let shown: Vec<&crate::console::Line> = app.console.lines().collect();
        assert!(shown.len() >= 4);
        for line in shown {
            assert!(!line.text.contains('\u{1b}') && !line.text.contains('\u{202e}'), "{:?}", &line.text[..30]);
            assert!(line.text.chars().count() <= CONSOLE_CHARS + 1);
        }
        assert!(app.console.lines().any(|line| line.text.chars().count() > 4 * STATUS_CHARS), "more than the status");
    }

    #[test]
    fn instances_and_versions_from_the_launcher_are_checked_before_they_are_shown_or_used() {
        let mut app = ready(true);
        let _ = app.update(Message::Refresh);
        let hostile = Instance {
            id: "ok".into(),
            minecraft: Some(format!("26.3\u{1b}[31m{}", "x".repeat(500))),
            kind: "cur\u{202e}ated".into(),
            detail: Some("d".repeat(5000)),
            error: Some(format!("bad\n{}", "e".repeat(5000))),
        };
        let listed = vec![
            instance("--demo", None),
            instance("../x", None),
            instance("UPPER", None),
            instance("", None),
            hostile,
            instance("vanilla", None),
        ];
        feed(&mut app, events([Event::Instances { instances: listed }]));
        let ids: Vec<&str> = app.instances.iter().map(|choice| choice.0.id.as_str()).collect();
        assert_eq!(ids, ["ok", "vanilla"], "a name that could not be a launcher argument is never offered");
        assert_eq!(app.instances_hidden, 0, "what was refused is not counted as merely not shown");
        let kept = &app.instances[0].0;
        assert_eq!(kept.kind, "curated");
        let version = kept.minecraft.as_ref().unwrap();
        assert!(version.starts_with("26.3[31m") && version.chars().count() == 49, "{version}");
        assert_eq!(kept.detail.as_ref().unwrap().chars().count(), 241);
        let error = kept.error.as_ref().unwrap();
        assert!(error.starts_with("badeee") && error.chars().count() == 401);
        assert!(!app.instances[0].launchable());
        // An error without words still means "cannot be launched".
        feed(&mut app, events([Event::Instances { instances: vec![instance("silent", Some(""))] }]));
        assert!(!app.instances[0].launchable());
        finish(&mut app, 0);

        let _ = app.update(Message::ToggleNewInstance);
        let version = |id: &str| Version { id: id.into(), kind: "release".into(), released: "2026-09-15".into() };
        // A name is what `instance new --minecraft` is given. One that cannot be shown as it
        // is, or would read as an option, is not offered under a name that was made up here.
        let refused = ["", "  ", "26.3\u{7}", "26.3\u{202e}", "--snapshots", "-", &"9".repeat(MAX_VERSION_CHARS + 1)];
        let mut versions: Vec<Version> = refused.iter().map(|id| version(id)).collect();
        versions.push(Version {
            id: "26.3".into(),
            kind: "rel\u{1b}ease".repeat(20),
            released: "2026-09-15\u{7}".repeat(20),
        });
        versions.push(version(&"9".repeat(MAX_VERSION_CHARS)));
        versions.extend((0..5000).map(|i| version(&format!("1.{i}"))));
        feed(
            &mut app,
            events([Event::Versions { latest_release: "26.3".into(), latest_snapshot: String::new(), versions }]),
        );
        let shown = &app.new_instance.versions;
        let ids: Vec<&str> = shown.iter().take(3).map(|choice| choice.0.id.as_str()).collect();
        assert_eq!(ids, ["26.3", &"9".repeat(MAX_VERSION_CHARS), "1.0"], "the usable ones, under their own names");
        assert!(shown[0].0.kind.chars().count() <= 25 && !shown[0].0.kind.contains('\u{1b}'), "{:?}", shown[0].0);
        assert!(shown[0].0.released.chars().count() <= 17 && !shown[0].0.released.contains('\u{7}'));
        assert!(shown.len() <= MAX_VERSIONS, "{}", shown.len());
        assert_eq!(app.new_instance.version.as_ref().map(|choice| choice.0.id.as_str()), Some("26.3"));
    }
}

#[cfg(test)]
mod account_tests {
    use super::{testing::*, *};

    fn labels(app: &App) -> Vec<&str> {
        app.accounts.iter().map(|entry| entry.label.as_str()).collect()
    }

    fn entry<'a>(app: &'a App, label: &str) -> &'a AccountEntry {
        app.accounts.iter().find(|entry| entry.label == label).unwrap_or_else(|| panic!("no account {label}"))
    }

    fn feed_idle_accounts(app: &mut App) {
        let _ = app.update(Message::Refresh);
        feed(
            app,
            events([Event::Accounts {
                default: "default".into(),
                accounts: vec![named("alt", false), named("default", true)],
            }]),
        );
        finish(app, 0);
    }

    #[test]
    fn without_a_label_the_launchers_default_account_is_adopted() {
        let (mut app, _task) = App::boot(PathBuf::from("/nonexistent/bastion-mc"), String::new());
        assert!(app.accounts.is_empty());
        feed(
            &mut app,
            events([
                Event::Account(named("work", true)),
                Event::Accounts { default: "work".into(), accounts: vec![named("alt", false), named("work", true)] },
            ]),
        );
        finish(&mut app, 0);
        assert_eq!(app.account_name, "work");
        assert!(app.signed_in());
        assert_eq!(labels(&app), ["alt", "work"]);
        assert_eq!(app.default_account.as_deref(), Some("work"));
    }

    #[test]
    fn every_account_keeps_the_summary_the_launcher_gave() {
        let mut app = ready(true);
        assert_eq!(labels(&app), ["default"], "the account in use is listed from the start");
        assert_eq!(entry(&app, "default").summary, Some(account(true)), "a single report fills its entry");
        feed_idle_accounts(&mut app);
        assert_eq!(labels(&app), ["alt", "default"]);
        let alt = entry(&app, "alt").summary.as_ref().expect("the summary is kept, not just the label");
        assert!(!alt.signed_in && alt.player.is_none());
        assert_eq!(alt.store, "kernel keyring (RAM only, cleared at reboot)");
        let default = entry(&app, "default").summary.as_ref().unwrap();
        assert!(default.signed_in);
        assert_eq!(default.player.as_deref(), Some("Alex"));
        assert!(!app.busy(), "no further launcher call was needed to know all this");
    }

    #[test]
    fn switching_asks_the_launcher_about_the_other_account_and_shows_nothing_stale() {
        let mut app = ready(true);
        feed_idle_accounts(&mut app);
        let _ = app.update(Message::AccountSelected("alt".into()));
        assert_eq!(app.account_name, "alt");
        assert!(app.account.is_none() && !app.signed_in(), "the previous account's state is not shown for this one");
        assert_eq!(app.job_kind(), Some(JobKind::Refresh));
        // An answer about the old account is not taken for the new one.
        feed(&mut app, events([Event::Account(named("default", true))]));
        assert!(app.account.is_none());
        feed(&mut app, events([Event::Account(named("alt", false))]));
        finish(&mut app, 0);
        assert!(!app.signed_in());
        assert_eq!(
            app.play_blocker(),
            Some("Sign in, or switch on demo mode."),
            "the other account's sign-in does not count"
        );
    }

    #[test]
    fn a_new_account_needs_a_valid_label_and_an_idle_launcher() {
        let mut app = ready(false);
        for bad in ["", "..", "a/b", "default"] {
            let _ = app.update(Message::NewAccountName(bad.into()));
            assert!(!app.can_add_account(), "{bad:?}");
            let _ = app.update(Message::AddAccount);
            assert_eq!(app.account_name, "default");
        }
        let _ = app.update(Message::NewAccountName("Alt".into()));
        assert_eq!(app.new_account, "alt", "labels are lower case");
        let _ = app.update(Message::AddAccount);
        assert_eq!(app.account_name, "alt");
        assert_eq!(labels(&app), ["alt", "default"]);
        assert_eq!(entry(&app, "alt").summary, None, "nothing is assumed about a label the launcher has not described");
        assert!(app.new_account.is_empty());
        // Busy now (refreshing): no further switch.
        let _ = app.update(Message::AccountSelected("default".into()));
        assert_eq!(app.account_name, "alt");
        // The launcher's answer fills the new entry in.
        feed(&mut app, events([Event::Account(named("alt", false))]));
        assert_eq!(entry(&app, "alt").summary, Some(named("alt", false)));
    }

    #[test]
    fn account_labels_from_the_launcher_are_checked_before_they_are_shown() {
        let mut app = ready(true);
        let _ = app.update(Message::Refresh);
        feed(
            &mut app,
            events([
                Event::Accounts {
                    default: "../x".into(),
                    accounts: vec![named("ok", false), named("\u{1b}[31mbad", false), named("ok", true)],
                },
                Event::Account(named("../../etc", true)),
            ]),
        );
        assert_eq!(labels(&app), ["default", "ok"]);
        assert_eq!(app.default_account, None);
        // The free text of a summary is made printable and bounded.
        let hostile = Account {
            player: Some("Al\u{202e}ex\u{0}".into()),
            store: format!("keyring\n{}", "x".repeat(1000)),
            ..named("ok", true)
        };
        feed(&mut app, events([Event::Account(hostile)]));
        let kept = entry(&app, "ok").summary.as_ref().unwrap();
        assert_eq!(kept.player.as_deref(), Some("Alex"));
        assert!(kept.store.starts_with("keyringxxx") && kept.store.chars().count() == 241, "{}", kept.store.len());
    }

    #[test]
    fn signing_in_and_out_is_per_account_and_does_not_change_which_one_is_in_use() {
        let mut app = ready(true);
        feed_idle_accounts(&mut app);
        let alt = entry(&app, "alt").clone();
        assert!(app.can_sign_in(&alt) && !app.can_sign_out(&alt));
        let default = entry(&app, "default").clone();
        assert!(!app.can_sign_in(&default) && app.can_sign_out(&default), "already signed in");

        let _ = app.update(Message::SignIn { account: "alt".into(), device_code: false });
        assert_eq!(app.job_kind(), Some(JobKind::SignIn));
        assert_eq!(app.job.as_ref().unwrap().subject, "alt", "the prompt can say whose sign-in it is");
        assert!(!app.can_sign_in(&alt) && !app.can_sign_out(&default), "one launcher command at a time");
        feed(
            &mut app,
            events([
                Event::Account(Account { player: Some("Steve".into()), ..named("alt", true) }),
                Event::Accounts {
                    default: "default".into(),
                    accounts: vec![
                        Account { player: Some("Steve".into()), ..named("alt", true) },
                        named("default", true),
                    ],
                },
            ]),
        );
        finish(&mut app, 0);
        assert_eq!(app.status, "Signed in (alt).");
        assert_eq!(app.account_name, "default", "still the account in use");
        assert_eq!(app.account.as_ref().unwrap().player.as_deref(), Some("Alex"), "and still its own state");
        assert_eq!(entry(&app, "alt").summary.as_ref().unwrap().player.as_deref(), Some("Steve"));

        let _ = app.update(Message::SignOut { account: "alt".into() });
        assert_eq!(app.job_kind(), Some(JobKind::SignOut));
        feed(&mut app, events([Event::Account(named("alt", false))]));
        finish(&mut app, 0);
        assert_eq!(app.status, "Signed out (alt).");
        assert!(!entry(&app, "alt").summary.as_ref().unwrap().signed_in);
        assert!(app.signed_in(), "the account in use was not touched");

        // Labels that could not be a launcher argument never become one.
        for bad in ["", "--device", "a b", "../x"] {
            let _ = app.update(Message::SignIn { account: bad.into(), device_code: false });
            let _ = app.update(Message::SignOut { account: bad.into() });
            let _ = app.update(Message::MakeDefaultAccount(bad.into()));
            let _ = app.update(Message::AccountSelected(bad.into()));
            assert!(!app.busy(), "{bad:?}");
        }
    }

    #[test]
    fn sign_in_needs_an_oauth_client_and_what_the_launcher_said() {
        let mut app = ready(false);
        let no_client = AccountEntry {
            label: "default".into(),
            summary: Some(Account { client_id_configured: false, ..account(false) }),
        };
        assert!(!app.can_sign_in(&no_client), "nothing to sign in with");
        let unknown = AccountEntry { label: "new".into(), summary: None };
        assert!(!app.can_sign_in(&unknown) && !app.can_sign_out(&unknown), "nothing is offered on a guess");
        let known = AccountEntry { label: "default".into(), summary: Some(account(false)) };
        assert!(app.can_sign_in(&known));
        let _ = app.update(Message::Play); // not signed in, no demo: nothing starts
        let _ = app.update(Message::DemoToggled(true));
        let _ = app.update(Message::Play);
        assert!(!app.can_sign_in(&known), "not while a game is being launched");
    }

    #[test]
    fn making_an_account_the_default_names_that_account() {
        let mut app = ready(true);
        feed_idle_accounts(&mut app);
        let _ = app.update(Message::MakeDefaultAccount("alt".into()));
        assert_eq!(app.job_kind(), Some(JobKind::Refresh));
        assert_eq!(app.job.as_ref().unwrap().subject, "alt");
        feed(
            &mut app,
            events([Event::Accounts {
                default: "alt".into(),
                accounts: vec![named("alt", false), named("default", true)],
            }]),
        );
        finish(&mut app, 0);
        assert_eq!(app.default_account.as_deref(), Some("alt"));
        assert_eq!(
            app.account_name, "default",
            "the default is the launcher's choice for the command line, not this window's"
        );
    }

    #[test]
    fn the_account_list_is_the_launchers_word_on_the_account_in_use_too() {
        let mut app = ready(true);
        assert!(app.signed_in());
        // `account default` reports the list only. Meanwhile the token was removed elsewhere.
        let _ = app.update(Message::MakeDefaultAccount("default".into()));
        feed(
            &mut app,
            events([Event::Accounts {
                default: "default".into(),
                accounts: vec![named("alt", true), named("default", false)],
            }]),
        );
        finish(&mut app, 0);
        assert!(!app.signed_in(), "the launch control goes by the same report as the account's card");
        assert_eq!(app.play_blocker(), Some("Sign in, or switch on demo mode."));
        // A list that does not have the account in use says nothing about it.
        let _ = app.update(Message::Refresh);
        feed(
            &mut app,
            events([
                Event::Account(named("default", true)),
                Event::Accounts { default: "alt".into(), accounts: vec![named("alt", true)] },
            ]),
        );
        finish(&mut app, 0);
        assert!(app.signed_in());
    }

    #[test]
    fn a_flood_of_accounts_is_cut_off_but_never_the_one_in_use() {
        let (mut app, _task) = App::boot(PathBuf::from("/nonexistent/bastion-mc"), "zz-last".into());
        let many: Vec<Account> =
            (0..500).map(|i| named(&format!("acct-{i:03}"), false)).chain([named("zz-last", true)]).collect();
        feed(&mut app, events([Event::Accounts { default: "acct-000".into(), accounts: many }]));
        assert_eq!(app.accounts.len(), MAX_ACCOUNTS);
        assert_eq!(app.accounts_hidden, 501 - MAX_ACCOUNTS);
        assert!(entry(&app, "zz-last").summary.as_ref().unwrap().signed_in);
        assert!(app.accounts.windows(2).all(|pair| pair[0].label < pair[1].label), "still sorted");

        let mut app = ready(true);
        let _ = app.update(Message::Refresh);
        feed(
            &mut app,
            events([Event::Instances { instances: (0..1000).map(|i| instance(&format!("i{i}"), None)).collect() }]),
        );
        assert_eq!((app.instances.len(), app.instances_hidden), (MAX_INSTANCES, 1000 - MAX_INSTANCES));
    }
}

#[cfg(test)]
mod emergency_tests {
    use super::{testing::*, *};

    const REMOVED: &str = "removed the refresh token of account default from the kernel keyring";
    const NOT_REMOVED: &str =
        "COULD NOT delete the vault file /home/u/.local/share/bastion-mc/vault/alt.age: Permission denied";

    fn notice(app: &App) -> &Notice {
        app.notice.as_ref().expect("a notice")
    }

    #[test]
    fn scram_needs_the_question_answered_and_runs_once() {
        let mut app = ready(true);
        // Without the question having been asked, a confirmation does nothing.
        let _ = app.update(Message::ConfirmScram);
        assert!(!app.emergency.active());
        let _ = app.update(Message::AskScram);
        assert!(app.emergency.confirming);
        let _ = app.update(Message::CancelScram);
        let _ = app.update(Message::ConfirmScram);
        assert!(!app.emergency.active(), "cancelled");
        confirm_scram(&mut app);
        assert_eq!(app.emergency.state, ScramState::Running);
        assert!(!app.emergency.confirming);
        // While it runs it cannot be asked for again.
        let _ = app.update(Message::AskScram);
        assert!(!app.emergency.confirming);
        let run = app.emergency.run;
        let _ = app.update(Message::ConfirmScram);
        assert_eq!(app.emergency.run, run, "no second run");

        scram_lines(&mut app, &["no launch was in progress", REMOVED, ""]);
        assert_eq!(app.emergency.lines.len(), 3, "the report is shown as it arrives");
        assert!(app.emergency.outcome.is_none() && app.emergency.active());
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::Exited(0))]);
        assert!(!app.emergency.active());
        assert_eq!(app.emergency.outcome, Some(ScramOutcome::Complete));
        assert_eq!(app.emergency.failures, 0);
        assert!(app.account.is_none() && !app.signed_in(), "the old sign-in state is not shown any more");
        assert!(app.accounts.iter().all(|entry| entry.summary.is_none()), "on no card either");
        assert_eq!(notice(&app).severity, Severity::Warning);
        assert!(notice(&app).text.contains("Nothing is revoked at Microsoft"));
        assert_eq!(app.job_kind(), Some(JobKind::Refresh), "the launcher is asked for the new state");
        assert!(app.console.lines().any(|line| line.text == format!("scram: {REMOVED}")));
    }

    #[test]
    fn a_scram_the_launcher_calls_incomplete_is_never_reported_as_done() {
        let mut app = ready(true);
        confirm_scram(&mut app);
        scram_lines(&mut app, &["no launch was in progress", REMOVED, NOT_REMOVED, "", "This removed the copies…"]);
        // The launcher's contract: exit status 1 when something could not be removed.
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::Exited(1))]);

        let Some(ScramOutcome::Incomplete(why)) = &app.emergency.outcome else {
            panic!("reported as {:?}", app.emergency.outcome)
        };
        assert!(why.contains("status 1"), "{why}");
        assert_eq!(app.emergency.failures, 1);
        let failed: Vec<&str> = app.emergency.failed_lines().map(|line| line.text.as_str()).collect();
        assert_eq!(failed, [NOT_REMOVED], "what is still there is singled out");
        assert_eq!(notice(&app).severity, Severity::Error);
        let said = notice(&app).text.to_lowercase();
        assert!(said.contains("scram incomplete"), "{said}");
        for claim in ["done", "complete", "removed"] {
            assert!(
                !said.replace("incomplete", "").contains(claim),
                "an incomplete scram must not read as finished: {said:?}"
            );
        }
        let errors: Vec<&str> =
            app.console.lines().filter(|line| line.level == Level::Error).map(|line| line.text.as_str()).collect();
        assert!(errors.iter().any(|line| line.contains(NOT_REMOVED)), "{errors:?}");
        assert!(errors.iter().any(|line| line.contains("INCOMPLETE")), "{errors:?}");
        assert_eq!(app.job_kind(), Some(JobKind::Refresh), "the cards then show what is in fact still stored");
    }

    #[test]
    fn nothing_but_a_clean_exit_with_a_clean_report_counts_as_complete() {
        let incomplete = |lines: &[&str], end: ScramEnd| {
            let mut app = ready(true);
            confirm_scram(&mut app);
            scram_lines(&mut app, lines);
            scram(&mut app, [ScramUpdate::Finished(end.clone())]);
            match app.emergency.outcome.clone() {
                Some(ScramOutcome::Incomplete(why)) => {
                    assert_eq!(notice(&app).severity, Severity::Error, "{end:?}");
                    why
                }
                other => panic!("{end:?} with {lines:?} was reported as {other:?}"),
            }
        };
        // Status 0 is not believed over a line that says something is still there…
        assert!(incomplete(&[REMOVED, NOT_REMOVED], ScramEnd::Exited(0)).contains("could not"));
        assert!(
            incomplete(&["  COULD NOT look for account x in the kernel keyring: EPERM"], ScramEnd::Exited(0))
                .contains("could not")
        );
        // …nor without any report at all.
        assert!(incomplete(&[], ScramEnd::Exited(0)).contains("reported nothing"));
        assert!(incomplete(&["", "  "], ScramEnd::Exited(0)).contains("reported nothing"));
        // Any other status, with or without an explanation.
        assert!(incomplete(&[REMOVED], ScramEnd::Exited(1)).contains("status 1"));
        assert!(incomplete(&[REMOVED], ScramEnd::Exited(101)).contains("status 101"));
        assert!(incomplete(&[REMOVED], ScramEnd::Killed).contains("killed"));
        // The wait limit: the launcher is still at it, and that is what is said.
        let waited = incomplete(&[REMOVED], ScramEnd::StillRunning);
        assert!(waited.contains("left running") && waited.contains("30 minutes"), "{waited}");

        // The launcher's own error message is a failure line like any other.
        let mut app = ready(true);
        confirm_scram(&mut app);
        scram(
            &mut app,
            [ScramUpdate::Error("cannot create /run/user/1000".into()), ScramUpdate::Finished(ScramEnd::Exited(3))],
        );
        assert!(matches!(app.emergency.outcome, Some(ScramOutcome::Incomplete(_))));
        assert!(app.emergency.failed_lines().any(|line| line.text.contains("cannot create")));
    }

    #[test]
    fn a_scram_that_could_not_be_started_says_that_nothing_was_removed() {
        let mut app = ready(true);
        confirm_scram(&mut app);
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::FailedToStart("No such file or directory".into()))]);
        let Some(ScramOutcome::NotRun(why)) = &app.emergency.outcome else { panic!("{:?}", app.emergency.outcome) };
        assert!(why.contains("/nonexistent/bastion-mc") && why.contains("No such file"), "{why}");
        assert_eq!(notice(&app).severity, Severity::Error);
        assert!(notice(&app).text.contains("Nothing was removed"));
        assert!(app.signed_in(), "nothing changed, so what is known about the accounts stands");
        assert!(!app.busy());
        // It can be tried again.
        confirm_scram(&mut app);
        assert_eq!(app.emergency.state, ScramState::Running);
        assert!(app.emergency.outcome.is_none() && app.emergency.lines.is_empty(), "the new run starts clean");
    }

    #[test]
    fn scram_works_while_a_game_is_running() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
        confirm_scram(&mut app);
        assert_eq!(app.emergency.state, ScramState::Running, "a running game does not block or delay it");
        assert_eq!(app.job_kind(), Some(JobKind::Launch), "the game's job is left for the launcher to end");
        scram_lines(
            &mut app,
            &["killed the running launcher of instance vanilla (pid 4242): its session token is gone with it"],
        );
        // The launch ends because the scram killed it: that is not a second alarm.
        feed(&mut app, [Update::Finished(Outcome::Killed)]);
        assert!(app.notice.is_none(), "{:?}", app.notice);
        assert_eq!(app.job_kind(), Some(JobKind::Refresh));
        scram_lines(&mut app, &[REMOVED]);
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::Exited(0))]);
        assert_eq!(app.emergency.outcome, Some(ScramOutcome::Complete));
        // The refresh that was already running predates the removal: another one follows.
        assert!(app.refresh_wanted);
        finish(&mut app, 0);
        assert_eq!(app.job_kind(), Some(JobKind::Refresh));
        assert!(!app.refresh_wanted);
    }

    #[test]
    fn a_sign_in_in_progress_is_ended_before_the_scram_starts() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        feed(&mut app, [Update::Started(stop_handle(sender))]);
        feed(
            &mut app,
            events([Event::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://www.microsoft.com/link".into(),
                expires_in: 900,
            }]),
        );

        confirm_scram(&mut app);
        assert_eq!(
            app.emergency.state,
            ScramState::StoppingSignIn,
            "not started yet: the sign-in could still store a token"
        );
        assert_eq!(receiver.try_recv(), Ok(SIGN_IN_ABORT_GRACE), "the sign-in was told to stop, and given little time");
        assert!(app.job.as_ref().unwrap().stopping);
        assert_eq!(app.tick_interval(), Some(CLOCK_INTERVAL), "the wait is shown as it goes on");

        // The launcher that was signing in is gone: now the scram runs.
        feed(&mut app, events([Event::Error { message: "cancelled".into(), code: 130 }]));
        finish(&mut app, 130);
        assert_eq!(app.emergency.state, ScramState::Running);
        assert!(app.login.is_none() && app.notice.is_none());
        scram_lines(&mut app, &[REMOVED]);
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::Exited(0))]);
        assert_eq!(app.emergency.outcome, Some(ScramOutcome::Complete));
        // The deadline that was armed for the wait finds nothing left to do.
        let run = app.emergency.run;
        let _ = app.update(Message::ScramStartDeadline(run));
        assert!(!app.emergency.active());
    }

    #[test]
    fn a_sign_in_that_was_asked_for_before_its_launcher_started_is_stopped_the_same_way() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: false });
        confirm_scram(&mut app); // the worker has not reported `Started` yet
        assert_eq!(app.emergency.state, ScramState::StoppingSignIn);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        feed(&mut app, [Update::Started(stop_handle(sender))]);
        assert_eq!(receiver.try_recv(), Ok(SIGN_IN_ABORT_GRACE));
        // Killed by the worker when the grace ran out: no separate alarm about that.
        feed(&mut app, [Update::Finished(Outcome::Killed)]);
        assert_eq!(app.emergency.state, ScramState::Running);
        assert!(app.notice.is_none());
    }

    #[test]
    fn the_scram_does_not_wait_forever_for_a_sign_in_to_end() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
        confirm_scram(&mut app);
        let run = app.emergency.run;
        // A deadline that belongs to an earlier run changes nothing.
        let _ = app.update(Message::ScramStartDeadline(run - 1));
        assert_eq!(app.emergency.state, ScramState::StoppingSignIn);
        let _ = app.update(Message::ScramStartDeadline(run));
        assert_eq!(app.emergency.state, ScramState::Running, "credentials are removed regardless");
        // …but the result cannot be called complete: that sign-in may still store a token.
        scram_lines(&mut app, &[REMOVED]);
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::Exited(0))]);
        assert!(matches!(app.emergency.outcome, Some(ScramOutcome::Incomplete(_))), "{:?}", app.emergency.outcome);
        assert!(app.emergency.failed_lines().any(|line| line.text.contains("sign-in in progress did not end")));
        // When the sign-in does end later, no second scram is started behind the user's back.
        assert_eq!(app.job_kind(), Some(JobKind::SignIn));
        finish(&mut app, 0);
        assert!(!app.emergency.active());
        assert_eq!(app.job_kind(), Some(JobKind::Refresh), "what is stored now is asked for");
    }

    #[test]
    fn a_window_asked_to_close_stays_until_a_confirmed_scram_has_started() {
        // Confirmed while a sign-in was pending: for a few seconds no launcher is removing
        // anything yet. Closing now must not turn the confirmation into nothing.
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
        confirm_scram(&mut app);
        assert!(app.emergency.awaiting_start());
        assert!(!closes(&mut app), "the scram that was confirmed has not started");
        assert!(app.status.contains("as soon as the scram has started"), "{}", app.status);
        // The sign-in ends: the scram is started, not dropped, and the window still waits.
        let id = app.job.as_ref().unwrap().id;
        let follow_up = app.update(Message::Backend(id, Update::Finished(Outcome::Exited(130))));
        assert!(follow_up.units() >= 1, "something was started");
        assert_eq!(app.emergency.state, ScramState::Running);
        assert!(app.closing && app.emergency.awaiting_start());
        // The launcher doing it is running: it needs no window from here on.
        let run = app.emergency.run;
        assert!(exits(app.update(Message::Scram(run, ScramUpdate::Started))));

        // A scram whose launcher is running is not waited for.
        let mut app = ready(true);
        confirm_scram(&mut app);
        assert!(app.emergency.awaiting_start(), "confirmed a moment ago: no launcher yet");
        scram(&mut app, [ScramUpdate::Started]);
        assert!(!app.emergency.awaiting_start());
        assert!(closes(&mut app));

        // Asked twice, the window goes: that is the user's word against their own.
        let mut app = ready(true);
        confirm_scram(&mut app);
        assert!(!closes(&mut app));
        assert!(closes(&mut app));
    }

    #[test]
    fn a_closing_window_stays_to_say_that_the_scram_could_not_be_started() {
        let mut app = ready(true);
        confirm_scram(&mut app);
        assert!(!closes(&mut app));
        let run = app.emergency.run;
        let end = ScramUpdate::Finished(ScramEnd::FailedToStart("No such file or directory".into()));
        assert!(!exits(app.update(Message::Scram(run, end))), "nothing was removed: that has to be read");
        assert!(!app.closing);
        assert!(matches!(app.emergency.outcome, Some(ScramOutcome::NotRun(_))));
        assert!(notice(&app).text.contains("Nothing was removed"));
        assert!(closes(&mut app), "and then the window closes when asked");

        // The same with a game running: the game is stopped as asked, the window stays.
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
        confirm_scram(&mut app);
        assert!(!closes(&mut app));
        assert!(app.job.as_ref().unwrap().stopping);
        let run = app.emergency.run;
        let end = ScramUpdate::Finished(ScramEnd::FailedToStart("No such file or directory".into()));
        assert!(!exits(app.update(Message::Scram(run, end))));
        assert!(!app.launch_status.contains("closing"), "{}", app.launch_status);
        let id = app.job.as_ref().unwrap().id;
        let after_the_game = app.update(Message::Backend(id, Update::Finished(Outcome::Exited(130))));
        assert!(!app.closing && notice(&app).text.contains("Nothing was removed"));
        // What follows the game is the usual refresh, not the end of the program.
        assert_eq!(app.job_kind(), Some(JobKind::Refresh));
        drop(after_the_game);
    }

    #[test]
    fn a_window_closing_for_a_game_and_a_scram_waits_for_both() {
        let play = || {
            let mut app = ready(true);
            let _ = app.update(Message::Play);
            feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
            confirm_scram(&mut app);
            assert!(!closes(&mut app));
            app
        };
        // The scram starts first, then the game is down.
        let mut app = play();
        let run = app.emergency.run;
        assert!(!exits(app.update(Message::Scram(run, ScramUpdate::Started))), "the game is still being stopped");
        let id = app.job.as_ref().unwrap().id;
        assert!(exits(app.update(Message::Backend(id, Update::Finished(Outcome::Killed)))));
        // The game is down first, then the scram starts.
        let mut app = play();
        let id = app.job.as_ref().unwrap().id;
        let _ = app.update(Message::Backend(id, Update::Finished(Outcome::Killed)));
        assert!(app.closing && app.emergency.awaiting_start(), "still here: no launcher is removing anything yet");
        let run = app.emergency.run;
        assert!(exits(app.update(Message::Scram(run, ScramUpdate::Started))));
    }

    #[test]
    fn nothing_that_stores_a_token_can_be_started_while_a_scram_runs() {
        let mut app = ready(false);
        let _ = app.update(Message::DemoToggled(true));
        confirm_scram(&mut app);
        let entry = app.accounts[0].clone();
        assert!(!app.can_sign_in(&entry));
        let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
        assert!(!app.busy(), "no sign-in");
        assert_eq!(app.play_blocker(), Some("A scram is in progress."));
        let _ = app.update(Message::Play);
        assert!(!app.busy(), "no launch");
        scram_lines(&mut app, &[REMOVED]);
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::Exited(0))]);
        finish(&mut app, 0); // the refresh
        assert!(app.can_play(), "afterwards everything is available again");
    }

    #[test]
    fn reports_of_an_earlier_scram_are_not_taken_for_the_current_one() {
        let mut app = ready(true);
        confirm_scram(&mut app);
        let first = app.emergency.run;
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::FailedToStart("x".into()))]);
        confirm_scram(&mut app);
        let _ = app.update(Message::Scram(first, ScramUpdate::Line(REMOVED.into())));
        let _ = app.update(Message::Scram(first, ScramUpdate::Finished(ScramEnd::Exited(0))));
        assert!(app.emergency.lines.is_empty() && app.emergency.active(), "the stale run changed nothing");
        // Nor is anything accepted once a run is over.
        scram_lines(&mut app, &[REMOVED]);
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::Exited(0))]);
        scram_lines(&mut app, &[NOT_REMOVED]);
        assert_eq!(app.emergency.outcome, Some(ScramOutcome::Complete));
        assert_eq!(app.emergency.lines.len(), 1);
    }

    #[test]
    fn a_long_report_is_bounded_without_losing_what_could_not_be_removed() {
        let mut app = ready(true);
        confirm_scram(&mut app);
        for i in 0..MAX_SCRAM_LINES + 50 {
            scram_lines(&mut app, &[&format!("deleted the vault file {i}.age")]);
        }
        scram_lines(&mut app, &[NOT_REMOVED]);
        assert_eq!(app.emergency.omitted, 50);
        assert_eq!(app.emergency.lines.len(), MAX_SCRAM_LINES + 1);
        assert_eq!(app.emergency.failed_lines().count(), 1, "the failure is kept although the report was full");
        scram(&mut app, [ScramUpdate::Finished(ScramEnd::Exited(0))]);
        assert!(matches!(app.emergency.outcome, Some(ScramOutcome::Incomplete(_))));
        // Even failures have a bound; the count stays right beyond it.
        let mut app = ready(true);
        confirm_scram(&mut app);
        for _ in 0..3 * MAX_SCRAM_LINES {
            scram_lines(&mut app, &[NOT_REMOVED]);
        }
        assert_eq!(app.emergency.failures, 3 * MAX_SCRAM_LINES);
        assert!(app.emergency.lines.len() <= 2 * MAX_SCRAM_LINES);
    }

    #[test]
    fn only_runbook_links_are_opened_from_the_emergency_panel() {
        let mut app = ready(true);
        for bad in ["https://account.live.com/", "https://evil.example/", "file:///etc/passwd", ""] {
            app.notice = None;
            let _ = app.update(Message::OpenRunbookLink(bad.into()));
            assert!(app.notice.as_ref().is_some_and(|n| n.text.contains("not one of the runbook's links")), "{bad:?}");
        }
        // A sign-in link is not a runbook link and the other way round: two separate lists.
        assert!(!runbook::is_runbook_link("https://login.microsoftonline.com/x"));
        assert!(!is_sign_in_link("https://help.minecraft.net/hc/en-us/requests/new"));
        for (link, _) in runbook::LINKS {
            app.notice = None;
            let _ = app.update(Message::OpenLink((*link).to_string()));
            let refused = app.notice.as_ref().is_some_and(|n| n.text.contains("not a Microsoft sign-in page"));
            assert_eq!(refused, !is_sign_in_link(link), "{link}: the sign-in check decides for sign-in links");
        }
    }

    #[test]
    fn a_new_launch_starts_without_the_previous_launchs_protections() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        feed(
            &mut app,
            events([Event::Protection {
                items: vec![Protection {
                    aspect: "network".into(),
                    shield: bastion_mc_proto::Shield::On,
                    detail: "filtered".into(),
                }],
            }]),
        );
        assert_eq!(app.protection.len(), 1);
        finish(&mut app, 0);
        assert_eq!(app.protection.len(), 1, "still shown after the game ended");
        if app.busy() {
            finish(&mut app, 0); // the status refresh that follows a launch
        }
        let _ = app.update(Message::Play);
        assert!(app.protection.is_empty());
    }

    #[test]
    fn only_lines_that_start_with_the_launchers_marker_count_as_failures() {
        assert!(is_failure_line(NOT_REMOVED));
        assert!(is_failure_line("   COULD NOT x"));
        assert!(!is_failure_line(REMOVED));
        assert!(!is_failure_line("the game could not be reached; COULD NOT is not at the start"));
        assert!(!is_failure_line(""));
    }
}
