//! The instances page: every instance directory the launcher found, which one a launch
//! would use, and the form that creates a new one.

use iced::{
    Center, Element, Fill, Length,
    widget::{Id, button, column, container, row, text, text_input},
};

use super::{Layout, kit};
use crate::{
    app::{App, InstanceChoice, JobKind, Message, VersionChoice, is_valid_id},
    backend::fit,
    hud,
    theme::{self, Tone},
};

/// Characters of an instance's kind shown in its tag.
const KIND_CHARS: usize = 12;

pub fn page(app: &App, _layout: Layout) -> Element<'_, Message> {
    let idle = !app.busy();
    let launching = app.launch().is_some();
    let tools = row![
        kit::tool("Refresh", Tone::Neutral, idle.then_some(Message::Refresh)),
        kit::tool(
            if app.new_instance.open { "Close form" } else { "New instance" },
            Tone::Accent,
            (!launching).then_some(Message::ToggleNewInstance)
        ),
    ]
    .spacing(theme::S2);

    let mut list = column![].spacing(theme::S1);
    if app.instances.is_empty() {
        list = list.push(kit::small(if app.busy() {
            "Asking the launcher…"
        } else {
            "No instances yet. Create one: it is a directory with its own saves, settings and Minecraft version."
        }));
    }
    for choice in &app.instances {
        list = list.push(entry(app, choice, launching));
    }
    if app.instances_hidden > 0 {
        list = list.push(kit::small(format!(
            "{} more instances are not shown here; `bastion-mc instance list` prints all of them.",
            app.instances_hidden
        )));
    }
    let heading = if launching { "Instances — fixed while a launch is under way" } else { "Instances" };
    let mut content =
        column![kit::panel_with(hud::Frame::plate(), kit::label(heading), Some(tools.into()), list)].spacing(theme::S4);
    if app.new_instance.open {
        content = content.push(new_instance(app));
    }
    content.into()
}

/// The widget id of the row of an instance.
pub fn entry_id(id: &str) -> Id {
    Id::from(format!("instance-{id}"))
}

fn entry<'a>(app: &'a App, choice: &'a InstanceChoice, launching: bool) -> Element<'a, Message> {
    let instance = &choice.0;
    let selected = app.selected.as_ref().is_some_and(|current| current.0.id == instance.id);
    let version = match &instance.minecraft {
        Some(version) => format!("Minecraft {version}"),
        None => "Minecraft version unknown".to_string(),
    };
    let mut tags = row![].spacing(theme::S1).align_y(Center);
    if selected {
        tags = tags.push(kit::chip("Selected", Tone::Accent));
    }
    if instance.error.is_some() {
        tags = tags.push(kit::chip("Cannot be launched", Tone::Danger));
    } else if !instance.kind.is_empty() {
        // The launcher's kinds are a word ("vanilla", "curated"); a tag does not wrap.
        tags = tags.push(kit::chip(&fit(&instance.kind, KIND_CHARS), Tone::Neutral));
    }
    // A name may be 64 characters long: it wraps, and the version moves under it, rather
    // than either being squeezed.
    let mut words = column![
        row![
            kit::mono(instance.id.as_str())
                .size(15)
                .color(if selected { theme::ACCENT } else { theme::TEXT })
                .wrapping(text::Wrapping::WordOrGlyph),
            kit::small(version),
        ]
        .spacing(theme::S3)
        .align_y(Center)
        .wrap()
    ]
    .spacing(3)
    .width(Fill);
    if let Some(error) = &instance.error {
        words = words.push(kit::small(error.as_str()).color(theme::RED));
    } else if let Some(detail) = &instance.detail {
        words = words.push(kit::small(format!("Signed profile: {detail}")));
    }
    let face = row![
        container(kit::pip(if selected { theme::ACCENT } else { theme::LINE_HI }, selected))
            .padding(iced::Padding { top: 5.0, ..iced::Padding::ZERO }),
        words,
        tags
    ]
    .spacing(theme::S3);
    // While a launch is under way the choice is shown, not offered.
    button(container(face).id(entry_id(&instance.id)).width(Fill))
        .width(Fill)
        .padding([9, 10])
        .style(theme::row_button(selected))
        .on_press_maybe((!launching && !selected).then(|| Message::InstanceSelected(choice.clone())))
        .into()
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

    let name = column![
        kit::label("Name"),
        text_input("for example: survival", &form.id)
            .on_input(Message::NewInstanceId)
            .on_submit_maybe(can_create.then_some(Message::CreateInstance))
            .id(Id::new("new-instance"))
            .font(theme::MONO)
            .size(theme::SIZE_SMALL + 1.0)
            .padding([7, 10])
            .style(theme::input)
            .width(Fill),
    ]
    .spacing(theme::S1)
    .width(Length::FillPortion(2));
    let version = column![
        kit::label("Minecraft version"),
        kit::with_picker_room(move |chars| kit::picker(
            form.versions.as_slice(),
            form.version.as_ref(),
            if loading { "Loading versions…" } else { "Minecraft version" },
            chars,
            VersionChoice::label,
            Message::VersionSelected,
        )),
    ]
    .spacing(theme::S1)
    .width(Length::FillPortion(3));

    let mut content = column![
        row![name, version].spacing(theme::S3),
        row![
            kit::switch("Snapshots", form.snapshots, idle.then_some(Message::SnapshotsToggled(!form.snapshots))),
            iced::widget::space().width(Fill),
            kit::solid("Create", Tone::Accent, can_create.then_some(Message::CreateInstance)),
        ]
        .spacing(theme::S3)
        .align_y(Center),
    ]
    .spacing(theme::S3);
    if let Some(hint) = hint {
        content = content.push(kit::small(hint).color(theme::RED));
    }
    kit::panel("New instance", content)
}
