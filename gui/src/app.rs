//! Application state and the logic that changes it.
//!
//! This is the whole "controller": [`App::update`] is the only place state changes, and it
//! never blocks. Anything slow is a [`Request`] handed to the backend worker; its
//! [`Update`]s come back as [`Message::Backend`] and are folded into the state here. The
//! view (`crate::view`) only reads.
//!
//! One launcher command runs at a time ([`App::job`]). That is what makes the Play button's
//! behaviour trivial to get right: it is enabled exactly when there is no job.

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
    backend::{self, Handle, Outcome, Request, Update},
    console::{Console, Level},
};

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
    handle: Option<Handle>,
    /// Where a launch is; `None` until the launcher says so.
    pub phase: Option<Phase>,
    /// A stop was requested; the command is winding down.
    pub stopping: bool,
    /// The launcher explained its own failure, so its exit status needs no second notice.
    reported_error: bool,
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

    pub fn caption(&self) -> String {
        let percent = (self.fraction() * 100.0).floor() as u32;
        match self.unit {
            Unit::Bytes => format!(
                "Downloading {}: {:.1} of {:.1} MiB ({percent}%)",
                self.label,
                self.done as f64 / 1048576.0,
                self.total as f64 / 1048576.0
            ),
            Unit::Files => format!("Downloading {}: {} of {} files ({percent}%)", self.label, self.done, self.total),
        }
    }
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

/// An entry of the instance drop-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceChoice(pub Instance);

impl InstanceChoice {
    pub fn launchable(&self) -> bool {
        self.0.error.is_none()
    }
}

impl fmt::Display for InstanceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.0.error, &self.0.minecraft) {
            (Some(_), _) => write!(f, "{} (cannot be launched)", self.0.id),
            (None, Some(version)) => write!(f, "{}  -  Minecraft {version}, {}", self.0.id, self.0.kind),
            (None, None) => write!(f, "{}", self.0.id),
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

#[derive(Debug, Default)]
pub struct NewInstance {
    pub open: bool,
    pub id: String,
    pub versions: Vec<VersionChoice>,
    pub version: Option<VersionChoice>,
    pub snapshots: bool,
}

/// The emergency panel.
#[derive(Debug, Default)]
pub struct Emergency {
    pub open: bool,
    /// "Scram" was pressed once and waits for the confirmation.
    pub confirming: bool,
    pub running: bool,
    /// What the last scram reported.
    pub report: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Something from the backend worker, tagged with the job it belongs to.
    Backend(u64, Update),
    Refresh,
    SignIn {
        device_code: bool,
    },
    SignOut,
    /// Switch to another local account (an existing label, or a new one to sign in to).
    AccountSelected(String),
    NewAccountName(String),
    AddAccount,
    /// Make the current account the launcher's default.
    MakeDefaultAccount,
    /// Stop the game, or abandon a sign-in.
    Stop,
    InstanceSelected(InstanceChoice),
    DemoToggled(bool),
    Play,
    ToggleConsole,
    ClearConsole,
    CopyConsole,
    Copy(String),
    OpenLink(String),
    LinkOpened(Result<(), String>),
    /// Show or hide the emergency panel (runbook, scram).
    ToggleEmergency,
    /// Open one of the runbook's links (only those; see `runbook::LINKS`).
    OpenRunbookLink(String),
    /// First press of "Scram": ask. Second (`ConfirmScram`): do it.
    AskScram,
    CancelScram,
    ConfirmScram,
    ScramFinished(Result<Vec<String>, String>),
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
    pub account_name: String,
    /// `None` until the launcher has reported once.
    pub account: Option<Account>,
    /// Every local account label the launcher knows, plus the current one.
    pub accounts: Vec<String>,
    /// The launcher's default account, once it has said.
    pub default_account: Option<String>,
    /// Text of the "new account" field.
    pub new_account: String,
    pub login: Option<LoginPrompt>,
    pub instances: Vec<InstanceChoice>,
    pub selected: Option<InstanceChoice>,
    pub demo: bool,
    pub job: Option<Job>,
    next_job: u64,
    pub progress: Option<Progress>,
    /// One line about the launch in progress, or how the last one ended (shown by Play).
    pub launch_status: String,
    /// One line about everything else: sign-in, instance management (shown at the bottom).
    pub status: String,
    pub notice: Option<Notice>,
    pub console: Console,
    pub new_instance: NewInstance,
    /// What the launch in progress (or the last one) runs with.
    pub protection: Vec<Protection>,
    pub emergency: Emergency,
    pub now: Instant,
    /// The window was asked to close while a game was running: stop it, then exit.
    closing: bool,
    /// Instance to select as soon as it shows up in the list (it was just created).
    select_when_listed: Option<String>,
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

impl App {
    pub fn new(launcher: PathBuf, account_name: String) -> Self {
        App {
            launcher,
            account_name,
            account: None,
            accounts: Vec::new(),
            default_account: None,
            new_account: String::new(),
            login: None,
            instances: Vec::new(),
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
            now: Instant::now(),
            closing: false,
            select_when_listed: None,
        }
    }

    /// The state the window opens with, and the first thing to do: ask the launcher who is
    /// signed in and which instances exist.
    pub fn boot(launcher: PathBuf, account_name: String) -> (Self, Task<Message>) {
        let mut app = App::new(launcher, account_name);
        let task = app.start(JobKind::Refresh, app.status_request());
        (app, task)
    }

    fn status_request(&self) -> Request {
        Request::Status { account: self.account_name.clone() }
    }

    pub fn can_add_account(&self) -> bool {
        !self.busy() && is_valid_id(&self.new_account) && self.new_account != self.account_name
    }

    /// Show another account. Nothing is signed in or out by this; each label has its own
    /// stored token, and the launcher is asked afresh about the one now shown.
    fn switch_account(&mut self, name: String) -> Task<Message> {
        if self.busy() || !is_valid_id(&name) || name == self.account_name {
            return Task::none();
        }
        self.account_name = name;
        self.account = None;
        self.login = None;
        self.notice = None;
        self.remember_current_account();
        self.start(JobKind::Refresh, self.status_request())
    }

    fn remember_current_account(&mut self) {
        if !self.account_name.is_empty() && !self.accounts.contains(&self.account_name) {
            self.accounts.push(self.account_name.clone());
            self.accounts.sort();
        }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn job_kind(&self) -> Option<JobKind> {
        self.job.as_ref().map(|job| job.kind)
    }

    pub fn signed_in(&self) -> bool {
        self.account.as_ref().is_some_and(|a| a.signed_in)
    }

    /// Why Play cannot be pressed right now, if it cannot.
    pub fn play_blocker(&self) -> Option<&'static str> {
        match &self.selected {
            None if self.instances.is_empty() => return Some("Create an instance first."),
            None => return Some("Choose an instance."),
            Some(choice) if !choice.launchable() => return Some("This instance cannot be launched (see below)."),
            Some(_) => {}
        }
        if !self.demo && !self.signed_in() {
            return Some("Sign in, or tick demo mode.");
        }
        None
    }

    pub fn can_play(&self) -> bool {
        !self.busy() && self.play_blocker().is_none()
    }

    pub fn can_create_instance(&self) -> bool {
        !self.busy()
            && self.new_instance.version.is_some()
            && is_valid_id(&self.new_instance.id)
            && !self.instances.iter().any(|i| i.0.id == self.new_instance.id)
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
    fn start(&mut self, kind: JobKind, request: Request) -> Task<Message> {
        let id = self.next_job;
        self.next_job += 1;
        self.job = Some(Job { id, kind, handle: None, phase: None, stopping: false, reported_error: false });
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
    fn request(&mut self, kind: JobKind, request: Request) -> Task<Message> {
        if self.busy() { Task::none() } else { self.start(kind, request) }
    }

    fn stop_job(&mut self) {
        let Some(job) = &mut self.job else { return };
        job.stopping = true;
        if let Some(handle) = &job.handle {
            handle.stop();
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
            Message::Refresh => return self.request(JobKind::Refresh, self.status_request()),
            Message::SignIn { device_code } => {
                self.notice = None;
                let request = Request::SignIn { account: self.account_name.clone(), device_code };
                return self.request(JobKind::SignIn, request);
            }
            Message::SignOut => {
                let request = Request::SignOut { account: self.account_name.clone() };
                return self.request(JobKind::SignOut, request);
            }
            Message::AccountSelected(name) => return self.switch_account(name),
            Message::NewAccountName(text) => self.new_account = text.to_ascii_lowercase(),
            Message::AddAccount => {
                if self.can_add_account() {
                    let name = std::mem::take(&mut self.new_account);
                    return self.switch_account(name);
                }
            }
            Message::MakeDefaultAccount => {
                if !self.account_name.is_empty() {
                    let request = Request::DefaultAccount { account: self.account_name.clone() };
                    return self.request(JobKind::Refresh, request);
                }
            }
            Message::Stop => self.stop_job(),
            Message::InstanceSelected(choice) => self.selected = Some(choice),
            Message::DemoToggled(on) => self.demo = on,
            Message::Play => {
                if !self.can_play() {
                    return Task::none();
                }
                let Some(choice) = &self.selected else { return Task::none() };
                let request = Request::Launch {
                    instance: choice.0.id.clone(),
                    account: self.account_name.clone(),
                    demo: self.demo,
                };
                self.notice = None;
                self.progress = None;
                self.protection.clear();
                self.status.clear();
                self.console.new_session();
                self.launch_status = "Starting…".into();
                return self.start(JobKind::Launch, request);
            }
            Message::ToggleConsole => self.console.open = !self.console.open,
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
            Message::ToggleEmergency => {
                self.emergency.open = !self.emergency.open;
                self.emergency.confirming = false;
            }
            Message::OpenRunbookLink(url) => {
                if runbook::is_runbook_link(&url) {
                    return Task::perform(open_link(url), Message::LinkOpened);
                }
                self.notify(Severity::Warning, "That address is not one of the runbook's links; it was not opened.");
            }
            Message::AskScram => self.emergency.confirming = !self.emergency.running,
            Message::CancelScram => self.emergency.confirming = false,
            Message::ConfirmScram => {
                // Only as the answer to the question, and only once at a time.
                if self.emergency.confirming && !self.emergency.running {
                    self.emergency.confirming = false;
                    self.emergency.running = true;
                    self.emergency.report.clear();
                    return Task::perform(backend::scram(self.launcher.clone()), Message::ScramFinished);
                }
            }
            Message::ScramFinished(result) => {
                self.emergency.running = false;
                match result {
                    Ok(lines) => {
                        for line in &lines {
                            self.console.push(Source::Launcher, line.clone());
                        }
                        self.emergency.report = lines;
                        self.account = None;
                        self.login = None;
                        self.notify(
                            Severity::Warning,
                            "Scram done: stored sign-ins on this machine are removed. Nothing is revoked at Microsoft \
                             yet — follow the runbook, from another device if this one may be compromised.",
                        );
                        // Show the new state as soon as nothing else is running.
                        if !self.busy() {
                            return self.start(JobKind::Refresh, self.status_request());
                        }
                    }
                    Err(why) => self
                        .notify(Severity::Error, format!("Scram failed: {why}. Run `bastion-mc scram` in a terminal.")),
                }
            }
            Message::DismissNotice => self.notice = None,
            Message::ToggleNewInstance => {
                self.new_instance.open = !self.new_instance.open;
                if self.new_instance.open && self.new_instance.versions.is_empty() {
                    let request = Request::Versions { snapshots: self.new_instance.snapshots };
                    return self.request(JobKind::Versions, request);
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
                return self.start(JobKind::Versions, Request::Versions { snapshots: on });
            }
            Message::CreateInstance => {
                if !self.can_create_instance() {
                    return Task::none();
                }
                let Some(version) = &self.new_instance.version else { return Task::none() };
                let request =
                    Request::CreateInstance { id: self.new_instance.id.clone(), minecraft: version.0.id.clone() };
                self.select_when_listed = Some(self.new_instance.id.clone());
                return self.start(JobKind::CreateInstance, request);
            }
            Message::Tick(now) => self.now = now,
            Message::CloseRequested(_) => {
                // A running game is shut down properly before the window goes away. A second
                // request does not wait: the worker closes the launcher's control pipe on
                // exit and the launcher finishes the shutdown by itself.
                if self.job_kind() == Some(JobKind::Launch) && !self.closing {
                    self.closing = true;
                    self.stop_job();
                    self.say("Stopping the game before closing…");
                } else {
                    return iced::exit();
                }
            }
        }
        Task::none()
    }

    fn on_update(&mut self, id: u64, update: Update) -> Task<Message> {
        // Anything from a job that is no longer current is stale.
        if self.job.as_ref().map(|job| job.id) != Some(id) {
            return Task::none();
        }
        match update {
            Update::Started(handle) => {
                if let Some(job) = &mut self.job {
                    if job.stopping {
                        handle.stop();
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
            Event::Hello { protocol, .. } => {
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
                    self.stop_job();
                }
            }
            Event::Status { text } => {
                self.console.push(Source::Launcher, text.clone());
                self.say(text);
            }
            Event::Phase { phase } => {
                if let Some(job) = &mut self.job {
                    job.phase = Some(phase);
                }
                if phase != Phase::Preparing {
                    self.progress = None;
                }
                self.say(phase_text(phase));
            }
            Event::Progress { label, done, total, unit } => {
                self.progress = Some(Progress { label, done, total, unit });
            }
            Event::DeviceCode { user_code, verification_uri, expires_in } => {
                self.say("Waiting for you to enter the code…");
                self.console
                    .push(Source::Launcher, format!("sign-in: enter the code shown above at {verification_uri}"));
                self.now = Instant::now();
                self.login = Some(LoginPrompt::DeviceCode {
                    user_code,
                    verification_uri,
                    expires_at: self.now + Duration::from_secs(expires_in),
                });
            }
            Event::BrowserLogin { url, timeout_secs } => {
                self.say("Waiting for you to sign in in your browser…");
                self.now = Instant::now();
                self.login =
                    Some(LoginPrompt::Browser { url, expires_at: self.now + Duration::from_secs(timeout_secs) });
            }
            Event::Account(account) => {
                if self.account_name.is_empty() && is_valid_id(&account.account) {
                    // Started without a label: this is the launcher's default account.
                    self.account_name = account.account.clone();
                }
                if account.account == self.account_name {
                    self.account = Some(account);
                }
            }
            Event::Accounts { default, accounts } => {
                self.accounts = accounts.into_iter().map(|a| a.account).filter(|name| is_valid_id(name)).collect();
                self.accounts.sort();
                self.accounts.dedup();
                self.default_account = Some(default).filter(|name| is_valid_id(name));
                self.remember_current_account();
            }
            Event::Protection { items } => self.protection = items,
            Event::Instances { instances } => self.set_instances(instances),
            Event::Versions { versions, .. } => {
                self.new_instance.versions = versions.into_iter().map(VersionChoice).collect();
                self.new_instance.version = self.new_instance.versions.first().cloned();
            }
            Event::Log { source, line } => self.console.push(source, line),
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
                    self.console.push_with(Source::Launcher, Level::Error, format!("error: {message}"));
                    self.notify(Severity::Error, message);
                }
            }
            Event::Unknown => {}
        }
    }

    fn set_instances(&mut self, instances: Vec<Instance>) {
        self.instances = instances.into_iter().map(InstanceChoice).collect();
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
            JobKind::SignIn if ok => self.status = "Signed in.".into(),
            JobKind::SignOut if ok => self.status = "Signed out.".into(),
            JobKind::Refresh | JobKind::Versions if ok => self.status.clear(),
            _ => {}
        }
        if self.closing {
            return iced::exit();
        }
        // A launch refreshes the stored token and may have learned the player's name.
        if job.kind == JobKind::Launch {
            return self.start(JobKind::Refresh, self.status_request());
        }
        // The "new instance" form was opened while something else was running: fetch its
        // version list now. (Not after a failed fetch — that would retry forever.)
        if self.new_instance.open && self.new_instance.versions.is_empty() && job.kind != JobKind::Versions {
            return self.start(JobKind::Versions, Request::Versions { snapshots: self.new_instance.snapshots });
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut all = vec![window::close_requests().map(Message::CloseRequested)];
        // Only while a countdown is on screen.
        if self.login.is_some() {
            all.push(iced::time::every(Duration::from_secs(1)).map(Message::Tick));
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

    pub fn stop_handle(sender: tokio::sync::mpsc::Sender<()>) -> Handle {
        backend::test_handle(sender)
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
    }

    #[test]
    fn play_is_disabled_while_launching_and_running_and_reenabled_after_exit() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        assert_eq!(app.job_kind(), Some(JobKind::Launch));
        assert!(!app.can_play(), "disabled the moment it is pressed");
        let _ = app.update(Message::Play);
        assert_eq!(app.next_job, 3, "a second press does not start a second launcher");

        feed(&mut app, events([Event::Phase { phase: Phase::Preparing }]));
        feed(&mut app, events([Event::Progress { label: "assets".into(), done: 50, total: 200, unit: Unit::Bytes }]));
        let progress = app.progress.clone().expect("progress is shown during downloads");
        assert_eq!(progress.fraction(), 0.25);
        assert!(progress.caption().contains("25%"), "{}", progress.caption());
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
        assert_eq!(app.play_blocker(), Some("Sign in, or tick demo mode."));
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
        let _ = app.update(Message::SignIn { device_code: true });
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
    }

    #[test]
    fn a_cancelled_sign_in_is_not_an_error() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { device_code: false });
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
        let _ = app.update(Message::SignIn { device_code: true });
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
        let _ = app.update(Message::CloseRequested(window::Id::unique()));
        assert!(app.closing && app.busy(), "the window stays until the game is down");
        assert!(receiver.try_recv().is_ok(), "the game was asked to stop");
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
    fn progress_edge_cases() {
        let p = Progress { label: "assets".into(), done: 0, total: 0, unit: Unit::Files };
        assert_eq!(p.fraction(), 1.0, "nothing to do is complete, not a division by zero");
        let p = Progress { label: "assets".into(), done: 9, total: 4, unit: Unit::Files };
        assert_eq!(p.fraction(), 1.0);
        assert_eq!(p.caption(), "Downloading assets: 9 of 4 files (100%)");
    }
}

#[cfg(test)]
mod account_tests {
    use super::{testing::*, *};

    fn named(name: &str, signed_in: bool) -> Account {
        Account { account: name.into(), ..account(signed_in) }
    }

    #[test]
    fn without_a_label_the_launchers_default_account_is_adopted() {
        let (mut app, _task) = App::boot(PathBuf::from("/nonexistent/bastion-mc"), String::new());
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
        assert_eq!(app.accounts, ["alt", "work"]);
        assert_eq!(app.default_account.as_deref(), Some("work"));
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
            Some("Sign in, or tick demo mode."),
            "the other account's sign-in does not count"
        );
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
        assert!(app.accounts.contains(&"alt".to_string()));
        assert!(app.new_account.is_empty());
        // Busy now (refreshing): no further switch.
        let _ = app.update(Message::AccountSelected("default".into()));
        assert_eq!(app.account_name, "alt");
    }

    #[test]
    fn account_labels_from_the_launcher_are_checked_before_they_are_shown() {
        let mut app = ready(true);
        let _ = app.update(Message::Refresh);
        feed(
            &mut app,
            events([Event::Accounts {
                default: "../x".into(),
                accounts: vec![named("ok", false), named("\u{1b}[31mbad", false)],
            }]),
        );
        assert_eq!(app.accounts, ["default", "ok"]);
        assert_eq!(app.default_account, None);
    }
}

#[cfg(test)]
mod emergency_tests {
    use super::{testing::*, *};

    #[test]
    fn scram_needs_the_question_answered_and_runs_once() {
        let mut app = ready(true);
        // Without the question having been asked, a confirmation does nothing.
        let _ = app.update(Message::ConfirmScram);
        assert!(!app.emergency.running);
        let _ = app.update(Message::AskScram);
        assert!(app.emergency.confirming);
        let _ = app.update(Message::CancelScram);
        let _ = app.update(Message::ConfirmScram);
        assert!(!app.emergency.running, "cancelled");
        let _ = app.update(Message::AskScram);
        let _ = app.update(Message::ConfirmScram);
        assert!(app.emergency.running && !app.emergency.confirming);
        // While it runs it cannot be asked for again.
        let _ = app.update(Message::AskScram);
        assert!(!app.emergency.confirming);

        let lines = vec!["removed the refresh token of account default from the kernel keyring".to_string()];
        let _ = app.update(Message::ScramFinished(Ok(lines.clone())));
        assert!(!app.emergency.running);
        assert_eq!(app.emergency.report, lines);
        assert!(app.account.is_none() && !app.signed_in(), "the old sign-in state is not shown any more");
        assert!(app.notice.as_ref().is_some_and(|n| n.text.contains("Nothing is revoked at Microsoft")));
        assert_eq!(app.job_kind(), Some(JobKind::Refresh), "the launcher is asked for the new state");
    }

    #[test]
    fn scram_works_while_a_game_is_running_and_failures_are_said() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        assert_eq!(app.job_kind(), Some(JobKind::Launch));
        let _ = app.update(Message::AskScram);
        let _ = app.update(Message::ConfirmScram);
        assert!(app.emergency.running, "a running game does not block it");
        let _ = app.update(Message::ScramFinished(Err("no such file".into())));
        assert!(app.notice.as_ref().is_some_and(|n| n.severity == Severity::Error && n.text.contains("no such file")));
        assert_eq!(app.job_kind(), Some(JobKind::Launch), "the game's job is left to end by itself");
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
}
