//! The phone's dispatcher: between the companion channel (protocol 2) and the apps of the
//! services prepared for this launch.
//!
//! Every command line the mod sends is untrusted (anything in the game's JVM can write it):
//! it is parsed strictly ([`Command::parse`]), routed by app name to the one app that has
//! that name, refused unless the verb is in that app's [`VerbLimit`]s, and refused when that
//! verb's token bucket is empty. Refusals are counted, and each kind of refusal is logged
//! once — so a flood of nonsense costs a counter, not the log. Nothing here interprets an
//! argument: what a verb may do is the app's own fixed vocabulary.
//!
//! When the phone connects (a hello of protocol 2), the dispatcher sends the `phone` state —
//! the apps whose services were prepared, and the theme — and asks every app for its state.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use super::{Command, Outbox, PhoneApp, Refused, VerbLimit};

/// The app name of the dispatcher's own state.
pub const PHONE_APP: &str = "phone";

/// Where the dispatcher's log lines go (the launcher's own voice).
pub type Logger = std::sync::Arc<dyn Fn(String) + Send + Sync>;

/// A token bucket in thousandths of a token.
#[derive(Debug, Clone)]
struct Bucket {
    milli: u64,
    capacity: u64,
    /// Thousandths of a token per millisecond, times 60 (that is: per minute, in thousandths).
    per_minute: u64,
    refilled: Duration,
}

impl Bucket {
    fn new(limit: &VerbLimit) -> Self {
        let capacity = u64::from(limit.burst.max(1)) * 1000;
        Bucket { milli: capacity, capacity, per_minute: u64::from(limit.per_minute) * 1000, refilled: Duration::ZERO }
    }

    /// Takes a token at `now` (time since the session began), if there is one.
    fn take(&mut self, now: Duration) -> bool {
        let elapsed = now.saturating_sub(self.refilled).as_millis().min(u128::from(u32::MAX)) as u64;
        if elapsed > 0 {
            self.refilled = now;
            self.milli = (self.milli + elapsed * self.per_minute / 60_000).min(self.capacity);
        }
        if self.milli >= 1000 {
            self.milli -= 1000;
            true
        } else {
            false
        }
    }
}

/// Why the dispatcher did not hand a command to an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Outside the grammar.
    Grammar(Refused),
    /// No app of that name in this launch.
    UnknownApp(String),
    /// The app has no such verb.
    UnknownVerb { app: &'static str, verb: String },
    /// The verb's bucket was empty.
    OverLimit { app: &'static str, verb: &'static str },
}

impl Refusal {
    /// The kind, for counting and for logging once: never more than one line per kind.
    fn kind(&self) -> String {
        match self {
            Refusal::Grammar(why) => format!("not a command ({})", why.as_str()),
            // The name came from the game: shown only when it is a well-formed name, and
            // counted under one kind whatever it is, so that a stream of different names
            // cannot make the log grow.
            Refusal::UnknownApp(_) => "a command for an app this launch does not have".into(),
            Refusal::UnknownVerb { app, .. } => format!("a verb the {app} app does not have"),
            Refusal::OverLimit { app, verb } => format!("{app} {verb}: over its rate limit"),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Command lines received.
    pub commands: u64,
    /// Handed to an app.
    pub accepted: u64,
    /// Refused, by kind.
    pub refused: BTreeMap<String, u64>,
    /// Phone connections greeted.
    pub hellos: u64,
}

/// Routes the phone's commands to the apps of this launch.
pub struct Dispatcher {
    apps: Vec<Box<dyn PhoneApp>>,
    buckets: BTreeMap<(&'static str, &'static str), Bucket>,
    out: Outbox,
    logger: Logger,
    logged: BTreeSet<String>,
    stats: Stats,
    theme: &'static str,
}

impl Dispatcher {
    /// `apps` are those of the services prepared for this launch (their names must differ);
    /// `out` is where everything for the phone goes.
    pub fn new(apps: Vec<Box<dyn PhoneApp>>, out: Outbox, logger: Logger) -> Dispatcher {
        let mut buckets = BTreeMap::new();
        let mut names = BTreeSet::new();
        let mut kept: Vec<Box<dyn PhoneApp>> = Vec::new();
        for app in apps {
            if app.name() == PHONE_APP || !names.insert(app.name()) {
                logger(format!("phone: a second app named {:?} was left out", app.name()));
                continue;
            }
            for limit in app.limits() {
                buckets.insert((app.name(), limit.verb), Bucket::new(limit));
            }
            kept.push(app);
        }
        Dispatcher { apps: kept, buckets, out, logger, logged: BTreeSet::new(), stats: Stats::default(), theme: "dark" }
    }

    /// The names of the apps, in the order the services were prepared.
    pub fn app_names(&self) -> Vec<&'static str> {
        self.apps.iter().map(|app| app.name()).collect()
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// The phone connected (or reconnected): its own state, then every app's.
    pub fn hello(&mut self) {
        self.stats.hellos += 1;
        let state = serde_json::json!({ "apps": self.app_names(), "theme": self.theme });
        self.out.state(PHONE_APP, &state);
        for app in &mut self.apps {
            app.hello(&self.out);
        }
    }

    /// One command line from the phone, at `now` (time since the session began).
    pub fn command(&mut self, line: &[u8], now: Duration) -> Result<(), Refusal> {
        self.stats.commands += 1;
        let outcome = self.route(line, now);
        match &outcome {
            Ok(()) => self.stats.accepted += 1,
            Err(refusal) => {
                let kind = refusal.kind();
                *self.stats.refused.entry(kind.clone()).or_default() += 1;
                if self.logged.insert(kind.clone()) {
                    (self.logger)(format!("phone: refused {kind} (further ones are only counted)"));
                }
            }
        }
        outcome
    }

    fn route(&mut self, line: &[u8], now: Duration) -> Result<(), Refusal> {
        let command = Command::parse(line).map_err(Refusal::Grammar)?;
        let index = self
            .apps
            .iter()
            .position(|app| app.name() == command.app)
            .ok_or_else(|| Refusal::UnknownApp(command.app.clone()))?;
        let app_name = self.apps[index].name();
        let verb = self.apps[index]
            .limits()
            .iter()
            .find(|limit| limit.verb == command.verb)
            .map(|limit| limit.verb)
            .ok_or_else(|| Refusal::UnknownVerb { app: app_name, verb: command.verb.clone() })?;
        let bucket = self.buckets.get_mut(&(app_name, verb)).expect("a bucket for every listed verb");
        if !bucket.take(now) {
            return Err(Refusal::OverLimit { app: app_name, verb });
        }
        self.apps[index].command(command, &self.out);
        Ok(())
    }

    /// One line for the end of the launch log.
    pub fn summary(&self) -> String {
        let s = &self.stats;
        let mut text = format!(
            "phone: {} app(s) ({}), {} connection(s) greeted, {} command(s), {} accepted",
            self.apps.len(),
            if self.apps.is_empty() { "none".to_string() } else { self.app_names().join(", ") },
            s.hellos,
            s.commands,
            s.accepted
        );
        let refused: u64 = s.refused.values().sum();
        if refused > 0 {
            let kinds: Vec<String> = s.refused.iter().map(|(kind, n)| format!("{n} {kind}")).collect();
            text.push_str(&format!(", {refused} refused ({})", kinds.join("; ")));
        }
        let dropped = self.out.dropped();
        if dropped > 0 {
            text.push_str(&format!(", {dropped} message(s) to the phone dropped (it did not keep up)"));
        }
        text
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::phone::Outbound;

    /// An app that records what it was asked.
    pub(crate) struct Recorder {
        pub name: &'static str,
        pub seen: Arc<Mutex<Vec<String>>>,
    }

    const LIMITS: &[VerbLimit] =
        &[VerbLimit { verb: "play", per_minute: 60, burst: 2 }, VerbLimit { verb: "search", per_minute: 6, burst: 1 }];

    impl PhoneApp for Recorder {
        fn name(&self) -> &'static str {
            self.name
        }
        fn limits(&self) -> &'static [VerbLimit] {
            LIMITS
        }
        fn hello(&mut self, out: &Outbox) {
            out.state(self.name, &serde_json::json!({ "hello": true }));
        }
        fn command(&mut self, command: Command, out: &Outbox) {
            self.seen
                .lock()
                .unwrap()
                .push(format!("{} {} {:?} {:?}", command.app, command.verb, command.args, command.text));
            out.notice(self.name, "done");
        }
    }

    type Lines = Arc<Mutex<Vec<String>>>;

    fn setup() -> (Dispatcher, Lines, Lines, tokio::sync::mpsc::Receiver<Outbound>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let (out, rx) = Outbox::new(64);
        let apps: Vec<Box<dyn PhoneApp>> = vec![
            Box::new(Recorder { name: "spotify", seen: seen.clone() }),
            Box::new(Recorder { name: "spotify", seen: seen.clone() }),
            Box::new(Recorder { name: "phone", seen: seen.clone() }),
        ];
        let dispatcher = Dispatcher::new(apps, out, Arc::new(move |line| sink.lock().unwrap().push(line)));
        (dispatcher, seen, log, rx)
    }

    #[test]
    fn hello_sends_the_phone_state_with_only_the_prepared_apps_and_then_each_apps_state() {
        let (mut dispatcher, _, log, mut rx) = setup();
        assert_eq!(
            dispatcher.app_names(),
            ["spotify"],
            "a second app of the same name, or one called phone, is left out"
        );
        assert_eq!(log.lock().unwrap().len(), 2);
        dispatcher.hello();
        let Ok(Outbound::State { app: "phone", json }) = rx.try_recv() else { panic!("the phone state first") };
        assert_eq!(json, r#"{"apps":["spotify"],"theme":"dark"}"#);
        assert!(matches!(rx.try_recv(), Ok(Outbound::State { app: "spotify", .. })));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn commands_reach_their_app_only_within_its_vocabulary_and_limits() {
        let (mut dispatcher, seen, log, _rx) = setup();
        let t = Duration::from_secs(1);
        assert_eq!(dispatcher.command(b"spotify play", t), Ok(()));
        assert_eq!(dispatcher.command(b"spotify search -- Daft Punk", t), Ok(()));
        // The burst of `search` is one: the next one at the same instant is over the limit.
        assert_eq!(
            dispatcher.command(b"spotify search -- again", t),
            Err(Refusal::OverLimit { app: "spotify", verb: "search" })
        );
        // Six a minute: ten seconds later there is one again.
        assert_eq!(dispatcher.command(b"spotify search -- later", t + Duration::from_secs(10)), Ok(()));
        assert_eq!(
            dispatcher.command(b"spotify wipe-library", t),
            Err(Refusal::UnknownVerb { app: "spotify", verb: "wipe-library".into() })
        );
        assert_eq!(dispatcher.command(b"browser open discord", t), Err(Refusal::UnknownApp("browser".into())));
        assert_eq!(dispatcher.command(b"spotify play\n", t), Err(Refusal::Grammar(Refused::ControlCharacter)));
        assert_eq!(dispatcher.command(b"", t), Err(Refusal::Grammar(Refused::Empty)));
        assert_eq!(seen.lock().unwrap().len(), 3);
        assert_eq!(seen.lock().unwrap()[1], "spotify search [] Some(\"Daft Punk\")");
        let stats = dispatcher.stats().clone();
        assert_eq!((stats.commands, stats.accepted), (8, 3));
        assert_eq!(stats.refused.values().sum::<u64>(), 5);
        let summary = dispatcher.summary();
        assert!(summary.starts_with("phone: 1 app(s) (spotify), 0 connection(s) greeted, 8 command(s), 3 accepted"));
        assert!(summary.contains("5 refused"), "{summary}");
        // Each kind once in the log, however often it happens.
        for _ in 0..1000 {
            let _ = dispatcher.command(b"browser open discord", t);
            let _ = dispatcher.command(b"x y z \xff", t);
        }
        let lines = log.lock().unwrap().clone();
        assert_eq!(lines.iter().filter(|l| l.contains("an app this launch does not have")).count(), 1);
        assert_eq!(lines.iter().filter(|l| l.contains("not UTF-8")).count(), 1);
        assert!(lines.iter().all(|l| !l.contains("discord")), "a name from the game is not logged: {lines:?}");
        assert_eq!(dispatcher.stats().commands, 2008);
    }

    #[test]
    fn a_burst_refills_at_its_rate_and_never_beyond_the_burst() {
        let mut bucket = Bucket::new(&VerbLimit { verb: "x", per_minute: 60, burst: 3 });
        let t = Duration::from_secs(5);
        assert!((0..3).all(|_| bucket.take(t)));
        assert!(!bucket.take(t));
        assert!(bucket.take(t + Duration::from_secs(1)), "one a second");
        assert!(!bucket.take(t + Duration::from_secs(1)));
        // An hour of rest gives the burst back, not more.
        let later = t + Duration::from_secs(3600);
        assert_eq!((0..10).filter(|_| bucket.take(later)).count(), 3);
        // Time going backwards gives nothing.
        assert!(!bucket.take(t));
    }
}
