//! The view, exercised without a display server: `iced_test` lays the widgets out, clicks
//! them and renders them in software.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use bastion_mc_proto::{Account, Event, Instance, Phase, Protection, Shield, Source, Unit, Version, runbook};
use iced::Size;
use iced_test::{
    Simulator,
    selector::{self, Candidate},
};

use super::*;
use crate::{
    app::{
        ScramOutcome, Severity,
        testing::{
            account, at, confirm_scram, events, feed, finish, instance, named, ready, running_for, scram, scram_lines,
        },
    },
    backend::{Outcome, ScramEnd, ScramUpdate, Update},
    theme::caps,
};

/// One headless renderer at a time: with a GPU backend, several of them starting up on
/// different test threads at once is more than some drivers survive.
static RENDERER: Mutex<()> = Mutex::new(());

fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    RENDERER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A small window, where the layout has to fold, and a large one where it spreads out. The
/// tests click at the first; the snapshots are taken at both, and the layout is checked at
/// these two, at the size the window opens with and at the smallest it can be made.
const SMALL: Size = Size::new(880.0, 720.0);
const LARGE: Size = Size::new(1280.0, 800.0);
const OPENING: Size = crate::WINDOW_SIZE;
const SMALLEST: Size = crate::WINDOW_MIN_SIZE;

fn ui(app: &App, size: Size) -> Simulator<'_, Message> {
    let settings = iced_test::core::Settings { default_font: theme::UI_FONT, ..Default::default() };
    Simulator::with_size(settings, size, root(app))
}

fn clicked(app: &App, label: &str) -> Vec<Message> {
    let _one = exclusive();
    let mut ui = ui(app, SMALL);
    ui.click(label).unwrap_or_else(|e| panic!("cannot click {label:?}: {e}"));
    ui.into_messages().collect()
}

fn clicked_id(app: &App, id: Id) -> Vec<Message> {
    let _one = exclusive();
    let mut ui = ui(app, SMALL);
    ui.click(selector::id(id.clone())).unwrap_or_else(|e| panic!("cannot click {id:?}: {e}"));
    ui.into_messages().collect()
}

fn shows(app: &App, text: &str) -> bool {
    let _one = exclusive();
    ui(app, SMALL).find(text).is_ok()
}

/// Every piece of text on the page, visible or scrolled away.
fn texts(app: &App, size: Size) -> Vec<String> {
    let _one = exclusive();
    let found = Arc::new(Mutex::new(Vec::new()));
    let collector = {
        let found = found.clone();
        move |candidate: Candidate<'_>| -> Option<()> {
            if let Candidate::Text { content, .. } = candidate {
                found.lock().unwrap().push(content.to_string());
            }
            None
        }
    };
    let _ = ui(app, size).find(collector);
    found.lock().unwrap().clone()
}

fn on(app: &mut App, page: Page) {
    let _ = app.update(Message::Navigate(page));
}

// ------------------------------------------------------------------ fixtures

fn launching(app: &mut App) {
    let _ = app.update(Message::Play);
    feed(app, events([Event::Phase { phase: Phase::Preparing }]));
}

fn with_device_code(app: &mut App) {
    on(app, Page::Accounts);
    let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
    feed(
        app,
        events([Event::DeviceCode {
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://www.microsoft.com/link".into(),
            expires_in: 900,
        }]),
    );
}

fn protections(levels: [Shield; 5]) -> Event {
    let said = |aspect: &str, level: Shield| -> &'static str {
        match (aspect, level) {
            ("display", Shield::On) => "nested compositor: a display server of the game's own, clipboard cut",
            ("display", Shield::Partial) => "the session's Wayland socket, without a security context",
            ("display", Shield::Off) => "the session's X11 display: the game can watch and type into other windows",
            ("network", Shield::On) => "filtered: only allowlisted hosts, through the launcher",
            ("network", _) => "open: the host's network, no egress control",
            ("credential", Shield::On) => "broker: the game holds a placeholder, the token stays in the launcher",
            ("credential", _) => "the session token is inside the game process",
            ("limits", Shield::On) => "memory, swap and task limits (systemd scope)",
            ("limits", _) => "no resource limits",
            ("launcher", Shield::On) => "confines itself while the game runs",
            _ => "not self-confined",
        }
    };
    Event::Protection {
        items: ["display", "network", "credential", "limits", "launcher"]
            .into_iter()
            .zip(levels)
            .map(|(aspect, shield)| Protection { aspect: aspect.into(), shield, detail: said(aspect, shield).into() })
            .collect(),
    }
}

const MIXED: [Shield; 5] = [Shield::On, Shield::On, Shield::Partial, Shield::Off, Shield::On];
const ALL_ON: [Shield; 5] = [Shield::On; 5];

/// A game that is running, with output in the console.
fn running(levels: [Shield; 5]) -> App {
    let mut app = ready(true);
    launching(&mut app);
    feed(
        &mut app,
        events([
            Event::Phase { phase: Phase::Verifying },
            Event::Phase { phase: Phase::Authenticating },
            Event::Phase { phase: Phase::Starting },
            protections(levels),
            Event::Status { text: "sandbox started; log: ~/.local/state/bastion-mc/logs/vanilla/launch-1.log".into() },
            Event::Phase { phase: Phase::Running },
            Event::Log {
                source: Source::Egress,
                line: "egress: ALLOW api.minecraftservices.com:443 tls, SNI verified".into(),
            },
            Event::Log {
                source: Source::Egress,
                line: "egress: DENY  name lookup \"telemetry.example\" (not allowlisted)".into(),
            },
        ]),
    );
    // What a current client really prints: one log4j XML document per message.
    for line in [
        r#"<log4j:Event logger="net.minecraft.client.Minecraft" timestamp="1" level="INFO" thread="Render thread">"#,
        r#"  <log4j:Message><![CDATA[Setting user: Alex]]></log4j:Message>"#,
        r#"</log4j:Event>"#,
        r#"<log4j:Event logger="oshi.util.FileUtil" timestamp="2" level="WARN" thread="main">"#,
        r#"  <log4j:Message><![CDATA[File not found or not readable: /etc/os-release]]></log4j:Message>"#,
        r#"</log4j:Event>"#,
        r#"<log4j:Event logger="net.minecraft.client.renderer.texture.TextureAtlas" timestamp="3" level="INFO" thread="Render thread">"#,
        r#"  <log4j:Message><![CDATA[Created: 1024x512x4 minecraft:textures/atlas/blocks.png-atlas]]></log4j:Message>"#,
        r#"</log4j:Event>"#,
        r#"<log4j:Event logger="net.minecraft.client.sounds.SoundEngine" timestamp="4" level="ERROR" thread="Render thread">"#,
        r#"  <log4j:Message><![CDATA[Error starting SoundSystem. Turning off sounds & music]]></log4j:Message>"#,
        r#"</log4j:Event>"#,
    ] {
        feed(&mut app, events([Event::Log { source: Source::Game, line: line.into() }]));
    }
    running_for(&mut app, Duration::from_secs(151));
    app
}

/// Three accounts as the launcher would list them, the second one in use.
fn three_accounts(app: &mut App) {
    let _ = app.update(Message::Refresh);
    feed(
        app,
        events([Event::Accounts {
            default: "default".into(),
            accounts: vec![
                named("alt", false),
                named("default", true),
                Account {
                    player: None,
                    store: "age vault (hardware-backed identity) at ~/.local/share/bastion-mc/vault".into(),
                    ..named("work-2", true)
                },
            ],
        }]),
    );
    finish(app, 0);
}

fn with_instances(app: &mut App) {
    let _ = app.update(Message::Refresh);
    feed(
        app,
        events([Event::Instances {
            instances: vec![
                instance("broken", Some("manifest signature does not verify against any trusted key")),
                Instance {
                    id: "fab".into(),
                    minecraft: Some("26.3".into()),
                    kind: "curated".into(),
                    detail: Some("serial 4, signed by me".into()),
                    error: None,
                },
                instance("vanilla", None),
                Instance { id: "vanilla-latest".into(), ..instance("x", None) },
            ],
        }]),
    );
    finish(app, 0);
}

const REMOVED: [&str; 9] = [
    "killed the running launcher of instance vanilla (pid 48211): its session token is gone with it",
    "removed the refresh token of account default from the kernel keyring",
    "removed the refresh token of account work-2 from the kernel keyring",
    "forgot the remembered player of account default",
    "",
    "This removed the copies on this machine. It revoked NOTHING at Microsoft or Mojang:",
    "  - a refresh token that was copied earlier works until you \"sign out everywhere\";",
    "  - a Minecraft session token that was already issued is valid for up to about 24 hours.",
    "Next: `bastion-mc wiki scram` — the recovery steps, with verified links.",
];
const NOT_REMOVED: [&str; 2] = [
    "COULD NOT delete the vault file /home/alex/.local/share/bastion-mc/vault/work-2.age: Permission denied (os error 13)",
    "COULD NOT remove the remembered player of account work-2: Read-only file system (os error 30)",
];

fn scrammed(lines: &[&str], end: Option<ScramEnd>) -> App {
    let mut app = ready(true);
    on(&mut app, Page::Security);
    confirm_scram(&mut app);
    scram_lines(&mut app, lines);
    if let Some(end) = end {
        scram(&mut app, [ScramUpdate::Finished(end)]);
        if app.busy() {
            // The refresh that follows: the launcher's new word on the accounts.
            feed(&mut app, events([Event::Account(account(false))]));
            finish(&mut app, 0);
        }
    } else {
        let since = app.emergency.since.expect("a scram is running");
        let _ = app.update(Message::Tick(since + Duration::from_secs(47)));
    }
    app
}

// ------------------------------------------------------------------ navigation

#[test]
fn the_rail_leads_to_every_page_and_marks_where_you_are() {
    let mut app = ready(true);
    for page in Page::ALL {
        assert!(
            matches!(clicked_id(&app, nav_id(page)).as_slice(), [Message::Navigate(to)] if *to == page),
            "{page:?}"
        );
        assert!(shows(&app, &caps(page.title())), "every entry is labelled: {page:?}");
    }
    // Each page shows something only it has.
    for (page, landmark) in [
        (Page::Launch, caps("Sequence")),
        (Page::Accounts, caps("New account")),
        (Page::Instances, caps("New instance")),
        (Page::Console, "Launcher and game output appears here.".to_string()),
        (Page::Security, caps("Recovery pages")),
    ] {
        on(&mut app, page);
        assert!(shows(&app, &landmark), "{page:?} shows {landmark:?}");
        let elsewhere = Page::ALL.into_iter().filter(|other| *other != page);
        for other in elsewhere {
            let mut there = ready(true);
            on(&mut there, other);
            assert!(!shows(&there, &landmark), "{landmark:?} belongs to {page:?}, not {other:?}");
        }
    }
    // The account at the foot of the rail leads to the accounts.
    assert!(matches!(clicked_id(&app, Id::new("rail-account")).as_slice(), [Message::Navigate(Page::Accounts)]));
}

#[test]
fn the_rail_says_what_is_going_on_behind_the_other_pages() {
    let mut app = ready(false);
    assert_eq!(attention(&app, Page::Launch), None);
    assert_eq!(attention(&app, Page::Accounts), None);
    assert_eq!(attention(&app, Page::Security), None);
    with_device_code(&mut app);
    assert_eq!(attention(&app, Page::Accounts), Some(Tone::Caution), "a sign-in waits for the user");

    let app = running(ALL_ON);
    assert_eq!(attention(&app, Page::Launch), Some(Tone::Accent));
    assert_eq!(attention(&app, Page::Security), None, "everything in force: nothing to point at");
    assert_eq!(attention(&running(MIXED), Page::Security), Some(Tone::Danger), "one aspect is unprotected");
    let reduced = running([Shield::On, Shield::On, Shield::Partial, Shield::On, Shield::On]);
    assert_eq!(attention(&reduced, Page::Security), Some(Tone::Caution));
    let failed = scrammed(&NOT_REMOVED, Some(ScramEnd::Exited(1)));
    assert_eq!(attention(&failed, Page::Security), Some(Tone::Danger), "an incomplete scram stays marked");

    // What the launcher is doing is named at the top of every page.
    assert_eq!(activity(&ready(true)).0, "Ready");
    assert_eq!(activity(&running(ALL_ON)).0, "Game running");
    assert_eq!(activity(&scrammed(&REMOVED, None)), ("Scram in progress", Tone::Danger));
}

// ------------------------------------------------------------------ launch

#[test]
fn the_launch_control_follows_the_state() {
    let mut app = ready(true);
    assert!(matches!(clicked(&app, &caps("Play")).as_slice(), [Message::Play]));
    assert!(matches!(clicked_id(&app, launch::core_id()).as_slice(), [Message::Play]));
    assert!(shows(&app, "Alex"), "the control says who would play");

    assert!(shows(&app, "Ready: vanilla."), "and what would be launched");
    assert!(clicked(&app, &caps("Stop")).is_empty(), "nothing to stop: the control is drawn, not offered");

    launching(&mut app);
    assert!(shows(&app, &caps("Launching")) && !shows(&app, &caps("Play")));
    assert!(clicked(&app, &caps("Launching")).is_empty(), "a control that cannot be used produces nothing");
    assert!(matches!(clicked(&app, &caps("Stop")).as_slice(), [Message::Stop]));

    feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
    assert!(shows(&app, &caps("Running")) && clicked(&app, &caps("Running")).is_empty());
    running_for(&mut app, Duration::from_secs(75));
    assert!(shows(&app, "1:15"), "the time the game has been running");

    let _ = app.update(Message::Stop);
    assert!(shows(&app, &caps("Stopping")));
    assert!(clicked(&app, &caps("Stop")).is_empty(), "a stop that was asked for is not asked for twice");

    feed(&mut app, events([Event::Exited { code: 130 }]));
    finish(&mut app, 130); // launch
    finish(&mut app, 0); // the refresh that follows it
    assert!(matches!(clicked(&app, &caps("Play")).as_slice(), [Message::Play]), "enabled again after the game exits");
    assert!(shows(&app, "The game was stopped."));
    assert!(clicked(&app, &caps("Stop")).is_empty(), "the stop control keeps its place, with nothing left to stop");
}

#[test]
fn the_launch_control_is_inert_and_explains_itself_when_nothing_can_be_launched() {
    let app = ready(false);
    assert!(clicked(&app, &caps("Play")).is_empty());
    assert!(shows(&app, "Sign in, or switch on demo mode."));
    assert!(matches!(clicked(&app, "Demo mode (no account)").as_slice(), [Message::DemoToggled(true)]));
    let mut demo = ready(false);
    let _ = demo.update(Message::DemoToggled(true));
    assert!(matches!(clicked(&demo, &caps("Play")).as_slice(), [Message::Play]));
    assert!(shows(&demo, "demo mode"));

    let mut broken = ready(true);
    let choice = broken.instances.iter().find(|i| !i.launchable()).cloned().unwrap();
    let _ = broken.update(Message::InstanceSelected(choice));
    assert!(clicked(&broken, &caps("Play")).is_empty());
    assert!(shows(&broken, "This instance cannot be launched.") && shows(&broken, "bad signature"));
}

#[test]
fn download_progress_is_visible_while_preparing() {
    let mut app = ready(true);
    launching(&mut app);
    feed(
        &mut app,
        events([Event::Progress { label: "assets".into(), done: 104857600, total: 419430400, unit: Unit::Bytes }]),
    );
    assert!(shows(&app, "100.0 / 400.0 MiB") && shows(&app, &caps("assets")), "what is measured, as figures");
    let shares = texts(&app, SMALL).iter().filter(|text| *text == "25%").count();
    assert_eq!(shares, 2, "the share is in the launch control and in the sequence");
    feed(&mut app, events([Event::Phase { phase: Phase::Verifying }]));
    assert!(!shows(&app, "100.0 / 400.0 MiB") && !shows(&app, "25%"));
    assert!(shows(&app, "Verifying every file before it is mounted…"));
}

#[test]
fn the_sequence_spells_out_where_a_launch_is() {
    let count = |app: &App, word: &str| texts(app, SMALL).iter().filter(|text| *text == word).count();
    let mut app = ready(true);
    assert_eq!((count(&app, "done"), count(&app, "now")), (0, 0), "nothing is being launched");
    let _ = app.update(Message::DemoToggled(true));
    launching(&mut app);
    feed(&mut app, events([Event::Phase { phase: Phase::Verifying }]));
    assert_eq!((count(&app, "done"), count(&app, "now")), (1, 1));
    feed(&mut app, events([Event::Phase { phase: Phase::Starting }]));
    assert_eq!((count(&app, "done"), count(&app, "skipped"), count(&app, "now")), (2, 1, 1), "no sign-in in demo mode");
    for name in ["Files", "Verify", "Sign-in", "Sandbox", "Game"] {
        assert!(shows(&app, &caps(name)), "{name}");
    }
}

#[test]
fn the_three_shield_levels_are_told_apart_in_words_on_both_pages() {
    let mut app = ready(true);
    // Before any launch: the aspects, and no claim about them.
    for page in [Page::Launch, Page::Security] {
        on(&mut app, page);
        let all = texts(&app, SMALL);
        assert_eq!(all.iter().filter(|text| **text == caps("No report")).count(), 5, "{page:?}");
        for word in ["In force", "Reduced", "Not protected"] {
            assert!(!all.contains(&caps(word)), "{page:?} claims {word:?} before anything was reported");
        }
        assert!(all.contains(&caps("credential")));
    }
    on(&mut app, Page::Launch);
    assert!(shows(&app, &caps("No launch yet")));
    // A launch that has not reported is a launch, with nothing known about it yet.
    launching(&mut app);
    assert!(shows(&app, &caps("Awaiting report")) && !shows(&app, &caps("No launch yet")));
    assert_eq!(texts(&app, SMALL).iter().filter(|text| **text == caps("No report")).count(), 5);

    let mut app = running(MIXED);
    for page in [Page::Launch, Page::Security] {
        on(&mut app, page);
        let all = texts(&app, SMALL);
        let count = |word: &str| all.iter().filter(|text| **text == caps(word)).count();
        assert_eq!((count("In force"), count("Reduced"), count("Not protected")), (3, 1, 1), "{page:?}");
        assert_eq!(count("No report"), 0);
        // The launcher's own description of each, next to the level.
        assert!(all.iter().any(|text| text == "the session token is inside the game process"), "{page:?}");
        assert!(all.iter().any(|text| text == "no resource limits"), "{page:?}");
    }
    on(&mut app, Page::Launch);
    assert!(shows(&app, &caps("1 not protected")), "the worst level is the summary");
    assert!(shows(&app, &caps("Protection")) && !shows(&app, &caps("Protection of the last launch")));
    assert!(shows(&running(ALL_ON), &caps("All in force")));
    // Once the game is over, what stays on the page is said to be the last launch's.
    let mut over = running(ALL_ON);
    feed(&mut over, events([Event::Exited { code: 0 }]));
    finish(&mut over, 0);
    finish(&mut over, 0);
    assert!(shows(&over, &caps("Protection of the last launch")) && shows(&over, &caps("All in force")));
    let _one = exclusive();
    assert!(ui(&over, LARGE).find(caps("Last launch").as_str()).is_ok(), "the short form in the narrow panel");
    drop(_one);
    let reduced = running([Shield::On, Shield::Partial, Shield::Partial, Shield::On, Shield::On]);
    assert!(shows(&reduced, &caps("2 reduced")));

    // The security page explains the three marks once, and what each aspect covers.
    on(&mut app, Page::Security);
    let all = texts(&app, SMALL);
    for start in ["In force:", "Reduced:", "Not protected:"] {
        assert!(all.iter().any(|text| text.starts_with(start)), "{start}");
    }
    assert!(all.iter().any(|text| text == "Covers what the game holds of your account."));
    assert!(all.iter().any(|text| text == "The game that is running was started with:"));

    // An aspect this build has never heard of is shown under its own name, with its level.
    let mut future = ready(true);
    launching(&mut future);
    feed(
        &mut future,
        events([Event::Protection {
            items: vec![Protection { aspect: "microphone".into(), shield: Shield::Off, detail: "open".into() }],
        }]),
    );
    assert!(shows(&future, &caps("microphone")) && shows(&future, &caps("Not protected")));
}

// ------------------------------------------------------------------ accounts

#[test]
fn every_account_has_a_card_with_what_the_launcher_said_about_it() {
    let mut app = ready(true);
    three_accounts(&mut app);
    on(&mut app, Page::Accounts);
    let all = texts(&app, SMALL);
    for label in ["alt", "default", "work-2"] {
        assert!(all.iter().any(|text| text == label), "a card for {label}");
    }
    let count = |word: &str| all.iter().filter(|text| text.as_str() == word).count();
    assert_eq!((count("Signed in"), count("Not signed in")), (3, 1), "two cards and the rail; one card");
    assert_eq!(count("plays as Alex"), 1, "the player name where the launcher knows one");
    assert_eq!(count("no player name known"), 2, "and no made-up one where it does not");
    assert_eq!(count("kernel keyring (RAM only, cleared at reboot)"), 2, "where each token rests");
    assert!(all.iter().any(|text| text.starts_with("age vault (hardware-backed identity)")));
    assert_eq!(all.iter().filter(|text| **text == caps("In use")).count(), 1);
    assert_eq!(all.iter().filter(|text| **text == caps("Default")).count(), 1);
    assert!(!app.busy(), "the cards needed no launcher call of their own");
}

#[test]
fn the_cards_offer_switch_default_sign_in_and_sign_out_where_they_apply() {
    let mut app = ready(true);
    three_accounts(&mut app);
    on(&mut app, Page::Accounts);
    let pressed = |app: &App, action: &str, label: &str| clicked_id(app, accounts::control_id(action, label));
    let offered = |app: &App, action: &str, label: &str| {
        let _one = exclusive();
        ui(app, SMALL).find(selector::id(accounts::control_id(action, label))).is_ok()
    };

    // The account in use, signed in and the default: only signing out applies.
    assert!(!offered(&app, "use", "default") && !offered(&app, "default", "default"));
    assert!(!offered(&app, "sign-in", "default"));
    assert!(
        matches!(pressed(&app, "sign-out", "default").as_slice(), [Message::SignOut { account }] if account == "default")
    );

    // Another account, not signed in.
    assert!(matches!(pressed(&app, "use", "alt").as_slice(), [Message::AccountSelected(label)] if label == "alt"));
    assert!(
        matches!(pressed(&app, "default", "alt").as_slice(), [Message::MakeDefaultAccount(label)] if label == "alt")
    );
    assert!(matches!(
        pressed(&app, "sign-in", "alt").as_slice(),
        [Message::SignIn { account, device_code: false }] if account == "alt"
    ));
    assert!(matches!(
        pressed(&app, "device-code", "alt").as_slice(),
        [Message::SignIn { account, device_code: true }] if account == "alt"
    ));
    assert!(!offered(&app, "sign-out", "alt"));

    // A third one, signed in.
    assert!(
        matches!(pressed(&app, "sign-out", "work-2").as_slice(), [Message::SignOut { account }] if account == "work-2")
    );
    assert!(!offered(&app, "sign-in", "work-2") && !offered(&app, "device-code", "work-2"));

    // While a game runs the controls are still drawn, and none of them does anything.
    launching(&mut app);
    for (action, label) in
        [("use", "alt"), ("default", "alt"), ("sign-in", "alt"), ("device-code", "alt"), ("sign-out", "work-2")]
    {
        assert!(offered(&app, action, label), "{action} on {label} is shown");
        assert!(pressed(&app, action, label).is_empty(), "{action} on {label} is not offered");
    }
}

#[test]
fn cards_side_by_side_are_equally_high_and_their_buttons_stand_on_the_same_lines() {
    // Where a token rests can take more lines to say for one account than for another (a
    // vault path against the keyring). The cards of one line end together all the same,
    // and their buttons stand on one line. The vault's text here is long enough to take
    // more lines than the keyring's in any text face, not only in the one installed here.
    let mut app = ready(true);
    let _ = app.update(Message::Refresh);
    let vault = "age vault (hardware-backed identity) at ~/.local/share/bastion-mc/vault, opened with a key \
                 that lives on a hardware token, which has to be plugged in to sign in";
    feed(
        &mut app,
        events([Event::Accounts {
            default: "default".into(),
            accounts: vec![
                named("alt", false),
                named("default", true),
                Account { player: None, store: vault.into(), ..named("work-2", true) },
            ],
        }]),
    );
    finish(&mut app, 0);
    on(&mut app, Page::Accounts);
    let find = |size: Size, action: &str, label: &str| -> iced::Rectangle {
        let _one = exclusive();
        ui(&app, size)
            .find(selector::id(accounts::control_id(action, label)))
            .unwrap_or_else(|e| panic!("{action} of {label} at {size:?}: {e}"))
            .bounds()
    };
    let level = |a: iced::Rectangle, b: iced::Rectangle| (a.y - b.y).abs() < 0.5;

    // At the size the window opens with, the three cards stand in one line.
    let (texts, _) = placed(&app, OPENING);
    let height_of = |start: &str| texts.iter().find(|text| text.content.starts_with(start)).unwrap().bounds.height;
    assert!(height_of("age vault") > height_of("kernel keyring") + 5.0, "the texts do differ in length");
    let cards = ["alt", "default", "work-2"].map(|label| find(OPENING, "card", label));
    for card in &cards[1..] {
        assert!(level(*card, cards[0]), "one line: {cards:?}");
        assert!((card.height - cards[0].height).abs() < 0.5, "equally high: {cards:?}");
    }
    assert!(level(find(OPENING, "use", "alt"), find(OPENING, "use", "work-2")), "switch to");
    assert!(level(find(OPENING, "default", "alt"), find(OPENING, "default", "work-2")), "make default");
    let session = [("sign-in", "alt"), ("sign-out", "default"), ("sign-out", "work-2")];
    let session = session.map(|(action, label)| find(OPENING, action, label));
    assert!(session.iter().all(|control| level(*control, session[0])), "sign in and out: {session:?}");

    // In two columns the first two cards make a line, and the third is the start of the
    // next one, beside the tile that adds a label.
    let (alt, default, work) =
        (find(SMALL, "card", "alt"), find(SMALL, "card", "default"), find(SMALL, "card", "work-2"));
    assert!(level(alt, default) && (alt.height - default.height).abs() < 0.5, "{alt:?} {default:?}");
    assert!(work.y > alt.y + alt.height, "a line of its own");
    assert!((work.width - alt.width).abs() < 0.5, "as wide as the cards above it");
}

#[test]
fn a_configured_device_flow_is_the_sign_in_and_no_client_means_no_sign_in() {
    let mut app = ready(false);
    on(&mut app, Page::Accounts);
    let _ = app.update(Message::Refresh);
    feed(&mut app, events([Event::Account(Account { device_flow: true, ..account(false) })]));
    finish(&mut app, 0);
    assert!(matches!(
        clicked(&app, &caps("Sign in")).as_slice(),
        [Message::SignIn { account, device_code: true }] if account == "default"
    ));
    assert!(!shows(&app, &caps("Device code")), "it already is the device-code flow");

    let _ = app.update(Message::Refresh);
    feed(&mut app, events([Event::Account(Account { client_id_configured: false, ..account(false) })]));
    finish(&mut app, 0);
    assert!(!shows(&app, &caps("Sign in")) && !shows(&app, &caps("Device code")));
    assert!(shows(&app, &caps("No OAuth client ID is configured")));
}

#[test]
fn an_account_is_added_by_its_label() {
    let mut app = ready(true);
    on(&mut app, Page::Accounts);
    assert!(clicked(&app, &caps("Add")).is_empty(), "nothing typed yet");
    let _ = app.update(Message::NewAccountName("Not Valid".into()));
    assert!(clicked(&app, &caps("Add")).is_empty());
    assert!(shows(&app, "Labels use a-z, 0-9, '.', '_' and '-', and start with a letter or digit."));
    let _ = app.update(Message::NewAccountName("alt".into()));
    assert!(matches!(clicked(&app, &caps("Add")).as_slice(), [Message::AddAccount]));
    let _ = app.update(Message::AddAccount);
    let all = texts(&app, SMALL);
    assert!(all.iter().any(|text| text == "alt"), "the new label has a card at once");
    assert!(all.iter().any(|text| text == "Checking…"), "and nothing is claimed about it yet");

    // Typing reaches the field.
    let _one = exclusive();
    let mut ui = ui(&app, SMALL);
    ui.click(selector::id(Id::new("new-account"))).expect("the field");
    ui.typewrite("b");
    assert!(
        matches!(ui.into_messages().collect::<Vec<_>>().as_slice(), [Message::NewAccountName(text)] if text == "b")
    );
}

#[test]
fn device_code_is_displayed_with_its_actions() {
    let mut app = ready(false);
    with_device_code(&mut app);
    assert!(shows(&app, "ABCD-EFGH"), "the code is its own piece of text, not buried in a sentence");
    assert!(shows(&app, "https://www.microsoft.com/link"));
    assert!(shows(&app, "The code expires in 15:00"));
    assert!(shows(&app, "Waiting for you to enter the code…"));
    assert!(matches!(clicked(&app, &caps("Copy code")).as_slice(), [Message::Copy(code)] if code == "ABCD-EFGH"));
    assert!(
        matches!(clicked(&app, &caps("Open link")).as_slice(), [Message::OpenLink(url)] if url == "https://www.microsoft.com/link")
    );
    assert!(matches!(clicked(&app, &caps("Cancel")).as_slice(), [Message::Stop]));
    assert!(matches!(clicked(&app, &caps("Cancel sign-in")).as_slice(), [Message::Stop]), "on the card as well");
    on(&mut app, Page::Launch);
    assert!(clicked(&app, &caps("Play")).is_empty(), "nothing else can be started meanwhile");
    on(&mut app, Page::Accounts);

    feed(&mut app, events([Event::Account(account(true))]));
    finish(&mut app, 0);
    assert!(!shows(&app, "ABCD-EFGH") && shows(&app, "plays as Alex") && shows(&app, "Signed in (default)."));
    assert!(
        matches!(clicked(&app, &caps("Sign out")).as_slice(), [Message::SignOut { account }] if account == "default")
    );
}

#[test]
fn a_browser_sign_in_offers_its_page_and_a_way_out() {
    let mut app = ready(false);
    on(&mut app, Page::Accounts);
    let _ = app.update(Message::SignIn { account: "default".into(), device_code: false });
    assert!(shows(&app, "Contacting Microsoft…"), "said before there is anything to act on");
    assert!(matches!(clicked(&app, &caps("Cancel")).as_slice(), [Message::Stop]), "and it can be abandoned already");
    let url = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?client_id=x&state=y";
    feed(&mut app, events([Event::BrowserLogin { url: url.into(), timeout_secs: 300 }]));
    assert!(matches!(clicked(&app, &caps("Open sign-in page")).as_slice(), [Message::OpenLink(to)] if to == url));
    assert!(matches!(clicked(&app, &caps("Copy link")).as_slice(), [Message::Copy(to)] if to == url));
    assert!(shows(&app, "Waiting for the browser, 5:00 left"));
}

// ------------------------------------------------------------------ links

#[test]
fn an_untrusted_link_gets_no_open_button() {
    let mut app = ready(false);
    on(&mut app, Page::Accounts);
    let _ = app.update(Message::SignIn { account: "default".into(), device_code: true });
    feed(
        &mut app,
        events([Event::DeviceCode {
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://not-microsoft.example/link".into(),
            expires_in: 900,
        }]),
    );
    assert!(shows(&app, &caps("Copy link")) && !shows(&app, &caps("Open link")));

    let mut app = ready(false);
    on(&mut app, Page::Accounts);
    let _ = app.update(Message::SignIn { account: "default".into(), device_code: false });
    feed(
        &mut app,
        events([Event::BrowserLogin {
            url: "https://login.microsoftonline.com.evil.example/".into(),
            timeout_secs: 300,
        }]),
    );
    assert!(shows(&app, &caps("Copy link")) && !shows(&app, &caps("Open sign-in page")));
}

/// Every caption of a control that hands an address to the desktop.
fn open_captions() -> [String; 3] {
    [caps("Open"), caps("Open link"), caps("Open sign-in page")]
}

#[test]
fn no_link_outside_the_two_allowlists_ever_gets_an_open_control() {
    let hostile = "https://evil.example/login?next=https://account.live.com/Activity";
    // An application in which every piece of text the launcher, the game or a file can
    // supply carries an address.
    let mut app = ready(true);
    let _ = app.update(Message::Refresh);
    feed(
        &mut app,
        events([
            Event::Account(Account {
                player: Some(hostile.into()),
                store: format!("keyring, see {hostile}"),
                ..account(true)
            }),
            Event::Instances {
                instances: vec![Instance {
                    id: "vanilla".into(),
                    minecraft: Some(hostile.into()),
                    kind: hostile.into(),
                    detail: Some(hostile.into()),
                    error: None,
                }],
            },
            Event::Status { text: hostile.into() },
            Event::Log { source: Source::Game, line: format!("click {hostile}") },
        ]),
    );
    feed(&mut app, events([Event::Error { message: format!("failed, visit {hostile}"), code: 1 }]));
    finish(&mut app, 1);
    launching(&mut app);
    feed(
        &mut app,
        events([
            Event::Protection {
                items: vec![Protection { aspect: hostile.into(), shield: Shield::Off, detail: hostile.into() }],
            },
            Event::Progress { label: hostile.into(), done: 1, total: 2, unit: Unit::Files },
            Event::Status { text: hostile.into() },
        ]),
    );
    let check = |app: &mut App, pages: &[Page]| {
        for page in pages {
            on(app, *page);
            let all = texts(app, SMALL);
            assert!(all.iter().any(|text| text.contains("evil.example")), "{page:?}: the fixture reaches this page");
            let opens = all.iter().filter(|text| open_captions().contains(text)).count();
            let expected = if *page == Page::Security { runbook::LINKS.len() } else { 0 };
            assert_eq!(opens, expected, "{page:?}");
        }
    };
    check(&mut app, &Page::ALL);
    // A scram report is the launcher's text too, and the sign-in prompt the launcher's link.
    confirm_scram(&mut app);
    scram_lines(&mut app, &[hostile, &format!("COULD NOT remove {hostile}")]);
    scram(&mut app, [ScramUpdate::Error(hostile.into()), ScramUpdate::Finished(ScramEnd::Exited(1))]);
    check(&mut app, &[Page::Security, Page::Console, Page::Launch]);

    // The controls that do exist name exactly the compiled-in addresses, in order…
    on(&mut app, Page::Security);
    for (index, (link, what)) in runbook::LINKS.iter().enumerate() {
        let _one = exclusive();
        // Tall enough for the whole page: a click needs its target on screen.
        let mut ui = ui(&app, Size::new(1000.0, 6000.0));
        assert!(ui.find(*what).is_ok() && ui.find(*link).is_ok(), "{what}: named and spelled out");
        ui.click(selector::id(security::open_id(index))).unwrap_or_else(|e| panic!("{what}: {e}"));
        let messages: Vec<Message> = ui.into_messages().collect();
        assert!(matches!(messages.as_slice(), [Message::OpenRunbookLink(url)] if url == link), "{what}: {messages:?}");
    }
    // …and what they send is checked against the list again before anything is opened.
    app.notice = None;
    let _ = app.update(Message::OpenRunbookLink(hostile.into()));
    assert!(app.notice.as_ref().is_some_and(|notice| notice.text.contains("was not opened")));
    app.notice = None;
    let _ = app.update(Message::OpenLink(hostile.into()));
    assert!(app.notice.as_ref().is_some_and(|notice| notice.text.contains("was not opened")));
}

// ------------------------------------------------------------------ instances

#[test]
fn instances_are_listed_and_selected_by_pressing_them() {
    let mut app = ready(true);
    with_instances(&mut app);
    on(&mut app, Page::Instances);
    let all = texts(&app, SMALL);
    for id in ["broken", "fab", "vanilla", "vanilla-latest"] {
        assert!(all.iter().any(|text| text == id), "{id}");
    }
    assert!(all.iter().any(|text| text == "Signed profile: serial 4, signed by me"));
    assert!(all.iter().any(|text| text == "manifest signature does not verify against any trusted key"));
    assert_eq!(all.iter().filter(|text| **text == caps("Selected")).count(), 1);
    assert!(all.contains(&caps("Cannot be launched")) && all.contains(&caps("curated")));

    assert!(matches!(
        clicked_id(&app, instances::entry_id("fab")).as_slice(),
        [Message::InstanceSelected(choice)] if choice.0.id == "fab"
    ));
    assert!(clicked_id(&app, instances::entry_id("vanilla")).is_empty(), "already selected");
    assert!(matches!(clicked(&app, &caps("Refresh")).as_slice(), [Message::Refresh]));
    assert!(matches!(clicked(&app, &caps("New instance")).as_slice(), [Message::ToggleNewInstance]));

    // The launch page offers the same choice as a list, and the way here.
    on(&mut app, Page::Launch);
    assert!(matches!(clicked(&app, &caps("Manage")).as_slice(), [Message::Navigate(Page::Instances)]));

    // While a launch is under way the list is shown, and pressing changes nothing.
    launching(&mut app);
    on(&mut app, Page::Instances);
    assert!(clicked_id(&app, instances::entry_id("fab")).is_empty());
    assert!(clicked(&app, &caps("New instance")).is_empty() && clicked(&app, &caps("Refresh")).is_empty());
    assert!(shows(&app, &caps("Instances — fixed while a launch is under way")));
}

#[test]
fn the_new_instance_form_checks_the_name_and_creates() {
    let mut app = ready(true);
    on(&mut app, Page::Instances);
    let _ = app.update(Message::ToggleNewInstance);
    assert!(shows(&app, &caps("Minecraft version")));
    feed(
        &mut app,
        events([Event::Versions {
            latest_release: "26.3".into(),
            latest_snapshot: "26.4-snapshot-8".into(),
            versions: vec![Version { id: "26.3".into(), kind: "release".into(), released: "2026-09-15".into() }],
        }]),
    );
    finish(&mut app, 0);
    assert_eq!(app.new_instance.version.as_ref().map(ToString::to_string).as_deref(), Some("26.3  (2026-09-15)"));
    assert!(clicked(&app, &caps("Create")).is_empty(), "no name yet");
    assert!(matches!(clicked(&app, "Snapshots").as_slice(), [Message::SnapshotsToggled(true)]));
    let _ = app.update(Message::NewInstanceId("vanilla".into()));
    assert!(shows(&app, "An instance with this name already exists.") && clicked(&app, &caps("Create")).is_empty());
    let _ = app.update(Message::NewInstanceId("Bad Name".into()));
    assert!(shows(&app, "Names use a-z, 0-9, '.', '_' and '-', and start with a letter or digit."));
    let _ = app.update(Message::NewInstanceId("survival".into()));
    assert!(matches!(clicked(&app, &caps("Create")).as_slice(), [Message::CreateInstance]));
    assert!(matches!(clicked(&app, &caps("Close form")).as_slice(), [Message::ToggleNewInstance]));
}

// ------------------------------------------------------------------ console

#[test]
fn the_console_is_only_built_on_its_own_page() {
    let mut app = ready(true);
    launching(&mut app);
    let line = "[Render thread/INFO]: Backend library: LWJGL";
    feed(&mut app, events([Event::Log { source: Source::Game, line: line.into() }]));
    for page in Page::ALL.into_iter().filter(|page| *page != Page::Console) {
        on(&mut app, page);
        assert!(!shows(&app, line), "{page:?}: the lines are not even built");
    }
    on(&mut app, Page::Console);
    assert!(shows(&app, line));
    assert!(shows(&app, "game"), "every line names its source");
    assert!(shows(&app, &caps("1 line")));
    assert!(matches!(clicked(&app, &caps("Copy all")).as_slice(), [Message::CopyConsole]));
    assert!(matches!(clicked(&app, &caps("Clear")).as_slice(), [Message::ClearConsole]));
    let _ = app.update(Message::ClearConsole);
    assert!(shows(&app, &caps("No output yet")));
    assert!(clicked(&app, &caps("Clear")).is_empty() && clicked(&app, &caps("Copy all")).is_empty());
}

// ------------------------------------------------------------------ notices

#[test]
fn errors_are_shown_on_every_page_and_can_be_dismissed() {
    let mut app = ready(true);
    let _ = app.update(Message::Play);
    feed(
        &mut app,
        events([Event::Error { message: "INTEGRITY VIOLATION: client jar does not match".into(), code: 4 }]),
    );
    finish(&mut app, 4);
    for page in Page::ALL {
        on(&mut app, page);
        assert!(shows(&app, "INTEGRITY VIOLATION: client jar does not match"), "{page:?}");
    }
    on(&mut app, Page::Launch);
    assert!(shows(&app, "The launch did not complete."));
    assert!(
        matches!(clicked(&app, &caps("Copy")).as_slice(), [Message::Copy(text)] if text.starts_with("INTEGRITY VIOLATION"))
    );
    assert!(matches!(clicked(&app, &caps("Dismiss")).as_slice(), [Message::DismissNotice]));
}

// ------------------------------------------------------------------ the emergency panel

#[test]
fn the_emergency_panel_asks_before_scram_and_offers_only_the_runbook_links() {
    let mut app = ready(true);
    assert!(!shows(&app, &caps("Scram…")), "on the security page only");
    on(&mut app, Page::Security);
    assert!(shows(&app, "Minecraft Support: submit a request"));
    assert!(shows(&app, "https://account.live.com/proofs/manage/additional"));
    // One press only asks.
    assert!(matches!(clicked(&app, &caps("Scram…")).as_slice(), [Message::AskScram]));
    assert!(!shows(&app, &caps("Yes, scram")));
    let _ = app.update(Message::AskScram);
    assert!(!shows(&app, &caps("Scram…")), "the first button is gone: the second press is a different one");
    assert!(matches!(clicked(&app, &caps("Yes, scram")).as_slice(), [Message::ConfirmScram]));
    assert!(matches!(clicked(&app, &caps("Cancel")).as_slice(), [Message::CancelScram]));
    let _ = app.update(Message::CancelScram);
    assert!(shows(&app, &caps("Scram…")) && !shows(&app, &caps("Yes, scram")));

    // The control can be reached without scrolling in the small window.
    let _one = exclusive();
    let target = ui(&app, SMALL).find(selector::id(security::scram_id("ask"))).expect("the scram control");
    let (all, seen) = (target.bounds(), target.visible_bounds().expect("on screen"));
    assert!((all.height - seen.height).abs() < 0.5 && (all.width - seen.width).abs() < 0.5, "{seen:?} of {all:?}");
}

#[test]
fn a_running_scram_says_that_it_is_still_running_and_shows_what_was_reported_so_far() {
    let mut app = scrammed(&REMOVED[..2], None);
    assert!(shows(&app, "Removing stored sign-ins… still running, 0:47"));
    assert!(shows(&app, REMOVED[0]) && shows(&app, REMOVED[1]), "the report appears as it arrives");
    for gone in ["Scram…", "Yes, scram", "Cancel"] {
        assert!(!shows(&app, &caps(gone)), "{gone:?}: a scram is neither asked for twice nor called off");
    }
    assert!(!shows(&app, &caps("Scram complete")) && !shows(&app, &caps("Scram incomplete")), "no verdict yet");
    let since = app.emergency.since.unwrap();
    let _ = app.update(Message::Tick(since + Duration::from_secs(20 * 60)));
    assert!(shows(&app, "Removing stored sign-ins… still running, 20:00"), "however long it takes");
    // It is said on the other pages too, and nothing that stores a token is offered there.
    on(&mut app, Page::Launch);
    assert!(shows(&app, &caps("Scram in progress")) && shows(&app, "A scram is in progress."));
    assert!(clicked(&app, &caps("Play")).is_empty());
}

#[test]
fn a_complete_scram_says_so_in_the_launchers_terms() {
    let app = scrammed(&REMOVED, Some(ScramEnd::Exited(0)));
    assert_eq!(app.emergency.outcome, Some(ScramOutcome::Complete));
    assert!(shows(&app, &caps("Scram complete")));
    assert!(shows(&app, REMOVED[1]) && shows(&app, REMOVED[5]), "with the whole report, the caveats included");
    assert!(shows(&app, &caps("Scram…")), "and it can be run again");
    assert!(!shows(&app, &caps("Scram incomplete")));
}

#[test]
fn an_incomplete_scram_is_said_plainly_with_what_is_still_there() {
    let lines: Vec<&str> = REMOVED[..4].iter().chain(&NOT_REMOVED).chain(&REMOVED[4..]).copied().collect();
    let mut app = scrammed(&lines, Some(ScramEnd::Exited(1)));
    for page in Page::ALL {
        on(&mut app, page);
        let all = texts(&app, SMALL);
        assert!(all.iter().any(|text| text.starts_with("SCRAM INCOMPLETE")), "{page:?}: said on every page");
        for text in &all {
            let claims = text.to_lowercase().replace("incomplete", "");
            assert!(
                !claims.contains("scram complete") && !claims.contains("scram done") && !claims.contains("scram: done"),
                "{page:?} reads as finished: {text:?}"
            );
        }
    }
    on(&mut app, Page::Security);
    assert!(shows(&app, &caps("Scram incomplete")) && !shows(&app, &caps("Scram complete")));
    assert!(shows(&app, "Not everything is confirmed removed: the launcher ended with status 1."));
    let all = texts(&app, SMALL);
    for line in NOT_REMOVED {
        assert_eq!(all.iter().filter(|text| *text == line).count(), 2, "set apart, and in its place in the report");
    }
    assert_eq!(app.notice.as_ref().map(|notice| notice.severity), Some(Severity::Error));
    on(&mut app, Page::Console);
    assert!(shows(&app, &format!("scram: {}", NOT_REMOVED[0])));
}

#[test]
fn a_scram_that_did_not_run_or_did_not_end_is_not_dressed_up() {
    let app = scrammed(&[], Some(ScramEnd::FailedToStart("No such file or directory (os error 2)".into())));
    assert!(shows(&app, &caps("Scram did not run")) && !shows(&app, &caps("Scram complete")));
    assert!(texts(&app, SMALL).iter().any(|text| text.contains("Nothing was removed")));

    let app = scrammed(&REMOVED[..2], Some(ScramEnd::StillRunning));
    assert!(shows(&app, &caps("Scram incomplete")));
    assert!(texts(&app, SMALL).iter().any(|text| text.contains("It was left running")), "what the limit means is said");
}

// ------------------------------------------------------------------ layout

/// What the layout checks look at: every text with where it is, and every scrolling area.
#[derive(Debug, Clone)]
struct Placed {
    content: String,
    bounds: iced::Rectangle,
    visible: Option<iced::Rectangle>,
}

fn placed(app: &App, size: Size) -> (Vec<Placed>, Vec<iced::Rectangle>) {
    let _one = exclusive();
    let found = Arc::new(Mutex::new((Vec::new(), Vec::new())));
    let collector = {
        let found = found.clone();
        move |candidate: Candidate<'_>| -> Option<()> {
            match candidate {
                Candidate::Text { content, bounds, visible_bounds, .. } if !content.trim().is_empty() => {
                    found.lock().unwrap().0.push(Placed {
                        content: content.to_string(),
                        bounds,
                        visible: visible_bounds,
                    });
                }
                Candidate::Scrollable { visible_bounds: Some(visible), .. } => found.lock().unwrap().1.push(visible),
                _ => {}
            }
            None
        }
    };
    let _ = ui(app, size).find(collector);
    found.lock().unwrap().clone()
}

/// The states whose layout is checked and drawn: every page, in every state that changes
/// what is on it.
fn states() -> Vec<(&'static str, App)> {
    let mut all: Vec<(&'static str, App)> = Vec::new();
    let mut add = |name: &'static str, app: App| all.push((name, app));

    add("launch-idle", ready(true));
    add("launch-blocked", ready(false));
    let mut demo = ready(false);
    let _ = demo.update(Message::DemoToggled(true));
    add("launch-demo", demo);

    let mut downloading = ready(true);
    launching(&mut downloading);
    feed(
        &mut downloading,
        events([
            Event::Status { text: "assets: 2114 of 5149 to download".into() },
            Event::Progress { label: "assets".into(), done: 163577856, total: 419430400, unit: Unit::Bytes },
        ]),
    );
    at(&mut downloading, Duration::from_millis(900));
    add("launch-downloading", downloading);

    let mut verifying = ready(true);
    launching(&mut verifying);
    feed(&mut verifying, events([Event::Phase { phase: Phase::Verifying }]));
    at(&mut verifying, Duration::from_millis(5300));
    add("launch-verifying", verifying);

    add("launch-running", running(MIXED));
    add("launch-running-protected", running(ALL_ON));
    let mut stopping = running(ALL_ON);
    let _ = stopping.update(Message::Stop);
    feed(&mut stopping, events([Event::Phase { phase: Phase::Stopping }]));
    add("launch-stopping", stopping);
    let mut ended = running(ALL_ON);
    feed(&mut ended, events([Event::Exited { code: 0 }]));
    finish(&mut ended, 0);
    finish(&mut ended, 0);
    add("launch-ended", ended);

    let mut failed = ready(true);
    let _ = failed.update(Message::Play);
    feed(
        &mut failed,
        events([Event::Error {
            message: "INTEGRITY VIOLATION: library lwjgl-3.3.6.jar: sha1 mismatch".into(),
            code: 4,
        }]),
    );
    finish(&mut failed, 4);
    add("launch-error", failed);

    // The longest message the launcher produces: it has to stay readable in the banner.
    let mut refused = ready(true);
    let _ = refused.update(Message::Play);
    feed(
        &mut refused,
        events([Event::Error {
            message: "policy violation: refusing to give the game the audio socket: the sound server accepts \
                      module-loading requests from its clients (it was asked). A sound server that loads modules \
                      for its clients lets a compromised game run code outside the sandbox. Choose one: (1) keep \
                      sound and close the hole — with PipeWire, create \
                      ~/.config/pipewire/pipewire-pulse.conf.d/90-no-module-loading.conf containing \
                      `pulse.properties = { pulse.allow-module-loading = false }` and run `systemctl --user \
                      restart pipewire-pulse`; (2) play without sound: `audio = \"none\"` under [sandbox]; (3) \
                      accept the risk: `allow_pulse_module_loading = true` under [sandbox]."
                .into(),
            code: 5,
        }]),
    );
    finish(&mut refused, 5);
    add("launch-refused", refused);

    // A launcher of the future that names more aspects than this build knows, at length.
    let mut many = ready(true);
    launching(&mut many);
    feed(
        &mut many,
        events([
            Event::Phase { phase: Phase::Starting },
            Event::Protection {
                items: (0..12)
                    .map(|i| Protection {
                        aspect: format!("aspect-number-{i}-of-many"),
                        shield: [Shield::On, Shield::Partial, Shield::Off][i % 3],
                        detail: "a description that goes on for as long as the launcher is allowed to make one "
                            .repeat(4),
                    })
                    .collect(),
            },
            Event::Status { text: "a status line that says a great deal about very little ".repeat(8) },
            Event::Phase { phase: Phase::Running },
        ]),
    );
    add("launch-many-aspects", many);

    // The longest names the launcher accepts, everywhere a name is shown.
    let long_label = format!("an-account-label-{}", "x".repeat(47));
    let long_instance = format!("an-instance-{}", "y".repeat(52));
    let mut long = App::boot(PathBuf::from("/nonexistent/bastion-mc"), long_label.clone()).0;
    feed(
        &mut long,
        events([
            Event::Hello { protocol: bastion_mc_proto::PROTOCOL, version: "0.1.0-with-a-long-suffix".into() },
            Event::Account(Account { player: Some("W".repeat(16)), ..named(&long_label, true) }),
            Event::Accounts {
                default: long_label.clone(),
                accounts: vec![
                    Account { player: Some("W".repeat(16)), ..named(&long_label, true) },
                    Account { store: "a store description ".repeat(12), ..named("b", false) },
                ],
            },
            Event::Instances {
                instances: vec![
                    Instance {
                        id: long_instance.clone(),
                        minecraft: Some("26.4-snapshot-12-experimental".into()),
                        kind: "curated".into(),
                        detail: Some("serial 18446744073709551615, signed by a-key-with-a-long-name ".repeat(3)),
                        error: None,
                    },
                    instance("z", Some(&"this instance cannot be launched, and here is why, at length: ".repeat(6))),
                ],
            },
        ]),
    );
    finish(&mut long, 0);
    add("launch-long-names", long);
    let relabelled = |page: Page| {
        let mut app = App::boot(PathBuf::from("/nonexistent/bastion-mc"), long_label.clone()).0;
        feed(
            &mut app,
            events([
                Event::Account(Account { player: Some("W".repeat(16)), ..named(&long_label, true) }),
                Event::Accounts {
                    default: long_label.clone(),
                    accounts: vec![
                        Account { player: Some("W".repeat(16)), ..named(&long_label, true) },
                        Account { store: "a store description ".repeat(12), ..named("b", false) },
                    ],
                },
                Event::Instances {
                    instances: vec![
                        Instance {
                            id: long_instance.clone(),
                            minecraft: Some("26.4-snapshot-12-experimental".into()),
                            kind: "curated".into(),
                            detail: Some("serial 18446744073709551615, signed by a-key-with-a-long-name ".repeat(3)),
                            error: None,
                        },
                        instance(
                            "z",
                            Some(&"this instance cannot be launched, and here is why, at length: ".repeat(6)),
                        ),
                    ],
                },
            ]),
        );
        finish(&mut app, 0);
        on(&mut app, page);
        app
    };
    add("accounts-long-names", relabelled(Page::Accounts));
    add("instances-long-names", relabelled(Page::Instances));

    let mut accounts = ready(true);
    three_accounts(&mut accounts);
    on(&mut accounts, Page::Accounts);
    add("accounts", accounts);
    let mut single = ready(false);
    on(&mut single, Page::Accounts);
    add("accounts-one-signed-out", single);
    // A second account is being signed in to while the first stays in use.
    let mut code = ready(true);
    three_accounts(&mut code);
    on(&mut code, Page::Accounts);
    let _ = code.update(Message::SignIn { account: "alt".into(), device_code: true });
    feed(
        &mut code,
        events([Event::DeviceCode {
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://www.microsoft.com/link".into(),
            expires_in: 900,
        }]),
    );
    add("accounts-device-code", code);
    let mut browser = ready(false);
    on(&mut browser, Page::Accounts);
    let _ = browser.update(Message::SignIn { account: "default".into(), device_code: false });
    feed(
        &mut browser,
        events([Event::BrowserLogin {
            url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?client_id=x".into(),
            timeout_secs: 300,
        }]),
    );
    add("accounts-browser", browser);
    let mut no_client = ready(false);
    on(&mut no_client, Page::Accounts);
    let _ = no_client.update(Message::Refresh);
    feed(&mut no_client, events([Event::Account(Account { client_id_configured: false, ..account(false) })]));
    finish(&mut no_client, 0);
    let _ = no_client.update(Message::NewAccountName("Not Valid".into()));
    add("accounts-no-client", no_client);
    let mut locked = running(ALL_ON);
    three_accounts_while_busy(&mut locked);
    on(&mut locked, Page::Accounts);
    add("accounts-while-running", locked);

    let mut instances = ready(true);
    with_instances(&mut instances);
    on(&mut instances, Page::Instances);
    add("instances", instances);
    let mut form = ready(true);
    with_instances(&mut form);
    on(&mut form, Page::Instances);
    let _ = form.update(Message::ToggleNewInstance);
    feed(
        &mut form,
        events([Event::Versions {
            latest_release: "26.3".into(),
            latest_snapshot: "26.4-snapshot-8".into(),
            versions: vec![Version { id: "26.3".into(), kind: "release".into(), released: "2026-09-15".into() }],
        }]),
    );
    finish(&mut form, 0);
    let _ = form.update(Message::NewInstanceId("vanilla".into()));
    add("instances-new", form);
    let mut none = ready(true);
    let _ = none.update(Message::Refresh);
    feed(&mut none, events([Event::Instances { instances: vec![] }]));
    finish(&mut none, 0);
    on(&mut none, Page::Instances);
    add("instances-none", none);

    let mut console = running(MIXED);
    on(&mut console, Page::Console);
    add("console", console);
    let mut quiet = ready(true);
    on(&mut quiet, Page::Console);
    add("console-empty", quiet);

    let mut security = ready(true);
    on(&mut security, Page::Security);
    add("security", security);
    let mut after = running(MIXED);
    on(&mut after, Page::Security);
    add("security-running", after);
    let mut asking = ready(true);
    on(&mut asking, Page::Security);
    let _ = asking.update(Message::AskScram);
    add("security-scram-asking", asking);
    add("security-scram-running", scrammed(&REMOVED[..3], None));
    add("security-scram-complete", scrammed(&REMOVED, Some(ScramEnd::Exited(0))));
    let lines: Vec<&str> = REMOVED[..4].iter().chain(&NOT_REMOVED).chain(&REMOVED[4..]).copied().collect();
    add("security-scram-incomplete", scrammed(&lines, Some(ScramEnd::Exited(1))));
    add(
        "security-scram-not-run",
        scrammed(&[], Some(ScramEnd::FailedToStart("No such file or directory (os error 2)".into()))),
    );
    all.extend(unbroken_states());
    all
}

/// One word with nowhere to break it, made of the widest letter there is.
fn unbroken(chars: usize) -> String {
    "W".repeat(chars)
}

/// States in which every text the launcher supplies is far longer than anything it sends
/// and has nowhere to break — what a launcher that is wrong, or was lied to by a server,
/// could put on its output. Every page, in the states that show such text. Nothing in
/// them may leave its place: that is what the layout tests hold them to.
fn unbroken_states() -> Vec<(&'static str, App)> {
    let long = unbroken(3000);
    let label = "w".repeat(64);
    let other = "n".repeat(64);
    let broken = "x".repeat(64);
    let account = |label: &str, signed_in: bool| Account {
        account: label.into(),
        signed_in,
        player: Some(long.clone()),
        store: long.clone(),
        client_id_configured: true,
        device_flow: false,
    };
    let base = || {
        let mut app = App::boot(PathBuf::from("/nonexistent/bastion-mc"), label.clone()).0;
        feed(
            &mut app,
            events([
                Event::Hello { protocol: bastion_mc_proto::PROTOCOL, version: long.clone() },
                Event::Account(account(&label, true)),
                Event::Accounts {
                    default: label.clone(),
                    accounts: vec![account(&label, true), account(&other, false)],
                },
                Event::Instances {
                    instances: vec![
                        Instance {
                            id: "m".repeat(64),
                            minecraft: Some(long.clone()),
                            kind: long.clone(),
                            detail: Some(long.clone()),
                            error: None,
                        },
                        Instance {
                            id: broken.clone(),
                            minecraft: Some(long.clone()),
                            kind: long.clone(),
                            detail: None,
                            error: Some(long.clone()),
                        },
                    ],
                },
                Event::Status { text: long.clone() },
            ]),
        );
        finish(&mut app, 0);
        app
    };
    let protection = Event::Protection {
        items: (0..12)
            .map(|i| Protection {
                aspect: long.clone(),
                shield: [Shield::On, Shield::Partial, Shield::Off][i % 3],
                detail: long.clone(),
            })
            .collect(),
    };
    let mut all: Vec<(&'static str, App)> = Vec::new();

    all.push(("unbroken-launch", base()));
    let mut unusable = base();
    let choice = unusable.instances.iter().find(|choice| choice.0.id == broken).cloned().expect("the instance");
    let _ = unusable.update(Message::InstanceSelected(choice));
    all.push(("unbroken-launch-unusable", unusable));

    for (name, unit) in [("unbroken-launch-bytes", Unit::Bytes), ("unbroken-launch-files", Unit::Files)] {
        let mut running = base();
        launching(&mut running);
        feed(
            &mut running,
            events([
                Event::Progress { label: long.clone(), done: u64::MAX - 1, total: u64::MAX, unit },
                Event::Status { text: long.clone() },
                protection.clone(),
            ]),
        );
        all.push((name, running));
    }

    let mut failed = base();
    let _ = failed.update(Message::Play);
    feed(&mut failed, events([protection.clone(), Event::Error { message: long.clone(), code: 1 }]));
    finish(&mut failed, 1);
    all.push(("unbroken-launch-error", failed));

    let mut accounts = base();
    on(&mut accounts, Page::Accounts);
    let _ = accounts.update(Message::NewAccountName(long.clone()));
    all.push(("unbroken-accounts", accounts));
    let mut code = base();
    on(&mut code, Page::Accounts);
    let _ = code.update(Message::SignIn { account: other.clone(), device_code: true });
    feed(
        &mut code,
        events([
            Event::Status { text: long.clone() },
            Event::DeviceCode {
                user_code: long.clone(),
                verification_uri: format!("https://{long}"),
                expires_in: u64::MAX,
            },
        ]),
    );
    all.push(("unbroken-accounts-code", code));
    let mut browser = base();
    on(&mut browser, Page::Accounts);
    let _ = browser.update(Message::SignIn { account: other.clone(), device_code: false });
    // The launcher's words after the prompt: they are what the status line then shows.
    feed(
        &mut browser,
        events([
            Event::BrowserLogin { url: format!("https://{long}"), timeout_secs: u64::MAX },
            Event::Status { text: long.clone() },
        ]),
    );
    assert!(browser.status.starts_with("WWWW"), "the status line is part of this state");
    all.push(("unbroken-accounts-browser", browser));

    let mut instances = base();
    on(&mut instances, Page::Instances);
    let _ = instances.update(Message::ToggleNewInstance);
    feed(
        &mut instances,
        events([Event::Versions {
            latest_release: long.clone(),
            latest_snapshot: long.clone(),
            versions: vec![
                Version { id: unbroken(64), kind: long.clone(), released: long.clone() },
                Version { id: "26.3".into(), kind: "release".into(), released: "2026-09-15".into() },
            ],
        }]),
    );
    finish(&mut instances, 0);
    let _ = instances.update(Message::NewInstanceId(long.clone()));
    all.push(("unbroken-instances", instances));

    let mut console = base();
    launching(&mut console);
    feed(
        &mut console,
        events([
            Event::Log { source: Source::Game, line: long.clone() },
            Event::Log { source: Source::Egress, line: long.clone() },
            Event::Status { text: long.clone() },
        ]),
    );
    on(&mut console, Page::Console);
    all.push(("unbroken-console", console));

    let mut security = base();
    launching(&mut security);
    feed(&mut security, events([protection.clone()]));
    on(&mut security, Page::Security);
    confirm_scram(&mut security);
    scram_lines(&mut security, &[&long, &format!("COULD NOT remove {long}")]);
    scram(&mut security, [ScramUpdate::Error(long.clone()), ScramUpdate::Finished(ScramEnd::Exited(1))]);
    all.push(("unbroken-security", security));
    all
}

/// The account list arriving while a game runs (the launcher reports it with every command).
fn three_accounts_while_busy(app: &mut App) {
    feed(
        app,
        events([Event::Accounts {
            default: "default".into(),
            accounts: vec![named("alt", false), named("default", true), named("work-2", true)],
        }]),
    );
}

#[test]
fn nothing_is_clipped_overlapping_or_out_of_reach_at_any_size() {
    let states = states();
    for size in [SMALL, LARGE, OPENING, SMALLEST] {
        for (name, app) in &states {
            let (texts, scrolling) = placed(app, size);
            assert!(texts.len() >= 8, "{name} at {size:?}: the page was laid out ({} texts)", texts.len());
            let window = iced::Rectangle::with_size(size);
            for text in &texts {
                let what = format!("{name} at {}x{}: {:?}", size.width, size.height, text.content);
                assert!(text.bounds.width > 0.5 && text.bounds.height > 0.5, "{what} was squeezed to nothing");
                // Sideways nothing scrolls, so nothing may be cut off sideways.
                assert!(
                    text.bounds.x >= -0.5 && text.bounds.x + text.bounds.width <= size.width + 0.5,
                    "{what} leaves the window sideways: {:?}",
                    text.bounds
                );
                // What starts in the rail ends in the rail. (This holds the *place* of a
                // text to the rail. Letters that are too many for their place are another
                // matter, which bounds cannot show: see `paragraphs`.)
                let rail = Layout::rail_width(size.width < Layout::COMPACT_BELOW);
                assert!(
                    text.bounds.x >= rail || text.bounds.x + text.bounds.width <= rail,
                    "{what} runs out of the rail into the stage: {:?}",
                    text.bounds
                );
                match text.visible {
                    Some(visible) => {
                        assert!(
                            (visible.width - text.bounds.width).abs() < 1.0,
                            "{what} is cut off sideways: {:?} of {:?}",
                            visible,
                            text.bounds
                        );
                        // Cut off above or below is fine exactly where the area scrolls.
                        if (visible.height - text.bounds.height).abs() >= 1.0 {
                            assert!(
                                scrolling.iter().any(|area| {
                                    (visible.y - area.y).abs() < 1.0
                                        || (visible.y + visible.height - area.y - area.height).abs() < 1.0
                                }),
                                "{what} is cut off, and not by the edge of a scrolling area"
                            );
                        }
                    }
                    // Not visible at all: it has to be inside something that scrolls to it.
                    None => assert!(
                        window.intersection(&text.bounds).is_none() || !scrolling.is_empty(),
                        "{what} is hidden"
                    ),
                }
            }
            // No two pieces of text that are on screen lie on top of each other.
            let visible: Vec<&Placed> = texts.iter().filter(|text| text.visible.is_some()).collect();
            for (index, a) in visible.iter().enumerate() {
                for b in &visible[index + 1..] {
                    if let Some(overlap) = a.visible.unwrap().intersection(&b.visible.unwrap()) {
                        assert!(
                            overlap.width < 1.5 || overlap.height < 1.5,
                            "{name} at {}x{}: {:?} and {:?} overlap by {overlap:?}",
                            size.width,
                            size.height,
                            a.content,
                            b.content
                        );
                    }
                }
            }
        }
    }
}

/// A paragraph of text as it was laid out: the room it was given, and the room its words
/// take.
#[derive(Debug)]
struct Measure {
    content: String,
    given: Size,
    needs: Size,
}

/// Every paragraph of a text widget in the window, on screen or scrolled away.
///
/// This is what the selector cannot tell: it reports the *bounds* of a text, and the
/// toolkit never makes those larger than the room there is — a caption that is too long
/// for its place has bounds that fit and letters that do not. The paragraph itself knows
/// both sizes. It is found in the widget's state, after a layout pass done by hand.
///
/// Not covered: text that a widget draws itself (the label of a drop-down, the value of an
/// input field). Those are bounded by construction and have tests of their own.
fn paragraphs(app: &App, size: Size) -> Vec<Measure> {
    use iced_test::core::{
        layout,
        renderer::Headless,
        text::{Paragraph, Renderer, paragraph},
        widget::{Tree, tree},
    };
    type Laid = <iced::Renderer as Renderer>::Paragraph;

    fn walk(tree: &Tree, found: &mut Vec<Measure>) {
        if tree.tag == tree::Tag::of::<paragraph::Plain<Laid>>() {
            let paragraph = tree.state.downcast_ref::<paragraph::Plain<Laid>>();
            let given = paragraph.raw().bounds();
            // The paragraph was laid out within the height it was given, and only knows
            // the lines that fitted. Laid out again without that limit, it says how high
            // all of its lines are.
            let unbounded = Laid::with_text(iced_test::core::text::Text {
                bounds: Size::new(given.width, f32::INFINITY),
                ..paragraph.as_text()
            });
            let needs = Size::new(paragraph.min_width().max(unbounded.min_width()), unbounded.min_height());
            if !paragraph.content().trim().is_empty() {
                found.push(Measure { content: paragraph.content().to_string(), given, needs });
            }
        }
        for child in &tree.children {
            walk(child, found);
        }
    }

    let _one = exclusive();
    let renderer: iced::Renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        theme::UI_FONT,
        iced::Pixels(16.0),
        Some("tiny-skia"),
    ))
    .expect("the software renderer");
    let mut window = root(app);
    let mut tree = Tree::new(window.as_widget());
    let _ = window.as_widget_mut().layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size));
    let mut found = Vec::new();
    walk(&tree, &mut found);
    found
}

/// The sizes the measuring test lays every state out at: two that are smaller than the
/// window lets itself be made (a tiling window manager does not ask), the smallest it does,
/// both sides of the width at which the rail folds, the sizes the other tests use, and
/// steps between and beyond them.
///
/// Below about 480 wide the headings of some panels are longer than the panels are wide;
/// that is where this stops.
const WIDTHS: [(f32, f32); 13] = [
    (480.0, 400.0),
    (560.0, 460.0),
    (640.0, 520.0),
    (720.0, 600.0),
    (799.0, 700.0),
    (800.0, 700.0),
    (880.0, 720.0),
    (940.0, 720.0),
    (1000.0, 720.0),
    (1100.0, 720.0),
    (1180.0, 720.0),
    (1280.0, 800.0),
    (1600.0, 900.0),
];

#[test]
fn no_text_needs_more_room_than_it_was_given_at_any_width() {
    let states = states();
    let (mut seen, mut too_large) = (0, Vec::new());
    for (width, height) in WIDTHS {
        let size = Size::new(width, height);
        for (name, app) in &states {
            let all = paragraphs(app, size);
            assert!(all.len() >= 8, "{name} at {width}x{height}: the page was laid out ({} paragraphs)", all.len());
            seen += all.len();
            for text in &all {
                let shown: String = text.content.chars().filter(|c| *c != '\u{2006}').take(60).collect();
                // A line that was broken to fit can come out a fraction of a pixel wider
                // than the room (the space it was broken at); a line that does not fit is
                // wider by a letter or more.
                if text.needs.width > text.given.width + 1.0 {
                    too_large.push(format!(
                        "{name} at {width}x{height}: {shown:?} is {} wide in a place {} wide",
                        text.needs.width, text.given.width
                    ));
                }
                if text.needs.height > text.given.height + 0.5 {
                    too_large.push(format!(
                        "{name} at {width}x{height}: {shown:?} is {} high in a place {} high",
                        text.needs.height, text.given.height
                    ));
                }
            }
        }
    }
    assert!(seen > 10_000, "the paragraphs were found and measured: {seen}");
    assert!(too_large.is_empty(), "{} texts overextend:\n{}", too_large.len(), too_large.join("\n"));
}

#[test]
fn the_installed_monospace_face_is_no_wider_than_the_layout_takes_it_to_be() {
    use iced_test::core::text::{Paragraph, Renderer, Text};
    type Laid = <iced::Renderer as Renderer>::Paragraph;
    // Text that has to fit on one line (a name in a drop-down, in the rail) is cut to a
    // number of characters, worked out from this figure. A face that is wider would make
    // such a line longer than its place.
    let _one = exclusive();
    for size in [theme::SIZE_LABEL, theme::SIZE_SMALL, theme::SIZE_SMALL + 1.0, theme::SIZE_DISPLAY] {
        for sample in ["W".repeat(40), "an-instance_0.9".repeat(3), format!("{}…", "m".repeat(39))] {
            let line = Laid::with_text(Text {
                content: sample.as_str(),
                bounds: Size::new(f32::INFINITY, f32::INFINITY),
                size: iced::Pixels(size),
                line_height: Default::default(),
                font: theme::MONO,
                align_x: Default::default(),
                align_y: iced::alignment::Vertical::Top,
                shaping: Default::default(),
                wrapping: text::Wrapping::None,
            });
            let (width, chars) = (line.min_width(), sample.chars().count());
            assert!(width > 0.3 * size * chars as f32, "{sample:?} was measured: {width}");
            assert!(
                width <= kit::MONO_ADVANCE * size * chars as f32 + 0.5,
                "{chars} characters at {size} are {width} wide: more than {} each",
                kit::MONO_ADVANCE * size
            );
            assert!(kit::mono_chars(width, size) <= chars, "never more characters than the room has been seen to hold");
        }
    }
    assert_eq!(kit::mono_chars(-5.0, 13.0), 0, "no room, no characters");
}

#[test]
fn the_launch_page_fits_the_window_it_opens_in_and_nothing_on_it_moves() {
    // Without a notice above it, the page needs no scrolling at the size the window opens
    // with — nor in a window a good deal smaller — whatever state a launch is in.
    let states = states();
    let fitting = ["launch-idle", "launch-blocked", "launch-demo", "launch-downloading", "launch-verifying"]
        .into_iter()
        .chain(["launch-running", "launch-running-protected", "launch-stopping", "launch-ended"]);
    for name in fitting {
        let app = &states.iter().find(|(state, _)| *state == name).unwrap_or_else(|| panic!("no state {name}")).1;
        assert!(app.notice.is_none(), "{name}");
        for size in [OPENING, SMALL] {
            let (texts, _) = placed(app, size);
            for text in &texts {
                let whole = text.visible.is_some_and(|visible| (visible.height - text.bounds.height).abs() < 0.5);
                assert!(whole, "{name} at {size:?}: {:?} is not on screen in full: {:?}", text.content, text);
            }
        }
    }

    // Pressing the launch control moves nothing: what is under the gauge and the panels
    // around it are where they were, at both sizes, from idle to a running game.
    let landmarks = [caps("Stop"), caps("Protection"), caps("Sequence"), caps("Instance"), caps("Game")];
    for size in [SMALL, OPENING, LARGE] {
        let place = |name: &str| -> Vec<iced::Rectangle> {
            let app = &states.iter().find(|(state, _)| *state == name).unwrap().1;
            let (texts, _) = placed(app, size);
            landmarks
                .iter()
                .map(|mark| {
                    let found = texts.iter().find(|text| text.content == *mark);
                    found.unwrap_or_else(|| panic!("{name}: no {mark:?}")).bounds
                })
                .collect()
        };
        let idle = place("launch-idle");
        for name in ["launch-downloading", "launch-verifying", "launch-running-protected", "launch-stopping"] {
            for ((mark, before), now) in landmarks.iter().zip(&idle).zip(place(name)) {
                let moved = (before.x - now.x).abs().max((before.y - now.y).abs());
                assert!(moved < 0.5, "{name} at {size:?}: {mark:?} moved from {before:?} to {now:?}");
            }
        }
    }
}

#[test]
fn the_launchers_words_under_the_gauge_are_bounded_and_the_console_has_them_in_full() {
    let mut app = ready(true);
    launching(&mut app);
    let said = "display: X11 :0 — WARNING: X11 lets the game observe and inject input into every other X client; \
                this is a sandbox escape path. Prefer a Wayland session.";
    feed(&mut app, events([Event::Status { text: said.into() }]));
    let shown = texts(&app, SMALL).into_iter().find(|text| text.starts_with("display: X11")).expect("the status");
    assert!(shown.chars().count() <= 97 && shown.ends_with('…'), "{shown:?}");
    on(&mut app, Page::Console);
    assert!(shows(&app, said), "nothing is lost: the console has the line as the launcher said it");

    // Idle, the same place says what would be launched, and how.
    let mut demo = ready(false);
    let _ = demo.update(Message::DemoToggled(true));
    assert!(shows(&demo, "Ready: vanilla, demo mode."));
}

/// Draw `app` the way the window would, in software, and return the pixels (RGBA).
fn render(app: &App, size: Size, scale: f32) -> (Vec<u8>, Size<u32>) {
    use iced_test::{
        core::{Event as Input, clipboard, mouse, renderer::Headless, window},
        runtime::{UserInterface, user_interface},
    };
    let _one = exclusive();
    let mut renderer: iced::Renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        theme::UI_FONT,
        iced::Pixels(16.0),
        Some("tiny-skia"),
    ))
    .expect("the software renderer");
    let mut interface = UserInterface::build(root(app), size, user_interface::Cache::default(), &mut renderer);
    let mut messages = Vec::new();
    let _ = interface.update(
        &[Input::Window(window::Event::RedrawRequested(std::time::Instant::now()))],
        mouse::Cursor::Unavailable,
        &mut renderer,
        &mut clipboard::Null,
        &mut messages,
    );
    interface.draw(
        &mut renderer,
        &theme::theme(),
        &iced_test::core::renderer::Style { text_color: theme::TEXT },
        mouse::Cursor::Unavailable,
    );
    let pixels = Size::new((size.width * scale).round() as u32, (size.height * scale).round() as u32);
    (renderer.screenshot(pixels, scale, theme::VOID), pixels)
}

fn write_png(path: &std::path::Path, rgba: &[u8], size: Size<u32>) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, size.width, size.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_compression(png::Compression::Fast);
    encoder.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// Draw `app` at `size` into `<name>-<width>x<height>.png` and check the one visual property
/// that can be stated objectively: the window is dark, with light text on it.
fn snapshot(dir: &std::path::Path, name: &str, app: &App, size: Size, scale: f32) {
    let name = format!("{name}-{}x{}", size.width, size.height);
    let (rgba, pixels) = render(app, size, scale);
    write_png(&dir.join(format!("{name}.png")), &rgba, pixels);

    let luma: Vec<u32> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000)
        .collect();
    assert_eq!(luma.len(), (pixels.width * pixels.height) as usize, "{name}");
    let mean = luma.iter().sum::<u32>() / luma.len() as u32;
    let dark = luma.iter().filter(|l| **l < 64).count() * 100 / luma.len();
    let bright = luma.iter().filter(|l| **l > 160).count();
    // A page filled edge to edge with the widest letter there is has more ink on it than
    // any real one.
    let least_dark = if name.starts_with("unbroken") { 60 } else { 85 };
    assert!(mean < 60, "{name}: mean luminance {mean} is not a dark theme");
    assert!(dark >= least_dark, "{name}: only {dark}% of the window is dark");
    assert!(bright > 200, "{name}: no light text was drawn ({bright} bright pixels)");
}

/// Render every state in software, at both sizes. The images are left in
/// `target/gui-snapshots/` for a human to look at; they are not compared to a baseline
/// (font rendering differs between machines).
///
/// Some are drawn again as `whole-…` (a page that scrolls, in a window tall enough for all
/// of it), `smallest-…` (the smallest the window can be made), `opening-…` (the size it
/// opens with) and `between-…` (1000 and 1100 wide, where the arrangements change).
///
/// `BASTION_GUI_SNAPSHOT_SCALE=2` writes them at twice the resolution (what a high-density
/// display shows); the default is one pixel per unit.
#[test]
fn every_screen_renders_dark() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/gui-snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    for stale in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        if stale.path().extension().is_some_and(|extension| extension == "png") {
            std::fs::remove_file(stale.path()).unwrap();
        }
    }
    let scale = std::env::var("BASTION_GUI_SNAPSHOT_SCALE").ok().and_then(|scale| scale.parse().ok()).unwrap_or(1.0);
    let scale = f32::clamp(scale, 1.0, 3.0);
    let states = states();
    for (name, app) in &states {
        for size in [SMALL, LARGE] {
            snapshot(&dir, name, app, size, scale);
        }
    }
    assert!(states.len() >= 28, "every page in every state that changes it: {}", states.len());

    // Some states once more: the pages that scroll in a window tall enough for all of them,
    // the smallest window, and the size the window opens with.
    let state = |wanted: &str| &states.iter().find(|(name, _)| *name == wanted).expect("a state of that name").1;
    for (view, of, size) in [
        ("whole", "security-scram-incomplete", Size::new(880.0, 3900.0)),
        ("whole", "security-scram-incomplete", Size::new(1280.0, 2500.0)),
        ("whole", "accounts-device-code", Size::new(880.0, 1100.0)),
        ("whole", "launch-refused", Size::new(880.0, 980.0)),
        ("smallest", "launch-running", SMALLEST),
        ("smallest", "security", SMALLEST),
        ("opening", "launch-idle", OPENING),
        ("opening", "launch-running", OPENING),
        ("opening", "accounts", OPENING),
        ("opening", "instances", OPENING),
        ("opening", "console", OPENING),
        ("opening", "security-running", OPENING),
        // Between the small and the large window, where the arrangements change.
        ("between", "launch-idle", Size::new(1000.0, 720.0)),
        ("between", "launch-running-protected", Size::new(1000.0, 720.0)),
        ("between", "launch-running-protected", Size::new(1100.0, 720.0)),
        ("between", "launch-downloading", Size::new(1100.0, 720.0)),
        ("between", "accounts", Size::new(1000.0, 720.0)),
        ("between", "accounts", Size::new(1100.0, 720.0)),
        ("between", "accounts-device-code", Size::new(1100.0, 720.0)),
        ("between", "instances-new", Size::new(1000.0, 720.0)),
        ("between", "security-running", Size::new(1000.0, 720.0)),
        ("between", "security-scram-incomplete", Size::new(1100.0, 720.0)),
        ("smallest", "launch-idle", SMALLEST),
        ("smallest", "accounts", SMALLEST),
        ("smallest", "instances-new", SMALLEST),
        ("smallest", "console", SMALLEST),
        ("smallest", "unbroken-launch-unusable", SMALLEST),
    ] {
        snapshot(&dir, &format!("{view}-{of}"), state(of), size, scale);
    }
}

#[test]
fn killed_and_failed_launchers_do_not_leave_the_view_in_a_state_it_cannot_draw() {
    // States the fixtures above do not reach: they only have to lay out.
    let mut app = ready(true);
    launching(&mut app);
    feed(&mut app, [Update::Finished(Outcome::Killed)]);
    assert!(shows(&app, "The launcher was killed before it finished."));
    let (empty, _task) = App::boot(PathBuf::from("/nonexistent/bastion-mc"), String::new());
    for page in Page::ALL {
        let mut app = App::new(PathBuf::from("/nonexistent/bastion-mc"), String::new());
        on(&mut app, page);
        assert!(texts(&app, SMALL).len() > 5, "{page:?} with nothing known at all");
    }
    assert!(shows(&empty, "launcher not heard from yet"));
}
