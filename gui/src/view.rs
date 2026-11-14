//! The view: a pure function from [`App`] to widgets.
//!
//! Nothing here changes state or does work. Every interaction is a [`Message`]; whether a
//! control is enabled is decided by asking the state (`can_play`, `busy`, …), so the rules
//! live in one place and are tested without a window.

use std::time::Instant;

use bastion_mc_proto::{Phase, Shield, Source, runbook};
use iced::{
    Center, Element, Fill, Font, Length, Theme,
    widget::{
        button, checkbox, column, container, pick_list, progress_bar, responsive, row, scrollable, space, text,
        text_input,
    },
};

use crate::{
    app::{App, JobKind, LoginPrompt, Message, Notice, is_sign_in_link, is_valid_id},
    console::{Level, Line},
    theme,
};

const GAP: f32 = 12.0;
/// Height the console keeps when it is open, however much the sections above need.
const CONSOLE_MIN_HEIGHT: f32 = 230.0;
const CONSOLE_BAR_HEIGHT: f32 = 34.0;

pub fn root(app: &App) -> Element<'_, Message> {
    let body = responsive(move |size| {
        let reserved = if app.console.open { CONSOLE_MIN_HEIGHT } else { CONSOLE_BAR_HEIGHT };
        // The sections scroll when the window is too small for them, so the console (when
        // open) always keeps its share and nothing is ever unreachable.
        let sections =
            container(scrollable(sections(app)).spacing(6)).max_height((size.height - reserved - GAP).max(120.0));
        column![sections, console(app)].spacing(GAP).into()
    });
    container(body).padding(18).width(Fill).height(Fill).into()
}

fn sections(app: &App) -> Element<'_, Message> {
    let mut all = column![header(app)].spacing(GAP);
    if let Some(notice) = &app.notice {
        all = all.push(notice_banner(notice));
    }
    if app.emergency.open {
        all = all.push(emergency(app));
    }
    all.push(account(app)).push(instances(app)).push(launch(app)).into()
}

fn header(app: &App) -> Element<'_, Message> {
    row![
        text("bastion-mc").size(24),
        text("sandboxed Minecraft launcher").size(13).style(theme::muted),
        space().width(Fill),
        button(text(if app.emergency.open { "Close emergency help" } else { "Emergency…" }).size(13))
            .style(button::text)
            .on_press(Message::ToggleEmergency),
    ]
    .spacing(10)
    .align_y(iced::Bottom)
    .into()
}

// ------------------------------------------------------------------ emergency

/// The runbook, its links (the compiled-in list, nothing else), and the scram button.
fn emergency(app: &App) -> Element<'_, Message> {
    let state = &app.emergency;
    let scram: Element<'_, Message> = if state.running {
        text("Removing stored sign-ins…").style(text::warning).into()
    } else if state.confirming {
        row![
            text("Stop every running game and remove all stored sign-ins on this machine?").width(Fill),
            button("Yes, scram").style(button::danger).on_press(Message::ConfirmScram),
            button("Cancel").style(button::secondary).on_press(Message::CancelScram),
        ]
        .spacing(8)
        .align_y(Center)
        .into()
    } else {
        row![
            column![
                text("Scram: remove everything this launcher stores about your sign-ins"),
                small("Stops running games, deletes refresh tokens and vault files. Revokes nothing at Microsoft."),
            ]
            .spacing(3)
            .width(Fill),
            button("Scram…").style(button::danger).on_press(Message::AskScram),
        ]
        .spacing(8)
        .align_y(Center)
        .into()
    };
    let mut content = column![
        text(
            "If this computer itself may be compromised, do the account steps from another device — \
             a compromised machine can show you fake pages."
        )
        .style(text::warning),
        scram,
    ]
    .spacing(10);
    if !state.report.is_empty() {
        let mut report = column![].spacing(2);
        for line in &state.report {
            report = report.push(text(line.as_str()).font(Font::MONOSPACE).size(12));
        }
        content = content.push(container(report).padding(8).width(Fill).style(theme::well));
    }
    let mut links =
        column![small("The only pages this window will open (taken from Microsoft's own help articles):")].spacing(4);
    for (link, what) in runbook::LINKS {
        links = links.push(
            row![
                column![text(*what).size(13), small(*link)].spacing(1).width(Fill),
                button(text("Open").size(13))
                    .style(button::secondary)
                    .on_press(Message::OpenRunbookLink((*link).to_string())),
                button(text("Copy").size(13)).style(button::text).on_press(Message::Copy((*link).to_string())),
            ]
            .spacing(8)
            .align_y(Center),
        );
    }
    content = content.push(links);
    content = content.push(
        container(
            scrollable(text(runbook::SCRAM).font(Font::MONOSPACE).size(12).wrapping(text::Wrapping::WordOrGlyph))
                .height(Length::Fixed(260.0)),
        )
        .padding(8)
        .width(Fill)
        .style(theme::well),
    );
    card("EMERGENCY — ACCOUNT COMPROMISED?", content)
}

fn card<'a>(title: &'a str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(column![text(title).size(11).style(theme::label), content.into()].spacing(8))
        .padding(14)
        .width(Fill)
        .style(theme::card)
        .into()
}

fn small<'a>(content: impl text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(content).size(12).style(theme::muted)
}

fn notice_banner(notice: &Notice) -> Element<'_, Message> {
    container(
        row![
            text(&notice.text).size(14).width(Fill),
            // Messages can contain paths and settings to act on; text is not selectable.
            button(text("Copy").size(13)).style(button::text).on_press(Message::Copy(notice.text.clone())),
            button(text("Dismiss").size(13)).style(button::text).on_press(Message::DismissNotice),
        ]
        .spacing(10)
        .align_y(Center),
    )
    .padding([10, 14])
    .width(Fill)
    .style(theme::notice(notice.severity))
    .into()
}

// ------------------------------------------------------------------ account

fn account(app: &App) -> Element<'_, Message> {
    let idle = !app.busy();
    let signing_in = app.job_kind() == Some(JobKind::SignIn);

    let summary: Element<'_, Message> = match &app.account {
        None => text("Checking the account…").style(theme::muted).into(),
        Some(account) if account.signed_in => {
            let who = match &account.player {
                Some(player) => format!("Signed in as {player}"),
                None => "Signed in".to_string(),
            };
            column![text(who).style(text::success), small(format!("Refresh token: {}", account.store))]
                .spacing(3)
                .into()
        }
        Some(account) if !account.client_id_configured => column![
            text("No OAuth client ID is configured"),
            small(
                "Set auth.client_id in config.toml to your own Azure application to sign in. \
                 Demo mode works without an account."
            ),
        ]
        .spacing(3)
        .into(),
        Some(account) => {
            column![text("Not signed in"), small(format!("Refresh token: {}", account.store))].spacing(3).into()
        }
    };

    let mut actions = row![].spacing(8).align_y(Center);
    if signing_in {
        let stopping = app.job.as_ref().is_some_and(|job| job.stopping);
        actions = actions
            .push(button("Cancel").style(button::secondary).on_press_maybe((!stopping).then_some(Message::Stop)));
    } else {
        match &app.account {
            Some(account) if account.signed_in => {
                actions = actions
                    .push(button("Sign out").style(button::secondary).on_press_maybe(idle.then_some(Message::SignOut)));
            }
            Some(account) if account.client_id_configured => {
                if !account.device_flow {
                    actions = actions.push(
                        button("Use a device code")
                            .style(button::text)
                            .on_press_maybe(idle.then_some(Message::SignIn { device_code: true })),
                    );
                }
                actions = actions.push(
                    button("Sign in")
                        .style(button::primary)
                        .on_press_maybe(idle.then_some(Message::SignIn { device_code: account.device_flow })),
                );
            }
            _ => {}
        }
    }

    let mut content = column![row![summary, space().width(Fill), actions].spacing(10).align_y(Center)].spacing(GAP);
    content = content.push(account_picker(app));
    if let Some(prompt) = &app.login {
        content = content.push(login_prompt(prompt, app.now));
    }
    card("ACCOUNT", content)
}

/// Which local account is shown, and a field to start another one. Accounts are labels:
/// each has its own stored token, and switching signs nobody in or out.
fn account_picker(app: &App) -> Element<'_, Message> {
    let idle = !app.busy();
    let current = (!app.account_name.is_empty()).then_some(&app.account_name);
    let chooser: Element<'_, Message> = if idle {
        pick_list(app.accounts.as_slice(), current, Message::AccountSelected)
            .placeholder("Account")
            .width(Length::Fixed(180.0))
            .into()
    } else {
        container(text(app.account_name.as_str())).padding([6, 10]).width(Length::Fixed(180.0)).into()
    };
    let is_default = app.default_account.as_deref() == Some(app.account_name.as_str());
    let default: Element<'_, Message> = if is_default {
        small("default account").into()
    } else {
        button(text("Make default").size(13))
            .style(button::text)
            .on_press_maybe((idle && current.is_some()).then_some(Message::MakeDefaultAccount))
            .into()
    };
    let can_add = app.can_add_account();
    row![
        chooser,
        default,
        space().width(Fill),
        text_input("another account, e.g. alt", &app.new_account)
            .on_input(Message::NewAccountName)
            .on_submit_maybe(can_add.then_some(Message::AddAccount))
            .width(Length::Fixed(210.0)),
        button("Add").style(button::secondary).on_press_maybe(can_add.then_some(Message::AddAccount)),
    ]
    .spacing(8)
    .align_y(Center)
    .into()
}

fn countdown(prompt: &LoginPrompt, now: Instant) -> String {
    let left = prompt.expires_at().saturating_duration_since(now).as_secs();
    format!("{}:{:02}", left / 60, left % 60)
}

fn login_prompt(prompt: &LoginPrompt, now: Instant) -> Element<'_, Message> {
    let remaining = countdown(prompt, now);
    let panel: Element<'_, Message> = match prompt {
        LoginPrompt::DeviceCode { user_code, verification_uri, .. } => {
            let mut buttons = row![
                button("Copy code").style(button::primary).on_press(Message::Copy(user_code.clone())),
                button("Copy link").style(button::secondary).on_press(Message::Copy(verification_uri.clone())),
            ]
            .spacing(8)
            .align_y(Center);
            if is_sign_in_link(verification_uri) {
                buttons = buttons.push(
                    button("Open link").style(button::secondary).on_press(Message::OpenLink(verification_uri.clone())),
                );
            }
            buttons = buttons.push(space().width(Fill)).push(small(format!("The code expires in {remaining}")));
            column![
                text("Open this page in any browser and enter the code:").size(14),
                text(verification_uri).font(Font::MONOSPACE).size(15),
                container(text(user_code).font(Font::MONOSPACE).size(36)).padding([10, 24]).style(theme::well),
                buttons,
            ]
            .spacing(10)
            .into()
        }
        LoginPrompt::Browser { url, .. } => {
            let mut buttons = row![].spacing(8).align_y(Center);
            if is_sign_in_link(url) {
                buttons = buttons
                    .push(button("Open sign-in page").style(button::primary).on_press(Message::OpenLink(url.clone())));
            }
            buttons = buttons
                .push(button("Copy link").style(button::secondary).on_press(Message::Copy(url.clone())))
                .push(space().width(Fill))
                .push(small(format!("Waiting for the browser, {remaining} left")));
            column![
                text("Sign in with Microsoft in your browser. This window continues by itself once you are done.")
                    .size(14),
                buttons,
            ]
            .spacing(10)
            .into()
        }
    };
    container(panel).padding(14).width(Fill).style(theme::well).into()
}

// ------------------------------------------------------------------ instances

fn instances(app: &App) -> Element<'_, Message> {
    let idle = !app.busy();
    let launching = app.job_kind() == Some(JobKind::Launch);

    // While a game runs the choice is shown, not offered: it could not change anything.
    let chooser: Element<'_, Message> = match (&app.selected, launching) {
        (Some(choice), true) => container(text(choice.to_string())).padding([6, 10]).width(Fill).into(),
        _ => pick_list(app.instances.as_slice(), app.selected.as_ref(), Message::InstanceSelected)
            .placeholder(if app.instances.is_empty() { "No instances yet" } else { "Choose an instance" })
            .width(Fill)
            .into(),
    };
    let mut content = column![
        row![
            chooser,
            button("Refresh").style(button::secondary).on_press_maybe(idle.then_some(Message::Refresh)),
            button(if app.new_instance.open { "Close" } else { "New…" })
                .style(button::secondary)
                .on_press_maybe((!launching).then_some(Message::ToggleNewInstance)),
        ]
        .spacing(8)
        .align_y(Center)
    ]
    .spacing(GAP);

    if let Some(choice) = &app.selected {
        if let Some(error) = &choice.0.error {
            content = content.push(text(error).size(13).style(text::danger));
        } else if let Some(detail) = &choice.0.detail {
            content = content.push(small(format!("Signed profile: {detail}")));
        }
    }
    if app.new_instance.open {
        content = content.push(new_instance(app));
    }
    card("INSTANCE", content)
}

fn new_instance(app: &App) -> Element<'_, Message> {
    let form = &app.new_instance;
    let idle = !app.busy();
    let loading = app.job_kind() == Some(JobKind::Versions);
    let can_create = app.can_create_instance();

    let hint: Option<&str> = if form.id.is_empty() {
        None
    } else if !is_valid_id(&form.id) {
        Some("Names use a-z, 0-9, '.', '_' and '-', and start with a letter or digit.")
    } else if app.instances.iter().any(|i| i.0.id == form.id) {
        Some("An instance with this name already exists.")
    } else {
        None
    };

    let mut content = column![
        row![
            text_input("name, e.g. survival", &form.id)
                .on_input(Message::NewInstanceId)
                .on_submit_maybe(can_create.then_some(Message::CreateInstance))
                .width(Length::Fixed(210.0)),
            pick_list(form.versions.as_slice(), form.version.as_ref(), Message::VersionSelected)
                .placeholder(if loading { "Loading versions…" } else { "Minecraft version" })
                .width(Fill),
            checkbox(form.snapshots).label("Snapshots").on_toggle_maybe(idle.then_some(Message::SnapshotsToggled)),
            button("Create").style(button::primary).on_press_maybe(can_create.then_some(Message::CreateInstance)),
        ]
        .spacing(8)
        .align_y(Center)
    ]
    .spacing(6);
    if let Some(hint) = hint {
        content = content.push(text(hint).size(12).style(text::danger));
    }
    container(content).padding(12).width(Fill).style(theme::well).into()
}

// ------------------------------------------------------------------ launch

fn launch(app: &App) -> Element<'_, Message> {
    let launching = app.job_kind() == Some(JobKind::Launch);
    let phase = app.job.as_ref().and_then(|job| job.phase);
    let stopping = app.job.as_ref().is_some_and(|job| job.stopping);

    let label = match (launching, phase) {
        (true, Some(Phase::Stopping)) => "Stopping…",
        (true, _) if stopping => "Stopping…",
        (true, Some(Phase::Running)) => "Running",
        (true, _) => "Launching…",
        (false, _) => "Play",
    };
    // `on_press_maybe(None)` is what disables the button: no handler, dimmed style.
    let play = button(text(label).size(18).width(Fill).center())
        .width(Length::Fixed(190.0))
        .padding([12, 18])
        .style(button::success)
        .on_press_maybe(app.can_play().then_some(Message::Play));

    let mut controls = row![play].spacing(12).align_y(Center);
    if launching {
        controls = controls.push(
            button(text("Stop").size(16))
                .padding([12, 18])
                .style(button::danger)
                .on_press_maybe((!stopping).then_some(Message::Stop)),
        );
    }
    controls = controls.push(space().width(Fill)).push(
        checkbox(app.demo)
            .label("Demo mode (no account)")
            .on_toggle_maybe((!app.busy()).then_some(Message::DemoToggled)),
    );

    let mut content = column![controls].spacing(10);
    if let Some(progress) = &app.progress {
        content = content.push(progress_bar(0.0..=1.0, progress.fraction()).girth(10)).push(small(progress.caption()));
    }
    let footnote = match (app.busy(), app.play_blocker()) {
        (false, Some(reason)) => Some(reason),
        _ if !app.launch_status.is_empty() => Some(app.launch_status.as_str()),
        _ => None,
    };
    if let Some(line) = footnote {
        content = content.push(small(line));
    }
    if !app.protection.is_empty() {
        content = content.push(protection(app));
    }
    card("PLAY", content)
}

// ------------------------------------------------------------------ console

/// What the launch runs with, one line per aspect: green when the protection is in force,
/// amber when it is weaker than it could be, red when it is off.
fn protection(app: &App) -> Element<'_, Message> {
    let mut lines = column![].spacing(2);
    for item in &app.protection {
        let (mark, style): (&str, fn(&Theme) -> text::Style) = match item.shield {
            Shield::On => ("protected", text::success),
            Shield::Partial => ("reduced", text::warning),
            Shield::Off => ("NOT protected", text::danger),
        };
        lines = lines.push(
            row![
                text(item.aspect.as_str()).size(12).style(theme::label).width(Length::Fixed(80.0)),
                text(mark).size(12).style(style).width(Length::Fixed(100.0)),
                text(item.detail.as_str()).size(12).style(theme::muted),
            ]
            .spacing(8),
        );
    }
    lines.into()
}

fn console_line(line: &Line) -> Element<'_, Message> {
    // Colour says who is talking (launcher, relay, game) and how loudly.
    let style: fn(&Theme) -> text::Style = match (line.level, line.source) {
        (Level::Error, _) => text::danger,
        (Level::Warn, _) => text::warning,
        (Level::Info, Source::Game) => text::default,
        (Level::Info, Source::Launcher) => text::primary,
        (Level::Info, Source::Egress) => theme::muted,
    };
    text(&line.text).font(Font::MONOSPACE).size(12).wrapping(text::Wrapping::WordOrGlyph).style(style).into()
}

fn console(app: &App) -> Element<'_, Message> {
    let console = &app.console;
    let toggle = match (console.open, console.len()) {
        (true, _) => "Hide console".to_string(),
        (false, 0) => "Show console".to_string(),
        (false, 1) => "Show console (1 line)".to_string(),
        (false, lines) => format!("Show console ({lines} lines)"),
    };
    let mut bar = row![button(text(toggle).size(13)).style(button::secondary).on_press(Message::ToggleConsole)]
        .spacing(10)
        .align_y(Center);
    if !app.status.is_empty() {
        bar = bar.push(small(app.status.as_str()));
    }
    if !console.open {
        return bar.into();
    }
    let has_lines = !console.is_empty();
    if console.discarded() > 0 {
        bar = bar.push(small(format!("({} earlier lines are only in the log file)", console.discarded())));
    }
    bar = bar
        .push(space().width(Fill))
        .push(
            button(text("Copy all").size(13))
                .style(button::text)
                .on_press_maybe(has_lines.then_some(Message::CopyConsole)),
        )
        .push(
            button(text("Clear").size(13))
                .style(button::text)
                .on_press_maybe(has_lines.then_some(Message::ClearConsole)),
        );

    let body: Element<'_, Message> = if has_lines {
        // Anchored to the bottom: new output stays in view unless the user scrolled up.
        scrollable(column(console.lines().map(console_line)).spacing(1).width(Fill))
            .anchor_bottom()
            .spacing(6)
            .width(Fill)
            .height(Fill)
            .into()
    } else {
        container(small("Launcher and game output appears here.")).center(Fill).into()
    };
    column![bar, container(body).padding(10).width(Fill).height(Fill).style(theme::well)].spacing(8).height(Fill).into()
}

#[cfg(test)]
mod tests {
    //! The view, exercised without a display server: `iced_test` lays the widgets out,
    //! clicks them and renders them in software.
    use std::path::PathBuf;

    use bastion_mc_proto::{Event, Protection, Unit};
    use iced::Size;
    use iced_test::{Simulator, simulator};

    use super::*;
    use crate::app::testing::{account, events, feed, finish, ready};

    /// One headless renderer at a time: with a GPU backend, several of them starting up on
    /// different test threads at once is more than some drivers survive.
    static RENDERER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        RENDERER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clicked(app: &App, label: &str) -> Vec<Message> {
        let _one = exclusive();
        let mut ui = simulator(root(app));
        ui.click(label).unwrap_or_else(|e| panic!("cannot click {label:?}: {e}"));
        ui.into_messages().collect()
    }

    fn shows(app: &App, text: &str) -> bool {
        let _one = exclusive();
        simulator(root(app)).find(text).is_ok()
    }

    #[test]
    fn the_emergency_panel_asks_before_scram_and_offers_only_the_runbook_links() {
        let mut app = ready(true);
        assert!(!shows(&app, "Scram…"), "closed until asked for");
        assert!(matches!(clicked(&app, "Emergency…").as_slice(), [Message::ToggleEmergency]));
        let _ = app.update(Message::ToggleEmergency);
        assert!(shows(&app, "Minecraft Support: submit a request"));
        assert!(shows(&app, "https://account.live.com/proofs/manage/additional"));
        // One press only asks.
        assert!(matches!(clicked(&app, "Scram…").as_slice(), [Message::AskScram]));
        assert!(!shows(&app, "Yes, scram"));
        let _ = app.update(Message::AskScram);
        assert!(matches!(clicked(&app, "Yes, scram").as_slice(), [Message::ConfirmScram]));
        assert!(matches!(clicked(&app, "Cancel").as_slice(), [Message::CancelScram]));
        let _ = app.update(Message::CancelScram);
        assert!(shows(&app, "Scram…") && !shows(&app, "Yes, scram"));
    }

    #[test]
    fn protections_of_a_launch_are_listed_by_the_play_button() {
        let mut app = ready(true);
        assert!(!shows(&app, "NOT protected"));
        launching(&mut app);
        feed(
            &mut app,
            events([Event::Protection {
                items: vec![
                    Protection {
                        aspect: "display".into(),
                        shield: Shield::Off,
                        detail: "the session's X11 display".into(),
                    },
                    Protection { aspect: "network".into(), shield: Shield::On, detail: "filtered".into() },
                ],
            }]),
        );
        assert!(shows(&app, "NOT protected") && shows(&app, "the session's X11 display"));
        assert!(shows(&app, "protected") && shows(&app, "filtered"));
    }

    fn launching(app: &mut App) {
        let _ = app.update(Message::Play);
        feed(app, events([Event::Phase { phase: Phase::Preparing }]));
    }

    fn with_device_code(app: &mut App) {
        let _ = app.update(Message::SignIn { device_code: true });
        feed(
            app,
            events([Event::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://www.microsoft.com/link".into(),
                expires_in: 900,
            }]),
        );
    }

    #[test]
    fn play_button_follows_the_state() {
        let mut app = ready(true);
        assert!(matches!(clicked(&app, "Play").as_slice(), [Message::Play]));

        launching(&mut app);
        assert!(shows(&app, "Launching…") && !shows(&app, "Play"));
        assert!(clicked(&app, "Launching…").is_empty(), "a disabled button produces nothing");
        assert!(matches!(clicked(&app, "Stop").as_slice(), [Message::Stop]));

        feed(&mut app, events([Event::Phase { phase: Phase::Running }]));
        assert!(shows(&app, "Running") && clicked(&app, "Running").is_empty());

        feed(&mut app, events([Event::Exited { code: 0 }]));
        finish(&mut app, 0); // launch
        finish(&mut app, 0); // the refresh that follows it
        assert!(matches!(clicked(&app, "Play").as_slice(), [Message::Play]), "enabled again after the game exits");
    }

    #[test]
    fn play_is_inert_and_explains_itself_when_nothing_can_be_launched() {
        let app = ready(false);
        assert!(clicked(&app, "Play").is_empty());
        assert!(shows(&app, "Sign in, or tick demo mode."));
        assert!(shows(&app, "Not signed in"));
        assert!(matches!(clicked(&app, "Sign in").as_slice(), [Message::SignIn { device_code: false }]));
        assert!(matches!(clicked(&app, "Use a device code").as_slice(), [Message::SignIn { device_code: true }]));
        assert!(matches!(clicked(&app, "Demo mode (no account)").as_slice(), [Message::DemoToggled(true)]));
    }

    #[test]
    fn download_progress_is_visible_while_preparing() {
        let mut app = ready(true);
        launching(&mut app);
        feed(
            &mut app,
            events([Event::Progress { label: "assets".into(), done: 104857600, total: 419430400, unit: Unit::Bytes }]),
        );
        assert!(shows(&app, "Downloading assets: 100.0 of 400.0 MiB (25%)"));
        feed(&mut app, events([Event::Phase { phase: Phase::Verifying }]));
        assert!(!shows(&app, "Downloading assets: 100.0 of 400.0 MiB (25%)"));
        assert!(shows(&app, "Verifying every file before it is mounted…"));
    }

    #[test]
    fn device_code_is_displayed_with_its_actions() {
        let mut app = ready(false);
        with_device_code(&mut app);
        assert!(shows(&app, "ABCD-EFGH"), "the code is its own piece of text, not buried in a sentence");
        assert!(shows(&app, "https://www.microsoft.com/link"));
        assert!(shows(&app, "The code expires in 15:00"));
        assert!(shows(&app, "Waiting for you to enter the code…"));
        assert!(matches!(clicked(&app, "Copy code").as_slice(), [Message::Copy(code)] if code == "ABCD-EFGH"));
        assert!(
            matches!(clicked(&app, "Open link").as_slice(), [Message::OpenLink(url)] if url == "https://www.microsoft.com/link")
        );
        assert!(matches!(clicked(&app, "Cancel").as_slice(), [Message::Stop]));
        assert!(clicked(&app, "Play").is_empty(), "nothing else can be started meanwhile");

        feed(&mut app, events([Event::Account(account(true))]));
        finish(&mut app, 0);
        assert!(!shows(&app, "ABCD-EFGH") && shows(&app, "Signed in as Alex") && shows(&app, "Signed in."));
        assert!(matches!(clicked(&app, "Sign out").as_slice(), [Message::SignOut]));
    }

    #[test]
    fn an_untrusted_link_gets_no_open_button() {
        let mut app = ready(false);
        let _ = app.update(Message::SignIn { device_code: true });
        feed(
            &mut app,
            events([Event::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://not-microsoft.example/link".into(),
                expires_in: 900,
            }]),
        );
        assert!(shows(&app, "Copy link") && !shows(&app, "Open link"));
    }

    #[test]
    fn console_is_collapsed_until_asked_for() {
        let mut app = ready(true);
        launching(&mut app);
        feed(
            &mut app,
            events([Event::Log { source: Source::Game, line: "[Render thread/INFO]: Backend library: LWJGL".into() }]),
        );
        assert!(
            !shows(&app, "[Render thread/INFO]: Backend library: LWJGL"),
            "collapsed: the lines are not even built"
        );
        assert!(matches!(clicked(&app, "Show console (1 line)").as_slice(), [Message::ToggleConsole]));
        let _ = app.update(Message::ToggleConsole);
        assert!(shows(&app, "[Render thread/INFO]: Backend library: LWJGL"));
        assert!(matches!(clicked(&app, "Copy all").as_slice(), [Message::CopyConsole]));
        assert!(matches!(clicked(&app, "Clear").as_slice(), [Message::ClearConsole]));
        assert!(matches!(clicked(&app, "Hide console").as_slice(), [Message::ToggleConsole]));
    }

    #[test]
    fn errors_are_shown_and_can_be_dismissed() {
        let mut app = ready(true);
        let _ = app.update(Message::Play);
        feed(
            &mut app,
            events([Event::Error { message: "INTEGRITY VIOLATION: client jar does not match".into(), code: 4 }]),
        );
        finish(&mut app, 4);
        assert!(shows(&app, "INTEGRITY VIOLATION: client jar does not match"));
        assert!(
            matches!(clicked(&app, "Copy").as_slice(), [Message::Copy(text)] if text.starts_with("INTEGRITY VIOLATION"))
        );
        assert!(matches!(clicked(&app, "Dismiss").as_slice(), [Message::DismissNotice]));
    }

    /// Render the main screens in software and check the one visual property that can be
    /// stated objectively: the window is dark, with light text on it. The images are left
    /// in `target/gui-snapshots/` for a human to look at; they are not compared to a
    /// baseline (font rendering differs between machines).
    #[test]
    fn every_screen_renders_dark() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/gui-snapshots");
        let theme = theme::theme();

        let idle = ready(true);
        let mut signing_in = ready(false);
        with_device_code(&mut signing_in);
        let mut downloading = ready(true);
        launching(&mut downloading);
        feed(
            &mut downloading,
            events([
                Event::Status { text: "assets: 2114 of 5149 to download".into() },
                Event::Progress { label: "assets".into(), done: 163577856, total: 419430400, unit: Unit::Bytes },
            ]),
        );
        let mut running = ready(true);
        launching(&mut running);
        let _ = running.update(Message::ToggleConsole);
        feed(
            &mut running,
            events([
                Event::Status {
                    text: "sandbox started; log: ~/.local/state/bastion-mc/logs/vanilla/launch-1.log".into(),
                },
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
        feed(
            &mut running,
            events([Event::Protection {
                items: vec![
                    Protection {
                        aspect: "display".into(),
                        shield: Shield::On,
                        detail: "nested compositor: a display server of the game's own, clipboard cut".into(),
                    },
                    Protection {
                        aspect: "network".into(),
                        shield: Shield::On,
                        detail: "filtered: only allowlisted hosts, through the launcher".into(),
                    },
                    Protection {
                        aspect: "credential".into(),
                        shield: Shield::Partial,
                        detail: "the session token is inside the game process".into(),
                    },
                    Protection { aspect: "limits".into(), shield: Shield::Off, detail: "no resource limits".into() },
                ],
            }]),
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
            feed(&mut running, events([Event::Log { source: Source::Game, line: line.into() }]));
        }
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

        let mut emergency = ready(true);
        let _ = emergency.update(Message::ToggleEmergency);
        let _ = emergency.update(Message::AskScram);

        for (name, app) in [
            ("emergency", &emergency),
            ("idle", &idle),
            ("device-code", &signing_in),
            ("downloading", &downloading),
            ("running", &running),
            ("error", &failed),
            ("refused", &refused),
        ] {
            let _one = exclusive();
            let mut ui = Simulator::with_size(iced_test::core::Settings::default(), Size::new(880.0, 720.0), root(app));
            let snapshot = ui.snapshot(&theme).expect("render");
            for stale in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if stale.file_name().to_string_lossy().starts_with(&format!("{name}-")) {
                    std::fs::remove_file(stale.path()).unwrap();
                }
            }
            assert!(snapshot.matches_image(dir.join(name)).expect("write snapshot"));
            let written = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{name}-"))))
                .expect("snapshot file");

            let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&written).unwrap()));
            let mut reader = decoder.read_info().unwrap();
            let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut pixels).unwrap();
            let luma: Vec<u32> = pixels[..info.buffer_size()]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000)
                .collect();
            let mean = luma.iter().sum::<u32>() / luma.len() as u32;
            let dark = luma.iter().filter(|l| **l < 64).count() * 100 / luma.len();
            let bright = luma.iter().filter(|l| **l > 160).count();
            assert!(mean < 60, "{name}: mean luminance {mean} is not a dark theme");
            assert!(dark >= 85, "{name}: only {dark}% of the window is dark");
            assert!(bright > 200, "{name}: no light text was drawn ({bright} bright pixels)");
        }
    }
}
