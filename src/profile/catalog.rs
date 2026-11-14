//! `profile add` and `profile check`: mods from a registry (Modrinth), one command each.
//!
//! In this launcher a mod exists only through a manifest the user signed, and nothing here
//! changes that. `profile add` is an authoring aid. It saves the search for a jar, and it
//! replaces "whatever the browser saved" by a file whose size and SHA-512 are the ones the
//! registry publishes for a named project and version. It ends exactly where `profile
//! lock` ends — a draft manifest, the serial bumped, the signature removed — and says so.
//! It signs nothing and starts nothing.
//!
//! What it decides, and by which rule:
//!
//! * **Which build.** Of the builds the registry lists for the profile's Minecraft version
//!   and loader, the newest *release*. Betas and alphas only with `--prerelease`, and then
//!   only for a project that has no release for that version.
//! * **Freshness quarantine.** A build published less than 48 hours ago is passed over —
//!   the next older one is taken and the output names what was skipped — unless
//!   `--allow-fresh` is given. The point is to leave time for a bad upload to be noticed
//!   and withdrawn; whether it is noticed in that time is not something this program can
//!   know. Age is measured against the earlier of this machine's clock and the `Date` of
//!   the registry's own answers, so a wrong clock can only make the rule stricter; a
//!   build dated after that "now" counts as fresh.
//!   The registry dates builds, not files, and a file can be put into a build after its
//!   publication. So the file server's date for the file itself (`Last-Modified`) is read
//!   with the download, and bytes it has held for less than 48 hours are refused whatever
//!   the age of their build (`file_date`, which also says what that date is, what it is
//!   not, and what the check costs). It needs the download: `--dry-run` and `profile
//!   check` go by the date of the build.
//! * **Dependencies.** What a build marks *required* is resolved by the same rules, a few
//!   levels deep, unless a build of that project which the registry lists for this
//!   Minecraft version and loader is already in `mods/`. Optional ones are only named.
//!   Not guaranteed: that the list is as old as the build. It is metadata a project's
//!   members can edit at any time, and nothing dates such an edit; what was added that way
//!   is followed, and stands in the plan like every other project to be added.
//! * **Replacing.** A jar is removed only when the registry identifies it, by its hash, as
//!   a build of the very project being added *and* the old and the new jar declare the
//!   same mod id - and then only if it is an *older* build, or, for a required dependency,
//!   one the registry does not list for this Minecraft version and loader. A build in
//!   `mods/` that is listed for this profile and is not older than the newest one the
//!   rules allow always stays: there is no downgrade, whatever a dependent asks for. A jar
//!   the registry does not know is never removed; if it is in the way (same file name,
//!   same mod id) the command stops.
//! * **All or nothing.** Everything is downloaded into a staging directory next to
//!   `mods/` and verified there. Only when every file is in hand are `mods/`, the manifest
//!   and the signature touched.
//!
//! Afterwards the static scan of `capabilities` runs over what was added, so that what a
//! new mod can reach is on the screen before anything is signed.
//!
//! `profile check` asks the registry about the hash of every jar in `mods/` and reports
//! which are known (project, version, when published), which are unknown, and which are
//! fresh. What a match does and does not mean is printed with every report
//! ([`CHECK_CAVEAT`]).
//!
//! **What the registry learns** from either command: the address the requests come from
//! (the SOCKS5 proxy's when `network.launcher_proxy` is set), the user agent
//! `bastion-mc/<version>`, the names asked for, the profile's Minecraft version and
//! loader, and the SHA-512 of every jar in `mods/` — that is how mods already present are
//! recognised. No account, cookie or token is involved. A launch contacts neither host.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::atomic::AtomicU64,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha512};

use super::{
    author, capabilities,
    manifest::{LoaderKind, ProfileManifest},
    modrinth::{self, Channel, DependencyKind, Project, Registry, Transport, Version, VersionFile},
    mods,
};
use crate::{
    config::Config,
    error::{Context, Error, Result},
    instance::{MANIFEST_FILE, SIGNATURE_FILE},
    paths::Paths,
    util,
};

/// A build younger than this is not taken without `--allow-fresh`.
pub const QUARANTINE_SECS: u64 = 48 * 3600;
/// How much later than its build the file server may date a file before that date counts
/// for anything. (Where the file server's date was that of the upload - ten of the fifteen
/// real files compared on 2026-11-14 - it was at most three seconds after the build's. The
/// hour leaves room for a slow upload, and costs nothing: a file that is an hour younger
/// than a build which is out of quarantine has itself been there for 47 hours.)
pub const LATE_FILE_SECS: u64 = 3600;
/// How far required dependencies are followed. What was asked for is level 0.
pub const MAX_DEPTH: usize = 4;
/// Projects one command may look at, dependencies included.
pub const MAX_PROJECTS: usize = 24;
/// Mods one command may be asked for.
pub const MAX_REQUESTED: usize = 16;
/// Jars in `mods/` these commands are willing to hash and look up.
const MAX_JARS: usize = 512;
const STAGING_PREFIX: &str = ".catalog-";

/// A short name for a well-known project. An alias names the project by its **id**: the
/// registry's documentation says slugs "can change at any moment", and a slug that was
/// given up can be taken by somebody else, while an id stays with its project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alias {
    pub name: &'static str,
    pub id: &'static str,
    /// The project's slug when the alias was recorded (2026-11-14), to notice a rename.
    pub slug: &'static str,
}

/// Looked up at the registry on 2026-11-14; `docs/notes/mod-catalog.md` has the table with
/// licences and what existed for Minecraft 26.3 on that day.
pub const ALIASES: &[Alias] = &[
    Alias { name: "viafabric", id: "YlKdE5VK", slug: "viafabric" },
    Alias { name: "simple-voice-chat", id: "9eGKb6K1", slug: "simple-voice-chat" },
    Alias { name: "voicechat", id: "9eGKb6K1", slug: "simple-voice-chat" },
    Alias { name: "fabric-api", id: "P7dR8mSH", slug: "fabric-api" },
    Alias { name: "sodium", id: "AANobbMI", slug: "sodium" },
    Alias { name: "lithium", id: "gvQqBUqZ", slug: "lithium" },
    Alias { name: "modmenu", id: "mOgUt4GM", slug: "modmenu" },
];

pub fn alias(name: &str) -> Option<&'static Alias> {
    ALIASES.iter().find(|a| a.name.eq_ignore_ascii_case(name))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Take a beta or alpha where a project has no release for this Minecraft version.
    pub prerelease: bool,
    /// Take builds published less than [`QUARANTINE_SECS`] ago.
    pub allow_fresh: bool,
    /// Resolve and show; download nothing, change nothing.
    pub dry_run: bool,
}

/// Where the commands' text goes: printed as it is produced, and kept for the caller.
/// (These commands are not offered in `--events` mode, so this is plain text; everything
/// that reaches it from the registry was cleaned or validated when it was parsed.)
#[derive(Debug, Default)]
pub struct Output {
    echo: bool,
    pub lines: Vec<String>,
}

impl Output {
    pub fn terminal() -> Self {
        Output { echo: true, lines: Vec::new() }
    }

    pub fn silent() -> Self {
        Output::default()
    }

    fn say(&mut self, line: impl Into<String>) {
        let line = line.into();
        if self.echo {
            println!("{line}");
        }
        self.lines.push(line);
    }
}

pub const CHECK_CAVEAT: &str = "\
What \"known\" means: Modrinth lists a file with exactly these bytes under that project and version.
What it does not mean: that the file is harmless. Whoever controls a project's account - its author,
or somebody who took the account over - publishes a bad build together with its hash, and it is
\"known\" here like any other. Only Modrinth was asked: CurseForge (its API needs a key) and GitHub
releases are not consulted, so \"unknown\" covers a mod published only elsewhere and a build you made
yourself as well as a jar that was tampered with or withdrawn.
\"Fresh\" goes by the date of the build. A file that was put into an older build afterwards is not
recognised by this command: the registry dates builds, not files (`profile add` reads the file
server's date for a file when it downloads it).";

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------------------------------------------------------------- what is there

/// A jar in `mods/`, and what the registry says its bytes are.
#[derive(Debug, Clone)]
struct Present {
    file: String,
    size: u64,
    sha256: String,
    sha512: String,
    known: Option<Version>,
}

fn sha512_of(path: &Path) -> Result<String> {
    struct Hashing(Sha512);
    impl io::Write for Hashing {
        fn write(&mut self, chunk: &[u8]) -> io::Result<usize> {
            self.0.update(chunk);
            Ok(chunk.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut file = util::open_regular_nofollow(path).ctx(|| format!("opening {}", path.display()))?;
    let mut hashing = Hashing(Sha512::new());
    io::copy(&mut file, &mut hashing).ctx(|| format!("reading {}", path.display()))?;
    Ok(util::hex_encode(&hashing.0.finalize()))
}

/// Every jar in `mods/`, under the same conditions as `profile lock`: plain files with
/// safe names ending in `.jar`, nothing else in the directory.
fn inventory(mods_dir: &Path) -> Result<Vec<Present>> {
    let entries = mods::scan_mods_dir(mods_dir)?;
    if entries.len() > MAX_JARS {
        return Err(Error::Policy(format!("{} holds more than {MAX_JARS} jars", mods_dir.display())));
    }
    entries
        .into_iter()
        .map(|entry| {
            let sha512 = sha512_of(&mods_dir.join(&entry.file))?;
            Ok(Present { file: entry.file, size: entry.size, sha256: entry.sha256, sha512, known: None })
        })
        .collect()
}

/// Ask the registry which versions the present jars are (one request per 256 jars).
async fn identify<T: Transport>(registry: &Registry<'_, T>, present: &mut [Present]) -> Result<()> {
    let hashes: Vec<String> =
        present.iter().map(|jar| jar.sha512.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    if hashes.is_empty() {
        return Ok(());
    }
    let known = registry.by_hash(&hashes).await?;
    for jar in present {
        jar.known = known.get(&jar.sha512).cloned();
    }
    Ok(())
}

struct Draft {
    dir: PathBuf,
    manifest: ProfileManifest,
    loader: LoaderKind,
}

fn draft(paths: &Paths, id: &str) -> Result<Draft> {
    let dir = paths.instance(id)?;
    if !dir.is_dir() {
        return Err(Error::Usage(format!("no instance called {id:?}")));
    }
    if !dir.join(MANIFEST_FILE).exists() {
        return Err(Error::Usage(format!(
            "{id} has no profile manifest. Mods belong to curated profiles: \
             `bastion-mc profile new <id> --minecraft <version> --fabric` creates one."
        )));
    }
    let (dir, manifest) = author::read_draft(paths, id)?;
    let loader = manifest
        .loader
        .as_ref()
        .map(|loader| loader.kind)
        .ok_or_else(|| Error::Usage(format!("profile {id} has no mod loader, so it cannot hold mods")))?;
    Ok(Draft { dir, manifest, loader })
}

/// The loader names, as the registry spells them, whose builds a profile can take.
fn loader_names(kind: LoaderKind) -> &'static [&'static str] {
    match kind {
        LoaderKind::Fabric => &["fabric"],
        // By its design Quilt's loader also loads mods built for Fabric, so builds that
        // list either are taken. No Quilt profile was run to confirm it on this code.
        LoaderKind::Quilt => &["quilt", "fabric"],
    }
}

// ---------------------------------------------------------------- choosing a build

struct Rules<'a> {
    minecraft: &'a str,
    loader: LoaderKind,
    options: Options,
    /// This machine's clock when the command started.
    local: u64,
    /// The registry's clock as of its last answer (`Registry::clock`).
    registry_clock: &'a AtomicU64,
}

impl Rules<'_> {
    fn loaders(&self) -> &'static [&'static str] {
        loader_names(self.loader)
    }

    fn target(&self) -> String {
        let loader = match self.loader {
            LoaderKind::Fabric => "Fabric",
            LoaderKind::Quilt => "Quilt",
        };
        format!("Minecraft {} on {loader}", self.minecraft)
    }

    /// The earlier of this machine's clock and the registry's: whichever of the two is
    /// wrong, a build can only look younger than it is, never older.
    fn now(&self) -> u64 {
        modrinth::earlier(self.local, self.registry_clock)
    }

    /// Inside the quarantine — or dated after "now", which only a wrong clock explains.
    fn fresh(&self, version: &Version) -> bool {
        version.published.saturating_add(QUARANTINE_SECS) > self.now()
    }

    fn stamp(&self, version: &Version) -> String {
        format!("published {}, {}", modrinth::utc(version.published), modrinth::age(self.now(), version.published))
    }
}

/// The second date: since when the file server says it has held the bytes it just sent.
///
/// The registry dates a *build*, and that date stays when the build's files change: its
/// API lets a member of a project put a file into a published build and make it the
/// primary one, and no answer of the API dates a file. Going by the build's date alone, a
/// file uploaded a minute ago to a build of last month would pass as a month old - which
/// is what somebody who has taken over a project's account would do. So the file server's
/// own date for the file (`Last-Modified`) is held to the 48 hours as well, wherever it is
/// later than the build's ([`LATE_FILE_SECS`]).
///
/// That date says when the file server last *wrote* its copy, and it writes for reasons
/// of its own too. Of fifteen real files compared on 2026-11-14, ten were dated within
/// three seconds of their builds; the other five - builds of three projects, published
/// between 2026-09-09 and 2026-09-28 - were all dated 2026-10-19 between 10:15 and 10:17
/// UTC: the file server had rewritten its storage. A late date therefore does not show
/// that a file was added to its build later, and nothing printed here claims it. A late
/// date that is also *recent* means only that these bytes cannot be shown to have been
/// there for 48 hours - and that is refused, for an old file after such a rewrite as much
/// as for a new one. Failing closed has that price: two days in which `--allow-fresh` is
/// needed, or patience. The message names both readings.
///
/// Returns what there is to tell the user, and an error if the file may not be taken.
///
/// **Not established by this:** that the bytes were *visible* for that time - a file can
/// sit in a build as an additional file before it is made the primary one, and whether a
/// build that was prepared as a draft keeps the date of its creation is not something the
/// API documentation says. And nothing is established when the file server states no
/// date: then the date of the build is all there is, and the output says so. The date is
/// the file server's own statement; a registry that lies about dates lies here too.
fn file_date(
    project: &Project,
    version: &Version,
    file: &VersionFile,
    stored: Option<u64>,
    rules: &Rules<'_>,
) -> Result<Option<String>> {
    let Some(stored) = stored else {
        return Ok(Some(format!(
            "note: the file server stated no date for {}; that these bytes have been at the registry for 48 hours \
             rests on the date of the build alone",
            file.filename
        )));
    };
    let now = rules.now();
    let late = stored > version.published.saturating_add(LATE_FILE_SECS);
    if !late || stored.saturating_add(QUARANTINE_SECS) <= now {
        return Ok(None);
    }
    let since = format!("{}, {}", modrinth::utc(stored), modrinth::age(now, stored));
    let build = modrinth::utc(version.published);
    if !rules.options.allow_fresh {
        return Err(Error::Policy(format!(
            "{} {}: the file server has held its copy of {} only since {since}, although the build is dated \
             {build}. Either the file was put into the build after its publication, or the file server has \
             rewritten its storage; from here the two look the same, and bytes that cannot be shown to have been \
             there for 48 hours are not taken. The 48 hours end {}. --allow-fresh takes the file now (it lifts the \
             48 hours for builds and files alike). Nothing was changed.",
            project.title,
            version.number,
            file.filename,
            modrinth::utc(stored.saturating_add(QUARANTINE_SECS))
        )));
    }
    Ok(Some(format!(
        "note: FRESH FILE: the file server has held its copy of {} only since {since}, although the build is dated \
         {build}; taken because of --allow-fresh",
        file.filename
    )))
}

/// A remark when this machine's clock and the registry's are more than five minutes apart.
fn clock_note(local: u64, registry: Option<u64>) -> Option<String> {
    let registry = registry.filter(|server| server.abs_diff(local) > 300)?;
    Some(format!(
        "note: this machine's clock ({}) and the registry's ({}) disagree; ages are measured against the earlier \
         one, so the 48-hour rule can only come out stricter",
        modrinth::utc(local),
        modrinth::utc(registry)
    ))
}

/// Say which project and build an error is about, keeping its kind.
fn within(context: &str, error: Error) -> Error {
    match error {
        Error::Policy(m) => Error::Policy(format!("{context}: {m}")),
        Error::Format(m) => Error::Format(format!("{context}: {m}")),
        Error::Integrity(m) => Error::Integrity(format!("{context}: {m}")),
        Error::Net(m) => Error::Net(format!("{context}: {m}")),
        other => other,
    }
}

struct Choice {
    version: Version,
    file: VersionFile,
    /// Newer builds that were passed over, and why.
    skipped: Vec<String>,
    /// What there is to say about the chosen build itself.
    remarks: Vec<String>,
}

/// The build of `project` to take from `versions`, by the rules in the module header.
///
/// The server was asked to filter by game version and loader; everything is filtered
/// again here, and ordered by date here, because the list is not trusted to be either.
/// Once a build is selected its file must pass every check: a refused file is an error,
/// never a reason to quietly fall back to an older build.
fn choose(project: &Project, versions: &[Version], rules: &Rules<'_>) -> Result<Choice> {
    let mut usable: Vec<&Version> = versions
        .iter()
        .filter(|v| v.project_id == project.id && v.is_listed() && v.channel != Channel::Other)
        .filter(|v| v.supports(rules.minecraft, rules.loaders()))
        .collect();
    usable.sort_by(|a, b| b.published.cmp(&a.published).then_with(|| b.id.cmp(&a.id)));
    let Some(newest) = usable.first() else {
        return Err(Error::Other(format!(
            "{} ({}) has no build for {} at the registry",
            project.title,
            project.slug,
            rules.target()
        )));
    };
    let allowed: &[Channel] =
        if rules.options.prerelease { &[Channel::Release, Channel::Beta, Channel::Alpha] } else { &[Channel::Release] };
    let Some(channel) = allowed.iter().copied().find(|c| usable.iter().any(|v| v.channel == *c)) else {
        return Err(Error::Policy(format!(
            "{} has only pre-release builds for {}: the newest is {} ({}, {}). Pass --prerelease to take a \
             pre-release.",
            project.title,
            rules.target(),
            newest.number,
            newest.channel.name(),
            rules.stamp(newest)
        )));
    };
    let (mut skipped, mut remarks) = (Vec::new(), Vec::new());
    let mut newest_in_channel = None;
    for version in usable.iter().filter(|v| v.channel == channel) {
        newest_in_channel.get_or_insert(*version);
        if rules.fresh(version) && !rules.options.allow_fresh {
            skipped.push(format!(
                "passed over: {} ({}) is inside the 48-hour quarantine; --allow-fresh takes it",
                version.number,
                rules.stamp(version)
            ));
            continue;
        }
        let file = version.primary_jar().map_err(|e| within(&format!("{} {}", project.title, version.number), e))?;
        if rules.fresh(version) {
            remarks.push("FRESH: published less than 48 hours ago; taken because of --allow-fresh".into());
        }
        if channel != Channel::Release {
            remarks.push(format!(
                "PRE-RELEASE ({}): there is no release build for {}; taken because of --prerelease",
                channel.name(),
                rules.target()
            ));
        }
        return Ok(Choice { version: (*version).clone(), file, skipped, remarks });
    }
    let newest = newest_in_channel.unwrap_or(newest);
    Err(Error::Policy(format!(
        "every {} build of {} for {} was published less than 48 hours ago (the newest: {}, {}). A build that new \
         has had no time to be noticed if it is bad. The quarantine ends {}; pass --allow-fresh to take it now.",
        channel.name(),
        project.title,
        rules.target(),
        newest.number,
        rules.stamp(newest),
        modrinth::utc(newest.published.saturating_add(QUARANTINE_SECS))
    )))
}

/// Why the exact build a dependency names cannot be taken under the rules, if it cannot.
fn pin_refusal(version: &Version, rules: &Rules<'_>) -> Option<String> {
    if !version.is_listed() {
        return Some(format!("the registry marks as {}", version.status));
    }
    if !version.supports(rules.minecraft, rules.loaders()) {
        return Some(format!("is not listed for {}", rules.target()));
    }
    match version.channel {
        Channel::Release => {}
        Channel::Beta | Channel::Alpha if rules.options.prerelease => {}
        other => return Some(format!("is a pre-release ({})", other.name())),
    }
    (rules.fresh(version) && !rules.options.allow_fresh).then(|| "is inside the 48-hour quarantine".to_string())
}

/// Does a build already in `mods/` do for a dependency? It must be listed by the registry
/// for this Minecraft version and loader and, where the dependent names an exact build,
/// be that build or one published after it. Version *ranges* are the loader's to check
/// when the game starts; this program does not evaluate them.
fn suitable(have: &Version, rules: &Rules<'_>, pinned: Option<&Version>) -> bool {
    have.supports(rules.minecraft, rules.loaders())
        && pinned.is_none_or(|pin| have.id == pin.id || have.published >= pin.published)
}

// ---------------------------------------------------------------- the plan

#[derive(Debug, Clone)]
enum Why {
    Requested,
    RequiredBy(String),
}

/// A jar in `mods/` that an added build takes the place of: one the registry identified,
/// by its hash, as a build of the same project.
#[derive(Debug)]
struct Replaced {
    file: String,
    /// Published before the build that replaces it. Otherwise it is a build of a required
    /// dependency that the registry does not list for this profile's Minecraft version and
    /// loader - the one case in which a build that is not older is replaced.
    older: bool,
}

#[derive(Debug)]
enum Action {
    Add { version: Version, file: VersionFile, replaces: Vec<Replaced> },
    Keep { file: String, version: Option<Version> },
}

#[derive(Debug)]
struct Step {
    project: Project,
    why: Why,
    action: Action,
    notes: Vec<String>,
}

#[derive(Debug, Default)]
struct Plan {
    steps: Vec<Step>,
    notes: Vec<String>,
}

impl Plan {
    fn additions(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s.action, Action::Add { .. })).count()
    }

    /// Files in `mods/` that the plan, as it stands, removes.
    fn removed(&self) -> BTreeSet<String> {
        self.steps
            .iter()
            .filter_map(|s| match &s.action {
                Action::Add { replaces, .. } => Some(replaces.iter().map(|old| old.file.clone())),
                Action::Keep { .. } => None,
            })
            .flatten()
            .collect()
    }
}

enum Key {
    /// As typed by the user: an alias, a slug or an id.
    Name(String),
    /// From a dependency list.
    Id(String),
}

struct Want {
    key: Key,
    why: Why,
    depth: usize,
    /// The exact build a dependency names, if it names one.
    pin: Option<String>,
}

/// The project a user means by `name`.
async fn named<T: Transport>(registry: &Registry<'_, T>, name: &str, notes: &mut Vec<String>) -> Result<Project> {
    if let Some(alias) = alias(name) {
        let project = registry.project(alias.id).await?.ok_or_else(|| {
            Error::Other(format!(
                "the registry no longer has project {} (the built-in alias {:?})",
                alias.id, alias.name
            ))
        })?;
        if project.id != alias.id {
            return Err(Error::Format(format!(
                "asked for project {}, the registry answered with project {}",
                alias.id, project.id
            )));
        }
        if project.slug != alias.slug {
            notes.push(format!(
                "the registry now calls this project {:?}; it was {:?} when the alias {:?} was recorded (the alias \
                 follows the project's id)",
                project.slug, alias.slug, alias.name
            ));
        }
        return Ok(project);
    }
    modrinth::check_key(name)?;
    let project = registry.project(name).await?.ok_or_else(|| {
        let aliases: Vec<&str> = ALIASES.iter().map(|a| a.name).collect();
        Error::Usage(format!(
            "the registry has no project called {name:?}. Give a Modrinth slug or project id, or one of the built-in \
             aliases: {}.",
            aliases.join(", ")
        ))
    })?;
    // The answer has to be about what was asked: that id, or that slug.
    if project.id != name && !project.slug.eq_ignore_ascii_case(name) {
        return Err(Error::Format(format!(
            "asked for {name:?}, the registry answered with another project ({}, id {}); name it by that id if it \
             is the one you mean",
            project.slug, project.id
        )));
    }
    Ok(project)
}

/// Only public mod projects are taken; an archived one is taken with a remark.
fn acceptable(project: &Project, notes: &mut Vec<String>) -> Result<()> {
    if project.kind != "mod" {
        return Err(Error::Policy(format!(
            "{} is listed by the registry as a {}, not a mod (packs are added with `bastion-mc pack import`)",
            project.title, project.kind
        )));
    }
    match project.status.as_str() {
        "approved" => Ok(()),
        "archived" => {
            notes.push("the registry lists this project as archived: its author no longer maintains it".into());
            Ok(())
        }
        other => Err(Error::Policy(format!(
            "the registry lists {} with the status {other:?}; only approved or archived projects are taken",
            project.title
        ))),
    }
}

/// Turn what was asked for into a plan: for every project one step, to add a build or to
/// keep what is there. Reads from the registry, changes nothing anywhere.
async fn resolve<T: Transport>(
    registry: &Registry<'_, T>,
    wanted: &[String],
    present: &[Present],
    rules: &Rules<'_>,
) -> Result<Plan> {
    let mut queue: VecDeque<Want> = wanted
        .iter()
        .map(|name| Want { key: Key::Name(name.clone()), why: Why::Requested, depth: 0, pin: None })
        .collect();
    let mut plan = Plan::default();
    // Projects that have a step, and projects that are waiting in the queue by id.
    let (mut seen, mut queued) = (BTreeSet::new(), BTreeSet::new());
    let mut optional = BTreeSet::new();
    let mut incompatible: Vec<(String, String)> = Vec::new();

    while let Some(want) = queue.pop_front() {
        let mut notes = Vec::new();
        let project = match &want.key {
            Key::Name(name) => named(registry, name, &mut notes).await?,
            Key::Id(id) if seen.contains(id) => continue,
            Key::Id(id) => {
                let by = match &want.why {
                    Why::RequiredBy(title) => title.as_str(),
                    Why::Requested => "this command",
                };
                let project = registry.project(id).await?.ok_or_else(|| {
                    Error::Other(format!("{by} requires project {id}, which the registry does not have"))
                })?;
                if project.id != *id {
                    return Err(Error::Format(format!(
                        "asked for project {id}, the registry answered with project {}",
                        project.id
                    )));
                }
                project
            }
        };
        if !seen.insert(project.id.clone()) {
            continue; // asked for twice, or by two names
        }
        if seen.len() > MAX_PROJECTS {
            return Err(Error::Policy(format!(
                "this would involve more than {MAX_PROJECTS} projects; resolve a dependency graph this large by hand"
            )));
        }
        acceptable(&project, &mut notes)?;

        // Builds of this project already in mods/, newest first.
        let mut have: Vec<(&Present, &Version)> = present
            .iter()
            .filter_map(|jar| jar.known.as_ref().filter(|v| v.project_id == project.id).map(|v| (jar, v)))
            .collect();
        have.sort_by_key(|(_, version)| std::cmp::Reverse(version.published));
        if have.len() > 1 {
            let files: Vec<&str> = have.iter().map(|(jar, _)| jar.file.as_str()).collect();
            notes.push(format!("mods/ holds {} builds of this project: {}", have.len(), files.join(", ")));
        }

        let pinned = match &want.pin {
            None => None,
            Some(version_id) => match registry.version(version_id).await? {
                Some(version) if version.project_id == project.id => Some(version),
                _ => {
                    notes.push(format!(
                        "the exact build asked for ({version_id}) is not a build of this project any more"
                    ));
                    None
                }
            },
        };

        let action = match (&want.why, have.first()) {
            (Why::RequiredBy(_), Some((jar, version))) if suitable(version, rules, pinned.as_ref()) => {
                Action::Keep { file: jar.file.clone(), version: Some((*version).clone()) }
            }
            (why, newest_present) => {
                let choice = match pinned.as_ref().map(|pin| (pin, pin_refusal(pin, rules))) {
                    Some((pin, None)) => {
                        let file =
                            pin.primary_jar().map_err(|e| within(&format!("{} {}", project.title, pin.number), e))?;
                        let mut remarks = vec!["the exact build its dependent asks for".to_string()];
                        if rules.fresh(pin) {
                            remarks
                                .push("FRESH: published less than 48 hours ago; taken because of --allow-fresh".into());
                        }
                        Choice { version: pin.clone(), file, skipped: Vec::new(), remarks }
                    }
                    other => {
                        if let Some((pin, Some(reason))) = other {
                            notes.push(format!(
                                "its dependent asks for exactly {}, which {reason}; the newest build the rules allow \
                                 is looked for instead",
                                pin.number
                            ));
                        }
                        let versions = registry.versions(&project.id, rules.minecraft, rules.loaders()).await?;
                        choose(&project, &versions, rules)?
                    }
                };
                notes.extend(choice.skipped);
                let listed_here = |version: &Version| version.supports(rules.minecraft, rules.loaders());
                match newest_present {
                    // What is there is the very build that would be taken.
                    Some((jar, _)) if jar.sha512 == choice.file.sha512 => {
                        notes.extend(choice.remarks);
                        Action::Keep { file: jar.file.clone(), version: Some(choice.version) }
                    }
                    // Nothing is replaced by an older build of itself. For a mod that was asked
                    // for this holds without condition. For a dependency it holds as long as
                    // what is there is listed for this profile at all: a dependent that names a
                    // build which cannot be taken (fresh, a pre-release) must not lead to a
                    // downgrade, whereas a build for another Minecraft version does not do.
                    Some((jar, version))
                        if version.published >= choice.version.published
                            && (matches!(why, Why::Requested) || listed_here(version)) =>
                    {
                        notes.push(format!(
                            "the build in mods/ is not older than the newest one the rules allow ({}, {}); it stays",
                            choice.version.number,
                            rules.stamp(&choice.version)
                        ));
                        if !listed_here(version) {
                            notes.push(format!(
                                "the registry does not list the build in mods/ for {}: if it is the wrong one, remove \
                                 mods/{} and run this again",
                                rules.target(),
                                jar.file
                            ));
                        }
                        Action::Keep { file: jar.file.clone(), version: Some((*version).clone()) }
                    }
                    _ => {
                        notes.extend(choice.remarks);
                        let replaces = have
                            .iter()
                            .map(|(jar, version)| Replaced {
                                file: jar.file.clone(),
                                older: version.published < choice.version.published,
                            })
                            .collect();
                        Action::Add { version: choice.version, file: choice.file, replaces }
                    }
                }
            }
        };

        // What the build that will be in mods/ says about other projects.
        let in_effect = match &action {
            Action::Add { version, .. } => Some(version),
            Action::Keep { version, .. } => version.as_ref(),
        };
        for dependency in in_effect.map(|v| v.dependencies.as_slice()).unwrap_or_default() {
            match dependency.kind {
                DependencyKind::Required => {
                    let (id, pin) = match (&dependency.project_id, &dependency.version_id) {
                        (Some(project_id), pin) => (project_id.clone(), pin.clone()),
                        (None, Some(version_id)) => match registry.version(version_id).await? {
                            Some(version) => (version.project_id, Some(version_id.clone())),
                            None => {
                                plan.notes.push(format!(
                                    "{} requires a build ({version_id}) that the registry no longer has; add what it \
                                     needs yourself",
                                    project.title
                                ));
                                continue;
                            }
                        },
                        (None, None) => {
                            plan.notes.push(format!(
                                "{} requires {}, which the registry does not host; add it yourself",
                                project.title,
                                dependency.file_name.as_deref().unwrap_or("a file it does not name")
                            ));
                            continue;
                        }
                    };
                    if seen.contains(&id) || !queued.insert(id.clone()) {
                        continue;
                    }
                    if want.depth + 1 > MAX_DEPTH {
                        return Err(Error::Policy(format!(
                            "the dependencies of what was asked for go more than {MAX_DEPTH} levels deep (at {}); \
                             resolve them by hand",
                            project.title
                        )));
                    }
                    queue.push_back(Want {
                        key: Key::Id(id),
                        why: Why::RequiredBy(project.title.clone()),
                        depth: want.depth + 1,
                        pin,
                    });
                }
                DependencyKind::Optional => optional.extend(dependency.project_id.clone()),
                DependencyKind::Incompatible => {
                    incompatible.extend(dependency.project_id.clone().map(|id| (project.title.clone(), id)))
                }
                DependencyKind::Embedded | DependencyKind::Other => {}
            }
        }
        plan.steps.push(Step { project, why: want.why, action, notes });
    }

    let in_profile = |id: &String| {
        seen.contains(id) || present.iter().any(|jar| jar.known.as_ref().is_some_and(|v| v.project_id == *id))
    };
    for (title, id) in &incompatible {
        if in_profile(id) {
            let other = plan.steps.iter().find(|s| s.project.id == *id).map(|s| s.project.title.as_str());
            plan.notes.push(format!(
                "WARNING: {title} declares itself incompatible with {}, which is in this profile",
                other.map(str::to_string).unwrap_or_else(|| format!("project {id}"))
            ));
        }
    }
    let optional: Vec<String> = optional.into_iter().filter(|id| !in_profile(id)).take(modrinth::MAX_IDS).collect();
    if !optional.is_empty() {
        let mut names: Vec<String> =
            registry.projects(&optional).await?.iter().map(|p| format!("{} ({})", p.title, p.slug)).collect();
        names.sort();
        plan.notes.push(format!("optional, not added: {}", names.join(", ")));
    }
    Ok(plan)
}

/// No file may land on a name that is taken by a jar which stays.
fn check_names(plan: &Plan, present: &[Present]) -> Result<()> {
    let removed = plan.removed();
    let mut taken: BTreeSet<String> =
        present.iter().filter(|jar| !removed.contains(&jar.file)).map(|jar| jar.file.to_ascii_lowercase()).collect();
    for step in &plan.steps {
        if let Action::Add { file, .. } = &step.action
            && !taken.insert(file.filename.to_ascii_lowercase())
        {
            return Err(Error::Policy(format!(
                "{} would be saved as {}, but mods/ already holds a file of that name which the registry does not \
                 know as an older build of the same project. Nothing was changed; remove or rename that file first.",
                step.project.title, file.filename
            )));
        }
    }
    Ok(())
}

fn describe(step: &Step, rules: &Rules<'_>) -> Vec<String> {
    let project = &step.project;
    let why = match &step.why {
        Why::Requested => "asked for".to_string(),
        Why::RequiredBy(title) => format!("required by {title}"),
    };
    let head = format!("{} ({} · project {} · licence {})", project.title, project.slug, project.id, project.license);
    let build = |v: &Version| format!("{} [{}], {}", v.number, v.channel.name(), rules.stamp(v));
    let mut lines = Vec::new();
    match &step.action {
        Action::Add { version, file, replaces } => {
            lines.push(format!("add   {head} - {why}"));
            lines.push(format!("      version   {}", build(version)));
            lines.push(format!("      file      {}, {} bytes", file.filename, file.size));
            lines.push(format!("      sha512    {}", file.sha512));
            if let Some(sha1) = &file.sha1 {
                lines.push(format!("      sha1      {sha1}"));
            }
            lines.push(format!("      from      {}", file.url));
            for old in replaces {
                let what = if old.older {
                    "an older build of this project, by its hash".to_string()
                } else {
                    format!(
                        "a build of this project, by its hash, that the registry does not list for {}",
                        rules.target()
                    )
                };
                lines.push(format!("      replaces  {} ({what})", old.file));
            }
        }
        Action::Keep { file, version } => {
            lines.push(format!("keep  {head} - {why}"));
            match version {
                Some(version) => lines.push(format!("      present   {file}: {}", build(version))),
                None => lines.push(format!("      present   {file}")),
            }
        }
    }
    lines.extend(step.notes.iter().map(|note| format!("      note      {note}")));
    lines
}

// ---------------------------------------------------------------- staging and installing

/// The directory downloads are verified in before anything else changes. It sits in the
/// instance directory: the same file system as `mods/`, so that a verified file can be
/// linked into place. Its name starts with a dot, which no directory handed to the sandbox
/// can (`paths::validate_file_name`). It is removed when this goes out of scope, whatever
/// the outcome.
struct Staging(PathBuf);

impl Staging {
    fn create(instance_dir: &Path) -> Result<Self> {
        let dir = instance_dir.join(format!("{STAGING_PREFIX}{}", util::hex_encode(&util::random_bytes::<8>()?)));
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&dir).ctx(|| format!("creating {}", dir.display()))?;
        Ok(Staging(dir))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Remove staging directories no destructor got to (the process was killed). Only ones old
/// enough that no running command can own them.
fn sweep_stale(instance_dir: &Path) {
    const STALE_AFTER: Duration = Duration::from_secs(3600);
    let Ok(entries) = fs::read_dir(instance_dir) else { return };
    for entry in entries.flatten() {
        let ours = entry.file_name().to_string_lossy().starts_with(STAGING_PREFIX);
        let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
        let stale = meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > STALE_AFTER);
        if ours && meta.file_type().is_dir() && stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

struct Staged {
    /// Index of its step in the plan.
    step: usize,
    path: PathBuf,
    file: VersionFile,
    sha256: String,
}

/// Look inside what was downloaded, before anything is installed.
///
/// * Every file must be readable as a jar.
/// * A jar is replaced only if it and its replacement declare the same mod id. The
///   registry's word ("this hash is an older build of that project") is what proposes a
///   replacement; this is the local check on it, so that a wrong or dishonest answer to a
///   hash lookup cannot talk the launcher into deleting an unrelated jar.
/// * Nothing may bring a mod id that a jar which stays already provides. For a mod the
///   user asked for that is an error; a dependency is then not added (what is there is
///   taken to be it, and the loader has the last word).
fn settle(
    plan: &mut Plan,
    staged: Vec<Staged>,
    present: &[Present],
    mods_dir: &Path,
    out: &mut Output,
) -> Result<Vec<Staged>> {
    // The mod id each jar in mods/ declares; none if it declares none or cannot be read.
    let declared: Vec<(&str, Option<String>)> = present
        .iter()
        .map(|jar| {
            let report = capabilities::scan_jar(&mods_dir.join(&jar.file));
            (jar.file.as_str(), report.ok().and_then(|report| report.mod_id))
        })
        .collect();
    let named = |id: Option<&str>| id.map(|id| format!("`{id}`")).unwrap_or_else(|| "none".into());
    let mut installing = Vec::new();
    let mut brought: BTreeMap<String, String> = BTreeMap::new();
    for item in staged {
        let report = capabilities::scan_jar(&item.path).map_err(|e| {
            Error::Format(format!(
                "{} matches the registry's hash but cannot be read as a jar ({e}). Nothing was changed.",
                item.file.filename
            ))
        })?;
        let title = plan.steps[item.step].project.title.clone();
        if let Action::Add { replaces, .. } = &plan.steps[item.step].action {
            for old in replaces.iter().map(|old| old.file.as_str()) {
                let old_id = declared.iter().find(|(file, _)| *file == old).and_then(|(_, id)| id.as_deref());
                if old_id.is_none() || old_id != report.mod_id.as_deref() {
                    return Err(Error::Policy(format!(
                        "the registry calls mods/{old} a build of {title}, yet that jar and {} do not declare the \
                         same mod id ({} and {}). Nothing was changed; if mods/{old} is to go, remove it yourself.",
                        item.file.filename,
                        named(old_id),
                        named(report.mod_id.as_deref())
                    )));
                }
            }
        }
        let Some(mod_id) = report.mod_id else {
            installing.push(item);
            continue;
        };
        let removed = plan.removed();
        let clash = declared
            .iter()
            .find(|(file, id)| id.as_deref() == Some(mod_id.as_str()) && !removed.contains(*file))
            .map(|(file, _)| file.to_string());
        let step = &mut plan.steps[item.step];
        if let Some(file) = clash {
            match &step.why {
                Why::Requested => {
                    return Err(Error::Policy(format!(
                        "mods/{file} already provides the mod id `{mod_id}`, and the registry does not know that jar \
                         as a build of {title}. Nothing was changed; remove it yourself if {title} is meant to \
                         replace it."
                    )));
                }
                Why::RequiredBy(dependent) => {
                    out.say(format!(
                        "not adding {}: mods/{file} already provides the mod id `{mod_id}`. Whether it satisfies \
                         {dependent} is for the loader to say.",
                        item.file.filename
                    ));
                    step.action = Action::Keep { file, version: None };
                    continue;
                }
            }
        }
        if let Some(other) = brought.insert(mod_id.clone(), item.file.filename.clone()) {
            return Err(Error::Policy(format!(
                "{other} and {} both provide the mod id `{mod_id}`. Nothing was changed.",
                item.file.filename
            )));
        }
        installing.push(item);
    }
    Ok(installing)
}

/// Are two existing paths on one file system? (Hard links do not cross file systems, nor
/// btrfs subvolumes; both show as different device numbers.)
fn same_file_system(a: &Path, b: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let device =
        |path: &Path| fs::metadata(path).map(|meta| meta.dev()).ctx(|| format!("inspecting {}", path.display()));
    Ok(device(a)? == device(b)?)
}

/// Move the verified files into `mods/` and take out the jars they replace. The signature
/// goes first: from here on it describes a state that is being taken apart.
///
/// What can be known to fail is found out before that: a `mods/` that is not a plain
/// directory of the user's, or one the staged files cannot be linked into. An error after
/// the first removal leaves `mods/` partly updated, and says so.
fn install(instance_dir: &Path, plan: &Plan, staged: &[Staged]) -> Result<()> {
    let mods_dir = instance_dir.join("mods");
    crate::instance::ensure_plain_dir(&mods_dir)?;
    if let Some(staging) = staged.first().and_then(|item| item.path.parent())
        && !same_file_system(staging, &mods_dir)?
    {
        return Err(Error::Policy(format!(
            "{} is on another file system (or subvolume) than the instance directory, so a verified file cannot \
             be linked into it. Nothing was changed; with this layout, download the jars yourself and use `profile \
             lock`.",
            mods_dir.display()
        )));
    }
    match fs::remove_file(instance_dir.join(SIGNATURE_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(Error::io(format!("removing the old signature of {}", instance_dir.display()), e));
        }
        _ => {}
    }
    let partial = |e: Error| {
        Error::Other(format!(
            "{e}. mods/ may be partly updated: `bastion-mc profile lock` records what is there now, for you to review."
        ))
    };
    for old in plan.removed() {
        fs::remove_file(mods_dir.join(&old)).ctx(|| format!("removing the replaced mods/{old}")).map_err(partial)?;
    }
    for item in staged {
        // A hard link never replaces: a name that is taken after all is an error, not a loss.
        fs::hard_link(&item.path, mods_dir.join(&item.file.filename))
            .ctx(|| format!("placing {} in mods/", item.file.filename))
            .map_err(partial)?;
    }
    File::open(&mods_dir).and_then(|dir| dir.sync_all()).ctx(|| "flushing mods/").map_err(partial)
}

/// What the static scan says about the jars in `files`, in the form `profile audit` uses.
fn capability_lines(
    scanned: &BTreeMap<String, (String, capabilities::JarReport)>,
    files: &BTreeSet<String>,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (identity, (file, report)) in scanned.iter().filter(|(_, (file, _))| files.contains(file)) {
        lines.push(format!(
            "  {identity}  ({file}: {} classes, {} nested jar(s))",
            report.classes,
            report.nested.len()
        ));
        if report.capabilities.is_empty() && report.natives.is_empty() {
            lines.push("      nothing on the watch list".into());
        }
        for (capability, classes) in &report.capabilities {
            let total = report.counts.get(capability).copied().unwrap_or(classes.len());
            let meaning = capabilities::describe(capability.trim_end_matches(" (named in a string)"));
            let more = if total > classes.len() { format!(", … {total} classes in all") } else { String::new() };
            lines.push(format!("      {capability} - {meaning}: {}{more}", classes.join(", ")));
        }
        lines.extend(report.natives.iter().map(|native| format!("      bundled native library: {native}")));
        lines.extend(report.unreadable.iter().map(|problem| format!("      could not be read: {problem}")));
    }
    lines
}

// ---------------------------------------------------------------- the two commands

/// `profile add`: resolve `wanted` for the profile `id`, show the plan, and — unless it is
/// a dry run or there is nothing to add — download, verify, install and re-lock the draft.
/// Returns whether the profile was changed. `now` is seconds since the epoch.
pub async fn add<T: Transport>(
    paths: &Paths,
    transport: &T,
    id: &str,
    wanted: &[String],
    options: Options,
    now: u64,
    out: &mut Output,
) -> Result<bool> {
    if wanted.is_empty() {
        return Err(Error::Usage("name at least one mod: an alias, a Modrinth slug or a project id".into()));
    }
    if wanted.len() > MAX_REQUESTED {
        return Err(Error::Usage(format!("at most {MAX_REQUESTED} mods per command")));
    }
    // A name that cannot be one is refused before the registry hears anything - the
    // first request of this command carries the hash of every jar in mods/.
    for name in wanted.iter().filter(|name| alias(name).is_none()) {
        modrinth::check_key(name)?;
    }
    let draft = draft(paths, id)?;
    let mods_dir = draft.dir.join("mods");
    let registry = Registry::new(transport);
    let rules = Rules {
        minecraft: &draft.manifest.minecraft.version,
        loader: draft.loader,
        options,
        local: now,
        registry_clock: registry.clock(),
    };

    let mut present = inventory(&mods_dir)?;
    identify(&registry, &mut present).await?;
    let mut plan = resolve(&registry, wanted, &present, &rules).await?;
    check_names(&plan, &present)?;

    out.say(format!("profile {id}: {}. Registry: Modrinth ({}).", rules.target(), modrinth::API_HOST));
    for step in &plan.steps {
        for line in describe(step, &rules) {
            out.say(line);
        }
    }
    for note in &plan.notes {
        out.say(format!("note: {note}"));
    }
    if let Some(note) = clock_note(now, registry.server_time()) {
        out.say(note);
    }
    const UNTOUCHED: &str = "mods/, the manifest and its signature were not touched.";
    if plan.additions() == 0 {
        out.say(format!("Nothing to add: {UNTOUCHED}"));
        return Ok(false);
    }
    if options.dry_run {
        out.say(format!("--dry-run: nothing was downloaded; {UNTOUCHED}"));
        out.say(
            "The dates above are those of the builds. The file server's date for each file is read when the file is \
             downloaded; bytes it has held for less than 48 hours are then refused as well.",
        );
        return Ok(false);
    }

    sweep_stale(&draft.dir);
    let staging = Staging::create(&draft.dir)?;
    let mut staged = Vec::new();
    for (index, step) in plan.steps.iter().enumerate() {
        let Action::Add { version, file, .. } = &step.action else { continue };
        out.say(format!("downloading {} ({} bytes)", file.filename, file.size));
        let path = staging.0.join(&file.filename);
        let downloaded = registry.download(file, &path).await.map_err(|e| within(&step.project.title, e))?;
        // The file's own date arrives with the file; a refusal here still changes nothing.
        if let Some(remark) = file_date(&step.project, version, file, downloaded.stored, &rules)? {
            out.say(remark);
        }
        staged.push(Staged { step: index, path, file: file.clone(), sha256: downloaded.sha256 });
    }
    out.say(format!(
        "{} file(s) downloaded and verified: size, SHA-512 and SHA-1 are the ones the registry lists.",
        staged.len()
    ));
    let staged = settle(&mut plan, staged, &present, &mods_dir, out)?;
    if staged.is_empty() {
        out.say(format!("Nothing left to add: {UNTOUCHED}"));
        return Ok(false);
    }

    // Jars that were in mods/ without being in the manifest get recorded too (this ends
    // like `profile lock`); they are named, so that nothing slips in unremarked.
    let listed = |jar: &Present| draft.manifest.mods.iter().any(|m| m.file == jar.file && m.sha256 == jar.sha256);
    let removed = plan.removed();
    let unrecorded: Vec<&str> = present
        .iter()
        .filter(|jar| !removed.contains(&jar.file) && !listed(jar))
        .map(|jar| jar.file.as_str())
        .collect();

    install(&draft.dir, &plan, &staged)?;
    let sources: BTreeMap<String, String> =
        staged.iter().map(|item| (item.file.filename.clone(), item.file.url.clone())).collect();
    let manifest = author::lock_with(paths, id, &sources).map_err(|e| {
        Error::Other(format!(
            "the new files are in mods/ but the manifest could not be updated ({e}). The profile is unsigned; \
             `bastion-mc profile lock {id}` records what is in mods/ now."
        ))
    })?;
    for item in &staged {
        let recorded = manifest.mods.iter().find(|m| m.file == item.file.filename);
        if !recorded.is_some_and(|m| m.sha256 == item.sha256 && m.size == item.file.size) {
            return Err(Error::Integrity(format!(
                "mods/{} is not the file that was downloaded and verified a moment ago; something else is writing \
                 to this directory. The profile is unsigned; inspect mods/ before going on.",
                item.file.filename
            )));
        }
    }
    drop(staging);

    out.say(format!("profile {id} is now at serial {} with {} mod(s):", manifest.serial, manifest.mods.len()));
    for entry in &manifest.mods {
        let origin = match &entry.source {
            Some(_) if sources.contains_key(&entry.file) => "  (added now)",
            Some(_) => "  (source recorded)",
            None => "",
        };
        out.say(format!("  {}  {:>10} bytes  {}{origin}", entry.sha256, entry.size, entry.file));
    }
    if !unrecorded.is_empty() {
        out.say(format!(
            "also recorded, because they were in mods/ without matching the manifest: {}",
            unrecorded.join(", ")
        ));
    }

    // What the new jars can reach. Informational, as in `profile lock`.
    let added: BTreeSet<String> = sources.keys().cloned().collect();
    match capabilities::scan_mods(&mods_dir) {
        Ok(scanned) => {
            out.say("");
            out.say("What was added refers to (static scan of the class files; a signal for review, not proof):");
            for line in capability_lines(&scanned, &added) {
                out.say(line);
            }
            let before = capabilities::Recorded::load(&draft.dir);
            let news: Vec<String> =
                capabilities::changes(&before, &scanned).into_iter().filter(|line| line.contains(": NEW ")).collect();
            if !news.is_empty() {
                out.say("New since the last lock, in jars that were there before:");
                for line in news {
                    out.say(format!("  {line}"));
                }
            }
            capabilities::Recorded::of(&scanned).save(&draft.dir)?;
            out.say(format!(
                "`bastion-mc profile audit {id}` shows every jar. Reflection, classes defined at run time and"
            ));
            out.say("native code can hide anything else; the scan informs the decision to sign, the sandbox enforces.");
        }
        Err(e) => out.say(format!("note: the mods could not be scanned for API use: {e}")),
    }
    out.say("");
    out.say(format!(
        "The previous signature (if any) was removed and nothing was signed. Review {} and the scan above, then:",
        draft.dir.join(MANIFEST_FILE).display()
    ));
    out.say(format!("  bastion-mc profile sign {id} --key <your key>"));
    Ok(true)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CheckSummary {
    pub jars: usize,
    pub known: usize,
    pub unknown: usize,
    pub fresh: usize,
}

/// `profile check`: what the registry knows about each jar in `mods/`. Changes nothing.
pub async fn check<T: Transport>(
    paths: &Paths,
    transport: &T,
    id: &str,
    now: u64,
    out: &mut Output,
) -> Result<CheckSummary> {
    let dir = paths.instance(id)?;
    if !dir.is_dir() {
        return Err(Error::Usage(format!("no instance called {id:?}")));
    }
    // Read as a draft: this is a report, not a launch, and it is wanted before signing.
    let manifest = if dir.join(MANIFEST_FILE).exists() { Some(author::read_draft(paths, id)?.1) } else { None };
    let mut present = inventory(&dir.join("mods"))?;
    let mut summary = CheckSummary { jars: present.len(), ..Default::default() };
    if present.is_empty() {
        out.say(format!("{id}: no mod jars"));
        return Ok(summary);
    }
    let registry = Registry::new(transport);
    identify(&registry, &mut present).await?;
    let ids: Vec<String> = present
        .iter()
        .filter_map(|jar| jar.known.as_ref().map(|v| v.project_id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut projects = BTreeMap::new();
    for chunk in ids.chunks(modrinth::MAX_IDS) {
        projects.extend(registry.projects(chunk).await?.into_iter().map(|p| (p.id.clone(), p)));
    }
    let target = manifest.as_ref().and_then(|m| m.loader.as_ref().map(|l| (m.minecraft.version.as_str(), l.kind)));
    let local = now;
    let now = registry.now(local);

    out.say(format!(
        "{id}: {} jar(s) in mods/, looked up at Modrinth ({}) by SHA-512",
        present.len(),
        modrinth::API_HOST
    ));
    for jar in &present {
        out.say(format!("  {}", jar.file));
        let recorded = manifest.as_ref().and_then(|m| m.mods.iter().find(|entry| entry.file == jar.file));
        let mut flags = Vec::new();
        match &jar.known {
            None => {
                summary.unknown += 1;
                out.say(format!("      UNKNOWN to the registry (sha512 {})", jar.sha512));
                if let Some(source) = recorded.and_then(|entry| entry.source.as_deref())
                    && source.starts_with("https://cdn.modrinth.com/")
                {
                    flags.push(format!(
                        "the manifest records {} as its source, yet the registry lists no file with this hash \
                         now: a build that was withdrawn looks like this",
                        util::sanitize_line(source.as_bytes(), 200)
                    ));
                }
            }
            Some(version) => {
                summary.known += 1;
                let project = projects.get(&version.project_id);
                let name = match project {
                    Some(p) => format!("{} ({} · project {} · licence {})", p.title, p.slug, p.id, p.license),
                    None => format!("project {}", version.project_id),
                };
                out.say(format!(
                    "      known: {name}, version {} [{}], published {}, {}",
                    version.number,
                    version.channel.name(),
                    modrinth::utc(version.published),
                    modrinth::age(now, version.published)
                ));
                if version.published.saturating_add(QUARANTINE_SECS) > now {
                    summary.fresh += 1;
                    flags.push("FRESH: published less than 48 hours ago".into());
                }
                if version.channel != Channel::Release {
                    flags.push(format!("a pre-release build ({})", version.channel.name()));
                }
                if !version.is_listed() {
                    flags.push(format!("the registry marks this build as {}", version.status));
                }
                match project {
                    None => flags.push("its project is not public at the registry (any more)".into()),
                    Some(p) => {
                        if p.status != "approved" {
                            flags.push(format!("the registry lists the project with the status {:?}", p.status));
                        }
                        if p.kind != "mod" {
                            flags.push(format!("the registry lists the project as a {}, not a mod", p.kind));
                        }
                    }
                }
                if let Some((minecraft, loader)) = target
                    && !version.supports(minecraft, loader_names(loader))
                {
                    let some = |list: &[String]| {
                        let shown: Vec<&str> = list.iter().take(6).map(String::as_str).collect();
                        format!("{}{}", shown.join(", "), if list.len() > shown.len() { ", …" } else { "" })
                    };
                    flags.push(format!(
                        "the registry lists this build for Minecraft {} on {}, not for this profile's {minecraft} \
                         on {}",
                        some(&version.game_versions),
                        some(&version.loaders),
                        loader_names(loader)[0]
                    ));
                }
                if let Some(name) = version.file_name_for(&jar.sha512).filter(|name| *name != jar.file) {
                    flags.push(format!("the registry's name for this file is {name}"));
                }
                let listed_at = version.file_with_sha512(&jar.sha512).and_then(|file| file.ok()).map(|file| file.url);
                if let Some(source) = recorded.and_then(|entry| entry.source.as_deref())
                    && listed_at.as_deref() != Some(source)
                {
                    flags.push(format!(
                        "the manifest records another source for it: {}",
                        util::sanitize_line(source.as_bytes(), 200)
                    ));
                }
            }
        }
        if manifest.is_some() && !recorded.is_some_and(|entry| entry.sha256 == jar.sha256 && entry.size == jar.size) {
            flags.push("not what the manifest lists (`profile lock` records what is in mods/)".into());
        }
        for flag in flags {
            out.say(format!("      flag: {flag}"));
        }
    }
    out.say(format!(
        "{} known, {} unknown, {} published less than 48 hours ago.",
        summary.known, summary.unknown, summary.fresh
    ));
    if let Some(note) = clock_note(local, registry.server_time()) {
        out.say(note);
    }
    out.say("");
    for line in CHECK_CAVEAT.lines() {
        out.say(line);
    }
    Ok(summary)
}

/// The command line of `profile add <id> <mod>… [flags]` and `profile check <id>`, from
/// the point where `cli` has read the subcommand and the instance id.
pub fn command(parser: &mut lexopt::Parser, paths: &Paths, sub: &str, id: &str) -> Result<()> {
    let usage = |message: String| Error::Usage(format!("{message}\n\nRun `bastion-mc --help` for usage."));
    let (mut wanted, mut options) = (Vec::new(), Options::default());
    while let Some(argument) = parser.next().map_err(|e| usage(e.to_string()))? {
        match argument {
            lexopt::Arg::Value(value) if sub == "add" => {
                wanted.push(value.into_string().map_err(|_| usage("a mod name is not valid UTF-8".into()))?)
            }
            lexopt::Arg::Long("prerelease") if sub == "add" => options.prerelease = true,
            lexopt::Arg::Long("allow-fresh") if sub == "add" => options.allow_fresh = true,
            lexopt::Arg::Long("dry-run") if sub == "add" => options.dry_run = true,
            other => return Err(usage(format!("unexpected {} for `profile {sub}`", argument_name(&other)))),
        }
    }
    if sub == "add" && wanted.is_empty() {
        let aliases: Vec<&str> = ALIASES.iter().map(|a| a.name).collect();
        return Err(usage(format!(
            "profile add needs at least one mod: a Modrinth slug or project id, or an alias ({})",
            aliases.join(", ")
        )));
    }
    let cfg = Config::load(paths)?;
    let live = modrinth::Live::new(&cfg)?;
    let runtime =
        tokio::runtime::Builder::new_multi_thread().enable_all().build().ctx(|| "starting the async runtime")?;
    let mut out = Output::terminal();
    runtime.block_on(async {
        match sub {
            "add" => add(paths, &live, id, &wanted, options, unix_now(), &mut out).await.map(|_| ()),
            _ => check(paths, &live, id, unix_now(), &mut out).await.map(|_| ()),
        }
    })
}

fn argument_name(argument: &lexopt::Arg<'_>) -> String {
    match argument {
        lexopt::Arg::Long(name) => format!("option --{name}"),
        lexopt::Arg::Short(letter) => format!("option -{letter}"),
        lexopt::Arg::Value(value) => format!("argument {:?}", value.to_string_lossy()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        instance,
        profile::{
            capabilities::tests::{class_file, jar},
            manifest::tests::sample,
            modrinth::testing::{Scripted, dependency, file, file_path, project, sha512_hex, version},
        },
    };
    use serde_json::{Value, json};

    /// 2026-11-14T00:00:00Z.
    const NOW: u64 = 1_794_614_400;
    const OLD: &str = "2026-09-01T00:00:00Z";
    const LATER: &str = "2026-10-01T00:00:00Z";
    const RECENT: &str = "2026-11-09T00:00:00Z";
    /// Exactly 48 hours before [`NOW`]: no longer fresh.
    const EDGE: &str = "2026-11-12T00:00:00Z";
    const FRESH_47H: &str = "2026-11-12T00:00:01Z";
    const FRESH_12H: &str = "2026-11-13T12:00:00Z";
    const TOMORROW: &str = "2026-11-15T00:00:00Z";

    struct Fixture {
        root: PathBuf,
        paths: Paths,
        dir: PathBuf,
        registry: Scripted,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A draft Fabric profile `perf` for Minecraft 1.21.4 with an empty `mods/`, a signature
    /// file (so that "untouched" and "removed" can be told apart) and a registry that
    /// knows no hash.
    fn fixture(tag: &str) -> Fixture {
        let root = std::env::temp_dir()
            .join(format!("bastion-catalog-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("c"),
            data: root.join("d"),
            cache: root.join("k"),
            state: root.join("s"),
            runtime: None,
        };
        paths.ensure().unwrap();
        let dir = instance::create(&paths, "perf", None).unwrap();
        fs::create_dir(dir.join("mods")).unwrap();
        let mut draft = sample("perf");
        draft.mods.clear();
        fs::write(dir.join(MANIFEST_FILE), draft.to_bytes().unwrap()).unwrap();
        fs::write(dir.join(SIGNATURE_FILE), "a signature\n").unwrap();
        let registry = Scripted::default();
        registry.json("POST", "/v2/version_files", &json!({}));
        Fixture { root, paths, dir, registry }
    }

    impl Fixture {
        async fn add(&self, wanted: &[&str], options: Options) -> (Result<bool>, String) {
            let wanted: Vec<String> = wanted.iter().map(|w| w.to_string()).collect();
            let mut out = Output::silent();
            let outcome = add(&self.paths, &self.registry, "perf", &wanted, options, NOW, &mut out).await;
            (outcome, out.lines.join("\n"))
        }

        /// Add with default options; the command must succeed and change the profile.
        async fn added(&self, wanted: &[&str]) -> String {
            let (outcome, text) = self.add(wanted, Options::default()).await;
            assert!(outcome.unwrap_or_else(|e| panic!("{e}\n{text}")), "nothing was changed:\n{text}");
            text
        }

        fn manifest(&self) -> ProfileManifest {
            serde_json::from_slice(&fs::read(self.dir.join(MANIFEST_FILE)).unwrap()).unwrap()
        }

        fn mods(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.dir.join("mods"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }

        /// Put a jar into `mods/` as if copied there by hand.
        fn drop_in(&self, name: &str, content: &[u8]) {
            fs::write(self.dir.join("mods").join(name), content).unwrap();
        }

        /// Make the registry recognise these files by their hash.
        fn known(&self, builds: &[&Build]) {
            let map: serde_json::Map<String, Value> =
                builds.iter().map(|b| (sha512_hex(&b.content), b.json.clone())).collect();
            self.registry.json("POST", "/v2/version_files", &Value::Object(map));
        }

        /// Every file below the instance directory with its content.
        fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
            fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
                for entry in fs::read_dir(dir).unwrap() {
                    let path = entry.unwrap().path();
                    let name = path.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                    if path.is_dir() {
                        out.push((format!("{name}/"), Vec::new()));
                        walk(&path, base, out);
                    } else {
                        out.push((name, fs::read(&path).unwrap()));
                    }
                }
            }
            let mut out = Vec::new();
            walk(&self.dir, &self.dir, &mut out);
            out.sort();
            out
        }
    }

    struct Build {
        json: Value,
        content: Vec<u8>,
        path: String,
        name: String,
    }

    /// A jar that declares the Fabric mod id `mod_id` and refers to `refers`.
    fn mod_jar(mod_id: &str, salt: &str, refers: &[&str]) -> Vec<u8> {
        jar(&[
            ("fabric.mod.json", format!(r#"{{"schemaVersion":1,"id":"{mod_id}","version":"{salt}"}}"#).into_bytes()),
            (&format!("{mod_id}/Main.class"), class_file(&format!("{mod_id}/Main"), refers, &[], &[salt])),
        ])
    }

    /// A build of `project` (its id) whose file is `<stem>-<number>.jar`.
    fn build(project: &str, id: &str, stem: &str, number: &str, channel: &str, published: &str) -> Build {
        let content = mod_jar(stem, number, &["java/util/ArrayList"]);
        build_of(project, id, stem, number, channel, published, content)
    }

    fn build_of(
        project: &str,
        id: &str,
        stem: &str,
        number: &str,
        channel: &str,
        published: &str,
        content: Vec<u8>,
    ) -> Build {
        let name = format!("{stem}-{number}.jar");
        Build {
            json: version(id, project, number, channel, published, file(project, id, &name, &content)),
            path: file_path(project, id, &name),
            content,
            name,
        }
    }

    impl Build {
        fn with(mut self, change: impl FnOnce(&mut Value)) -> Self {
            change(&mut self.json);
            self
        }

        fn requires(self, dependencies: Vec<Value>) -> Self {
            self.with(|json| json["dependencies"] = json!(dependencies))
        }
    }

    /// Publish a project and its builds: the project by id and slug, its list of builds,
    /// each build by id, each file on the file server.
    fn offer(registry: &Scripted, project: &Value, builds: &[&Build]) {
        let (id, slug) = (project["id"].as_str().unwrap(), project["slug"].as_str().unwrap());
        registry.json("GET", &format!("/v2/project/{id}"), project).json(
            "GET",
            &format!("/v2/project/{slug}"),
            project,
        );
        let list: Vec<Value> = builds.iter().map(|b| b.json.clone()).collect();
        registry.json("GET", &format!("/v2/project/{id}/version"), &json!(list));
        for b in builds {
            registry.on("GET", &b.path, 200, b.content.clone());
            // As on the real file server: the file is dated a moment after its build.
            if let Ok(published) = modrinth::parse_timestamp(b.json["date_published"].as_str().unwrap_or_default()) {
                registry.stored(&b.path, Some(published + 1));
            }
            registry.json("GET", &format!("/v2/version/{}", b.json["id"].as_str().unwrap()), &b.json);
        }
    }

    const A: &str = "AAAAAAAA";
    const B: &str = "BBBBBBBB";
    const C: &str = "CCCCCCCC";
    const D: &str = "DDDDDDDD";

    #[tokio::test]
    async fn the_newest_release_for_the_profile_is_taken_whatever_the_server_lists() {
        let f = fixture("newest");
        let wanted = build(A, "A0000002", "alpha", "2.0", "release", RECENT);
        // The server "forgot" to filter and to sort: other game versions, another loader,
        // a withdrawn build, a pre-release and a channel nobody knows — all newer.
        let others = [
            build(A, "A0000001", "alpha", "1.0", "release", OLD),
            build(A, "A0000003", "alpha", "3.0-beta", "beta", EDGE),
            build(A, "A0000004", "alpha", "3.0-old-mc", "release", EDGE)
                .with(|j| j["game_versions"] = json!(["1.20.1", "1.21.3"])),
            build(A, "A0000005", "alpha", "3.0-forge", "release", EDGE)
                .with(|j| j["loaders"] = json!(["forge", "neoforge"])),
            build(A, "A0000006", "alpha", "3.0-gone", "release", EDGE).with(|j| j["status"] = json!("archived")),
            build(A, "A0000007", "alpha", "3.0-nightly", "release", EDGE)
                .with(|j| j["version_type"] = json!("nightly")),
            build(A, "A0000008", "alpha", "3.0-quilt", "release", EDGE).with(|j| j["loaders"] = json!(["quilt"])),
        ];
        let mut all: Vec<&Build> = others.iter().collect();
        all.insert(3, &wanted);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &all);

        let text = f.added(&["alpha"]).await;
        assert!(text.contains("add   Alpha Mod (alpha · project AAAAAAAA · licence MIT) - asked for"), "{text}");
        assert!(text.contains("version   2.0 [release], published 2026-11-09 00:00 UTC, 5 days ago"), "{text}");
        assert!(text.contains(&format!("sha512    {}", sha512_hex(&wanted.content))), "{text}");
        assert!(text.contains("from      https://cdn.modrinth.com/data/AAAAAAAA/versions/A0000002/alpha-2.0.jar"));
        assert_eq!(f.mods(), ["alpha-2.0.jar"]);
        assert_eq!(fs::read(f.dir.join("mods/alpha-2.0.jar")).unwrap(), wanted.content);
        // The profile's game version and loader were what the registry was asked for.
        assert_eq!(f.registry.count("/version?loaders=%5B%22fabric%22%5D&game_versions=%5B%221.21.4%22%5D"), 1);
        // Asking for pre-releases does not prefer them: there is a release.
        let again = fixture("newest-pre");
        offer(&again.registry, &project(A, "alpha", "Alpha Mod"), &all);
        let (outcome, text) = again.add(&["alpha"], Options { prerelease: true, ..Default::default() }).await;
        assert!(outcome.unwrap());
        assert_eq!(again.mods(), ["alpha-2.0.jar"], "{text}");
    }

    #[tokio::test]
    async fn the_draft_ends_as_after_a_lock_with_a_source_and_without_a_signature() {
        let f = fixture("draft");
        let one = build(A, "A0000001", "alpha", "1.0+mc1.21", "release", OLD);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&one]);
        assert_eq!(f.manifest().serial, 1);
        let text = f.added(&["alpha"]).await;

        let manifest = f.manifest();
        manifest.validate("perf").unwrap();
        assert_eq!(manifest.serial, 2, "the serial is bumped");
        assert_eq!(manifest.mods.len(), 1);
        let entry = &manifest.mods[0];
        assert_eq!(entry.file, "alpha-1.0+mc1.21.jar");
        assert_eq!(entry.size, one.content.len() as u64);
        assert_eq!(entry.sha256, util::hex_encode(&util::hash_bytes(util::HashAlg::Sha256, &one.content)));
        assert_eq!(
            entry.source.as_deref(),
            Some("https://cdn.modrinth.com/data/AAAAAAAA/versions/A0000001/alpha-1.0%2Bmc1.21.jar")
        );
        assert!(!f.dir.join(SIGNATURE_FILE).exists(), "the old signature is removed");
        assert!(text.contains("profile perf is now at serial 2 with 1 mod(s):"), "{text}");
        assert!(
            text.contains(&format!("{}  {:>10} bytes  alpha-1.0+mc1.21.jar  (added now)", entry.sha256, entry.size))
        );
        assert!(text.contains("nothing was signed") && text.contains("bastion-mc profile sign perf --key"), "{text}");
        // Nothing of the staging area is left, and the file is private.
        assert!(!f.snapshot().iter().any(|(name, _)| name.starts_with(STAGING_PREFIX)), "{:?}", f.snapshot());
        // A plain lock afterwards finds nothing to change, and keeps the source.
        let locked = author::lock(&f.paths, "perf").unwrap();
        assert_eq!((locked.serial, &locked.mods), (2, &manifest.mods));
        // What it can reach was scanned and shown, and recorded for the next comparison.
        assert!(text.contains("alpha  (alpha-1.0+mc1.21.jar: 1 classes, 0 nested jar(s))"), "{text}");
        assert!(text.contains("nothing on the watch list"), "{text}");
        assert!(capabilities::Recorded::load(&f.dir).mods.contains_key("alpha"));
        // Asking again changes nothing at all — not even the serial.
        fs::write(f.dir.join(SIGNATURE_FILE), "signed again\n").unwrap();
        f.known(&[&one]);
        let before = f.snapshot();
        let (outcome, text) = f.add(&["alpha"], Options::default()).await;
        assert!(!outcome.unwrap());
        assert!(text.contains("keep  Alpha Mod") && text.contains("Nothing to add"), "{text}");
        assert_eq!(f.snapshot(), before);
    }

    #[tokio::test]
    async fn what_was_added_is_scanned_and_shown_even_next_to_a_jar_that_goes_by_the_same_name() {
        let f = fixture("scan-shown");
        // Copied in by hand, unknown to the registry, without a mod id: the scan files it
        // under its file name up to the version - "alpha", which is the new mod's id.
        f.drop_in("alpha-9.jar", &jar(&[("b/B.class", class_file("b/B", &["java/lang/ProcessBuilder"], &[], &[]))]));
        let new =
            build_of(A, "A0000001", "alpha", "1.0", "release", OLD, mod_jar("alpha", "1.0", &["java/net/Socket"]));
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&new]);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "alpha-9.jar"]);
        assert!(text.contains("alpha  (alpha-1.0.jar: 1 classes, 0 nested jar(s))"), "{text}");
        assert!(text.contains("network - opens its own network connections: alpha.Main"), "{text}");
        assert!(!text.contains("runs programs"), "the jar that was not added is not paraded:\n{text}");
        // Both are on record for the next comparison, and in the manifest.
        assert_eq!(capabilities::Recorded::load(&f.dir).mods.len(), 2);
        assert_eq!(f.manifest().mods.len(), 2);
    }

    #[tokio::test]
    async fn pre_releases_are_refused_unless_asked_for() {
        let f = fixture("pre");
        let beta = build(A, "A0000001", "alpha", "2.0-beta.1", "beta", OLD);
        let alpha = build(A, "A0000002", "alpha", "2.0-alpha.9", "alpha", RECENT);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&alpha, &beta]);
        let before = f.snapshot();

        let (outcome, _) = f.add(&["alpha"], Options::default()).await;
        let message = match outcome {
            Err(Error::Policy(message)) => message,
            other => panic!("{other:?}"),
        };
        assert!(message.contains("only pre-release builds for Minecraft 1.21.4 on Fabric"), "{message}");
        assert!(message.contains("2.0-alpha.9 (alpha, published 2026-11-09 00:00 UTC, 5 days ago)"), "{message}");
        assert!(message.contains("--prerelease"), "{message}");
        assert_eq!(f.snapshot(), before);
        assert_eq!(f.registry.count("cdn.modrinth.com"), 0, "nothing was fetched");

        // Asked for: the more stable channel that has a build, not simply the newest upload.
        let (outcome, text) = f.add(&["alpha"], Options { prerelease: true, ..Default::default() }).await;
        assert!(outcome.unwrap(), "{text}");
        assert_eq!(f.mods(), ["alpha-2.0-beta.1.jar"]);
        assert!(text.contains("version   2.0-beta.1 [beta]"), "{text}");
        assert!(
            text.contains("note      PRE-RELEASE (beta): there is no release build for Minecraft 1.21.4 on Fabric")
        );
        // Alphas when there is nothing else.
        let g = fixture("pre-alpha");
        offer(&g.registry, &project(A, "alpha", "Alpha Mod"), &[&alpha]);
        let (outcome, text) = g.add(&["alpha"], Options { prerelease: true, ..Default::default() }).await;
        assert!(outcome.unwrap() && text.contains("PRE-RELEASE (alpha)"), "{text}");
    }

    #[tokio::test]
    async fn a_build_younger_than_48_hours_is_passed_over_unless_allowed() {
        assert_eq!(QUARANTINE_SECS, 48 * 3600);
        let builds = [
            build(A, "A0000001", "alpha", "1.0", "release", OLD),
            build(A, "A0000002", "alpha", "1.1", "release", EDGE),
            build(A, "A0000003", "alpha", "1.2", "release", FRESH_47H),
            build(A, "A0000004", "alpha", "1.3", "release", FRESH_12H),
        ];
        let all: Vec<&Build> = builds.iter().collect();
        let f = fixture("fresh");
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &all);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.1.jar"], "exactly 48 hours old is old enough:\n{text}");
        assert!(text.contains("note      passed over: 1.3 (published 2026-11-13 12:00 UTC, 12 hours ago) is inside the 48-hour quarantine; --allow-fresh takes it"), "{text}");
        assert!(text.contains("note      passed over: 1.2 (published 2026-11-12 00:00 UTC, 47 hours ago)"), "{text}");
        assert_eq!(f.registry.count("cdn.modrinth.com"), 1, "the fresh builds were not fetched");

        let f = fixture("fresh-allowed");
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &all);
        let (outcome, text) = f.add(&["alpha"], Options { allow_fresh: true, ..Default::default() }).await;
        assert!(outcome.unwrap());
        assert_eq!(f.mods(), ["alpha-1.3.jar"]);
        assert!(
            text.contains("note      FRESH: published less than 48 hours ago; taken because of --allow-fresh"),
            "{text}"
        );

        // Nothing but fresh builds: refused, with the time the quarantine ends.
        let f = fixture("fresh-only");
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &all[2..]);
        let before = f.snapshot();
        let (outcome, _) = f.add(&["alpha"], Options::default()).await;
        let message = match outcome {
            Err(Error::Policy(message)) => message,
            other => panic!("{other:?}"),
        };
        assert!(message.contains("every release build of Alpha Mod") && message.contains("--allow-fresh"), "{message}");
        assert!(
            message.contains("the newest: 1.3") && message.contains("The quarantine ends 2026-11-15 12:00 UTC"),
            "{message}"
        );
        assert_eq!(f.snapshot(), before);

        // A build dated after this machine's "now" is fresh, not old.
        let f = fixture("fresh-future");
        let tomorrow = build(A, "A0000005", "alpha", "9.9", "release", TOMORROW);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&builds[0], &tomorrow]);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar"]);
        assert!(
            text.contains(
                "passed over: 9.9 (published 2026-11-15 00:00 UTC, 24 hours in the future by this machine's clock)"
            ),
            "{text}"
        );
    }

    #[tokio::test]
    async fn the_age_of_a_build_is_measured_against_the_earlier_of_two_clocks() {
        // By this machine's clock the newest build is 49 hours old …
        let publish = |f: &Fixture| {
            let older = build(A, "A0000001", "alpha", "1.0", "release", OLD);
            let newest = build(A, "A0000002", "alpha", "1.1", "release", "2026-11-11T23:00:00Z");
            offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&newest, &older]);
        };
        let f = fixture("clock-none");
        publish(&f);
        f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.1.jar"], "no registry clock: this machine's decides");

        // … but the registry, whose date that is, says it is three hours earlier than we
        // think: 46 hours. This machine's clock runs ahead, and the build is still fresh.
        let f = fixture("clock-behind");
        publish(&f);
        f.registry.clock(Some(NOW - 3 * 3600));
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar"], "{text}");
        assert!(text.contains("passed over: 1.1 (published 2026-11-11 23:00 UTC, 46 hours ago)"), "{text}");
        assert!(
            text.contains(
                "note: this machine's clock (2026-11-14 00:00 UTC) and the registry's (2026-11-13 21:00 UTC) disagree"
            ),
            "{text}"
        );

        // A registry clock that runs ahead does not make anything older than we see it.
        let f = fixture("clock-ahead");
        let older = build(A, "A0000001", "alpha", "1.0", "release", OLD);
        let fresh = build(A, "A0000002", "alpha", "1.1", "release", FRESH_47H);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&fresh, &older]);
        f.registry.clock(Some(NOW + 30 * 86_400));
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar"], "{text}");
        assert!(text.contains("passed over: 1.1") && text.contains("47 hours ago"), "{text}");
        // Clocks that agree to within minutes are not remarked upon.
        let f = fixture("clock-close");
        publish(&f);
        f.registry.clock(Some(NOW - 120));
        assert!(!f.added(&["alpha"]).await.contains("disagree"));
    }

    #[tokio::test]
    async fn bytes_the_file_server_has_held_for_less_than_48_hours_are_refused_whatever_the_age_of_the_build() {
        // The build is ten weeks old. What the file server sends for it is another matter.
        let old = build(A, "A0000001", "alpha", "1.0", "release", OLD);
        let published = modrinth::parse_timestamp(OLD).unwrap();
        let arranged = |tag: &str, stored: Option<u64>| {
            let f = fixture(tag);
            offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&old]);
            f.registry.stored(&old.path, stored);
            f
        };

        // Held for five hours: somebody put this file into the old build today - or the
        // file server rewrote its copy today. Which of the two cannot be known, and is not claimed.
        let f = arranged("file-fresh", Some(NOW - 5 * 3600));
        let before = f.snapshot();
        let (outcome, text) = f.add(&["alpha"], Options::default()).await;
        let message = match outcome {
            Err(Error::Policy(message)) => message,
            other => panic!("{other:?}\n{text}"),
        };
        assert!(
            message.contains(
                "Alpha Mod 1.0: the file server has held its copy of alpha-1.0.jar only since 2026-11-13 19:00 UTC, 5 \
                 hours ago, although the build is dated 2026-09-01 00:00 UTC. Either the file was put into the build \
                 after its publication, or the file server has rewritten its storage"
            ),
            "{message}"
        );
        assert!(
            message.contains("The 48 hours end 2026-11-15 19:00 UTC. --allow-fresh takes the file now"),
            "{message}"
        );
        assert_eq!(f.snapshot(), before, "nothing of the refused file is left");
        assert!(
            text.contains("published 2026-09-01 00:00 UTC, 74 days ago"),
            "the plan had shown an old build:\n{text}"
        );

        // The flag that lifts the quarantine lifts it for the file too, and the output says what was taken.
        let f = arranged("file-fresh-allowed", Some(NOW - 5 * 3600));
        let (outcome, text) = f.add(&["alpha"], Options { allow_fresh: true, ..Default::default() }).await;
        assert!(outcome.unwrap(), "{text}");
        assert!(
            text.contains(
                "note: FRESH FILE: the file server has held its copy of alpha-1.0.jar only since 2026-11-13 19:00 UTC, \
                 5 hours ago, although the build is dated 2026-09-01 00:00 UTC; taken because of --allow-fresh"
            ),
            "{text}"
        );

        // One second short of 48 hours is short of them; 48 hours are enough.
        let f = arranged("file-edge-inside", Some(NOW - QUARANTINE_SECS + 1));
        assert!(matches!(f.add(&["alpha"], Options::default()).await.0, Err(Error::Policy(_))));
        // What the file server has held for 48 hours or more is taken without a word, however
        // much later than the build that date is: the file server also rewrites its copies
        // for reasons of its own (it did on 2026-10-19), so a late date proves nothing.
        // The same for a date at or about the build's own, and for one before it.
        for stored in [
            NOW - QUARANTINE_SECS,
            NOW - 25 * 86_400,
            published + LATE_FILE_SECS + 1,
            published + LATE_FILE_SECS,
            published + 3,
            published,
            published - 86_400,
        ] {
            let f = arranged("file-ordinary", Some(stored));
            let text = f.added(&["alpha"]).await;
            assert!(!text.contains("file server"), "{stored}: {text}");
        }
        // A build that has just left quarantine is not refused for the seconds its upload took.
        let edge = build(A, "A0000003", "alpha", "1.1", "release", EDGE);
        let f = fixture("file-with-its-build");
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&edge]);
        f.registry.stored(&edge.path, Some(NOW - QUARANTINE_SECS + 40));
        let text = f.added(&["alpha"]).await;
        assert!(!text.contains("file server"), "{text}");

        // No date from the file server: the build's date is all there is, and that is said.
        let f = arranged("file-undated", None);
        let text = f.added(&["alpha"]).await;
        assert!(
            text.contains(
                "note: the file server stated no date for alpha-1.0.jar; that these bytes have been at the registry \
                 for 48 hours rests on the date of the build alone"
            ),
            "{text}"
        );

        // The age of a file is measured like the age of a build: against the earlier of the
        // two clocks. 49 hours by this machine's clock are 46 by the registry's.
        let f = arranged("file-clock", Some(NOW - 49 * 3600));
        f.registry.clock(Some(NOW - 3 * 3600));
        let (outcome, _) = f.add(&["alpha"], Options::default()).await;
        assert!(matches!(&outcome, Err(Error::Policy(m)) if m.contains("46 hours ago")), "{outcome:?}");
        // A file dated after "now" is not old.
        let f = arranged("file-future", Some(NOW + 86_400));
        let (outcome, _) = f.add(&["alpha"], Options::default()).await;
        assert!(matches!(&outcome, Err(Error::Policy(m)) if m.contains("in the future")), "{outcome:?}");

        // A dependency's file is held to the same rule, and its refusal leaves the mod that
        // was asked for out as well.
        let f = fixture("file-fresh-dependency");
        let a =
            build(A, "A0000002", "alpha", "2.0", "release", OLD).requires(vec![dependency(Some(B), None, "required")]);
        let b = build(B, "B0000001", "beta-lib", "1.0", "release", OLD);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
        offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&b]);
        f.registry.stored(&b.path, Some(NOW - 3600));
        let before = f.snapshot();
        let (outcome, _) = f.add(&["alpha"], Options::default()).await;
        assert!(matches!(&outcome, Err(Error::Policy(m)) if m.starts_with("Beta Library 1.0:")), "{outcome:?}");
        assert_eq!(f.snapshot(), before);
    }

    #[tokio::test]
    async fn required_dependencies_are_followed_and_optional_ones_only_named() {
        let f = fixture("deps");
        let a = build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![
            dependency(Some(B), None, "required"),
            dependency(Some(C), Some("C0000001"), "required"),
            dependency(Some("EEEEEEEE"), None, "optional"),
            dependency(Some("FFFFFFFF"), None, "embedded"),
            dependency(Some("GGGGGGGG"), None, "incompatible"),
        ]);
        let b = build(B, "B0000001", "beta-lib", "4.0", "release", OLD).requires(vec![dependency(
            Some(D),
            None,
            "required",
        )]);
        // C: the dependent names the exact build, although a newer one exists.
        let c_exact = build(C, "C0000001", "gamma", "1.0", "release", OLD);
        let c_newer = build(C, "C0000002", "gamma", "2.0", "release", RECENT);
        // D asks for A again: a cycle.
        let d =
            build(D, "D0000001", "delta", "0.1", "release", OLD).requires(vec![dependency(Some(A), None, "required")]);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
        offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&b]);
        offer(&f.registry, &project(C, "gamma", "Gamma"), &[&c_newer, &c_exact]);
        offer(&f.registry, &project(D, "delta", "Delta"), &[&d]);
        f.registry.json("GET", "/v2/projects", &json!([project("EEEEEEEE", "epsilon", "Epsilon Extras")]));

        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-4.0.jar", "delta-0.1.jar", "gamma-1.0.jar"]);
        assert!(
            text.contains("add   Beta Library (beta-lib · project BBBBBBBB · licence MIT) - required by Alpha Mod"),
            "{text}"
        );
        assert!(
            text.contains("add   Delta (delta · project DDDDDDDD · licence MIT) - required by Beta Library"),
            "{text}"
        );
        assert!(text.contains("note      the exact build its dependent asks for"), "{text}");
        assert!(text.contains("note: optional, not added: Epsilon Extras (epsilon)"), "{text}");
        assert!(
            !text.contains("FFFFFFFF") && !text.contains("GGGGGGGG"),
            "embedded and absent incompatible projects are not looked up"
        );
        // The cycle ends where it began: A is looked up once, by the name it was asked for.
        let seen = f.registry.seen();
        assert_eq!(f.registry.count("GET api.modrinth.com/v2/project/alpha"), 1, "{seen:?}");
        assert!(!seen.contains(&"GET api.modrinth.com/v2/project/AAAAAAAA".to_string()), "{seen:?}");
        assert_eq!(f.registry.count("/v2/project/AAAAAAAA/version?"), 1, "{seen:?}");
        let manifest = f.manifest();
        assert_eq!(manifest.mods.len(), 4);
        assert!(
            manifest
                .mods
                .iter()
                .all(|m| m.source.as_deref().is_some_and(|s| s.starts_with("https://cdn.modrinth.com/data/")))
        );
    }

    #[tokio::test]
    async fn a_dependency_already_present_in_a_suitable_build_is_kept() {
        let a =
            build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![dependency(Some(B), None, "required")]);
        let b_old = build(B, "B0000001", "beta-lib", "3.0", "release", OLD).requires(vec![dependency(
            Some(D),
            None,
            "required",
        )]);
        let b_new = build(B, "B0000002", "beta-lib", "4.0", "release", RECENT);
        let d = build(D, "D0000001", "delta", "0.1", "release", OLD);
        let publish = |f: &Fixture| {
            offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
            offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&b_new, &b_old]);
            offer(&f.registry, &project(D, "delta", "Delta"), &[&d]);
        };

        // Present, under another name, and listed for this game version: it stays, although
        // a newer build exists — and what *it* requires is still looked after.
        let f = fixture("dep-kept");
        publish(&f);
        f.drop_in("renamed-by-hand.jar", &b_old.content);
        f.known(&[&b_old]);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "delta-0.1.jar", "renamed-by-hand.jar"]);
        assert!(
            text.contains("keep  Beta Library (beta-lib · project BBBBBBBB · licence MIT) - required by Alpha Mod"),
            "{text}"
        );
        assert!(text.contains("present   renamed-by-hand.jar: 3.0 [release]"), "{text}");
        assert!(text.contains("add   Delta") && text.contains("required by Beta Library"), "{text}");
        assert_eq!(f.registry.count("/v2/project/BBBBBBBB/version"), 0, "no need to list builds of what is kept");
        assert!(
            f.registry.seen()[0]
                .starts_with("POST api.modrinth.com/v2/version_files {\"algorithm\":\"sha512\",\"hashes\":[\"")
        );
        // The jar that was there is recorded too, and the output says so; it gets no source.
        assert!(
            text.contains(
                "also recorded, because they were in mods/ without matching the manifest: renamed-by-hand.jar"
            ),
            "{text}"
        );
        let manifest = f.manifest();
        assert_eq!(manifest.mods.iter().find(|m| m.file == "renamed-by-hand.jar").unwrap().source, None);

        // Present, but the registry lists that build for another game version: replaced.
        let f = fixture("dep-replaced");
        publish(&f);
        let b_wrong = build(B, "B0000009", "beta-lib", "2.0-for-1.20", "release", OLD)
            .with(|j| j["game_versions"] = json!(["1.20.1"]));
        f.drop_in("beta-lib-2.0-for-1.20.jar", &b_wrong.content);
        f.known(&[&b_wrong]);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-4.0.jar"]);
        assert!(
            text.contains("replaces  beta-lib-2.0-for-1.20.jar (an older build of this project, by its hash)"),
            "{text}"
        );

        // The dependent names an exact build: what is present must be that one or newer.
        let pinned = |f: &Fixture, present: &Build| {
            publish(f);
            let a = build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![dependency(
                Some(B),
                Some("B0000002"),
                "required",
            )]);
            offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
            f.drop_in(&present.name, &present.content);
            f.known(&[present]);
        };
        let f = fixture("dep-pin-older");
        pinned(&f, &b_old);
        f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-4.0.jar"], "older than the build asked for: replaced");
        let f = fixture("dep-pin-same");
        pinned(&f, &b_new);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-4.0.jar"]);
        assert!(text.contains("keep  Beta Library"), "{text}");
    }

    #[tokio::test]
    async fn a_dependency_that_cannot_be_taken_as_named_follows_the_ordinary_rules() {
        // The exact build asked for is a beta; without --prerelease the newest release is taken.
        let f = fixture("pin-beta");
        let a = build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![
            dependency(Some(B), Some("B0000002"), "required"),
            dependency(None, Some("C0000001"), "required"),
            dependency(None, None, "required").tap(|d| d["file_name"] = json!("not-on-modrinth-1.0.jar")),
            dependency(None, Some("ZZZZZZZZ"), "required"),
        ]);
        let b_release = build(B, "B0000001", "beta-lib", "3.0", "release", OLD);
        let b_beta = build(B, "B0000002", "beta-lib", "4.0-beta", "beta", LATER);
        let c = build(C, "C0000001", "gamma", "1.0", "release", OLD);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
        offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&b_beta, &b_release]);
        offer(&f.registry, &project(C, "gamma", "Gamma"), &[&c]);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-3.0.jar", "gamma-1.0.jar"]);
        assert!(text.contains("its dependent asks for exactly 4.0-beta, which is a pre-release (beta); the newest build the rules allow is looked for instead"), "{text}");
        // A dependency named only by a build is traced to its project.
        assert!(
            text.contains("add   Gamma (gamma · project CCCCCCCC · licence MIT) - required by Alpha Mod"),
            "{text}"
        );
        // What the registry does not host, or no longer has, is the user's to add.
        assert!(
            text.contains(
                "note: Alpha Mod requires not-on-modrinth-1.0.jar, which the registry does not host; add it yourself"
            ),
            "{text}"
        );
        assert!(text.contains("note: Alpha Mod requires a build (ZZZZZZZZ) that the registry no longer has"), "{text}");
    }

    #[tokio::test]
    async fn a_build_that_is_not_older_is_never_replaced_by_an_older_one() {
        // mods/ holds a beta of the library. The mod that is asked for names an exact build
        // of it that is still in quarantine, so that one cannot be taken - and the newest
        // build the rules do allow is the old release. That must not replace the beta.
        let release = build(B, "B0000001", "beta-lib", "3.0", "release", OLD);
        let beta = build(B, "B0000002", "beta-lib", "4.0-beta", "beta", LATER);
        let fresh = build(B, "B0000003", "beta-lib", "4.0", "release", FRESH_12H);
        let a = build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![dependency(
            Some(B),
            Some("B0000003"),
            "required",
        )]);
        let publish = |f: &Fixture| {
            offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
            offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&fresh, &beta, &release]);
        };
        let f = fixture("no-downgrade");
        publish(&f);
        f.drop_in(&beta.name, &beta.content);
        f.known(&[&beta]);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-4.0-beta.jar"], "{text}");
        assert!(text.contains("keep  Beta Library") && !text.contains("replaces"), "{text}");
        assert!(text.contains("its dependent asks for exactly 4.0, which is inside the 48-hour quarantine"), "{text}");
        assert!(
            text.contains(
                "the build in mods/ is not older than the newest one the rules allow (3.0, published 2026-09-01 \
                 00:00 UTC, 74 days ago); it stays"
            ),
            "{text}"
        );
        assert_eq!(f.registry.count("beta-lib-3.0.jar"), 0, "the older build was not even fetched");
        // What is said of a build that was not taken is not said of the one that stays.
        assert!(!text.contains("taken because"), "{text}");

        // With the quarantine lifted the build that is asked for is newer than the beta: an update.
        let f = fixture("no-downgrade-allowed");
        publish(&f);
        f.drop_in(&beta.name, &beta.content);
        f.known(&[&beta]);
        let (outcome, text) = f.add(&["alpha"], Options { allow_fresh: true, ..Default::default() }).await;
        assert!(outcome.unwrap(), "{text}");
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-4.0.jar"]);
        assert!(
            text.contains("replaces  beta-lib-4.0-beta.jar (an older build of this project, by its hash)"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_dependency_built_for_another_minecraft_version_is_replaced_and_the_output_says_why() {
        // The library in mods/ is the newest there is, but the registry lists it for another
        // Minecraft version: it cannot serve this profile, and two builds of one mod cannot
        // both be loaded.
        let a =
            build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![dependency(Some(B), None, "required")]);
        let here = build(B, "B0000001", "beta-lib", "4.0", "release", OLD);
        let elsewhere = build(B, "B0000002", "beta-lib", "5.0-for-1.22", "release", RECENT)
            .with(|j| j["game_versions"] = json!(["1.22"]));
        let publish = |f: &Fixture| {
            offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
            offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&elsewhere, &here]);
            f.drop_in(&elsewhere.name, &elsewhere.content);
            f.known(&[&elsewhere]);
        };
        let f = fixture("other-minecraft");
        publish(&f);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-4.0.jar"], "{text}");
        assert!(
            text.contains(
                "replaces  beta-lib-5.0-for-1.22.jar (a build of this project, by its hash, that the registry does \
                 not list for Minecraft 1.21.4 on Fabric)"
            ),
            "{text}"
        );
        assert!(!text.contains("an older build"), "it is not older, and is not called so:\n{text}");

        // Asked for by name, the same jar stays - nothing that is not older is removed on
        // request - and the output says what to do about it.
        let f = fixture("other-minecraft-asked");
        publish(&f);
        let before = f.snapshot();
        let (outcome, text) = f.add(&["beta-lib"], Options::default()).await;
        assert!(!outcome.unwrap(), "{text}");
        assert!(text.contains("it stays"), "{text}");
        assert!(
            text.contains(
                "the registry does not list the build in mods/ for Minecraft 1.21.4 on Fabric: if it is the wrong \
                 one, remove mods/beta-lib-5.0-for-1.22.jar and run this again"
            ),
            "{text}"
        );
        assert_eq!(f.snapshot(), before);
    }

    trait Tap {
        fn tap(self, change: impl FnOnce(&mut Value)) -> Self;
    }

    impl Tap for Value {
        fn tap(mut self, change: impl FnOnce(&mut Value)) -> Self {
            change(&mut self);
            self
        }
    }

    #[tokio::test]
    async fn dependency_chains_and_fans_are_bounded() {
        // A chain one level deeper than allowed.
        let f = fixture("deep");
        let ids: Vec<String> = (0..=MAX_DEPTH + 1).map(|n| format!("CHAIN00{n}")).collect();
        for (n, id) in ids.iter().enumerate() {
            let next = ids.get(n + 1).map(|next| vec![dependency(Some(next), None, "required")]).unwrap_or_default();
            let b = build(id, &format!("V000000{n}"), &format!("link{n}"), "1.0", "release", OLD).requires(next);
            offer(&f.registry, &project(id, &format!("link{n}"), &format!("Link {n}")), &[&b]);
        }
        let before = f.snapshot();
        let (outcome, _) = f.add(&["link0"], Options::default()).await;
        assert!(matches!(&outcome, Err(Error::Policy(m)) if m.contains("more than 4 levels deep")), "{outcome:?}");
        assert_eq!(f.snapshot(), before);
        assert_eq!(f.registry.count("cdn.modrinth.com"), 0);
        // One level less is fine.
        let (outcome, text) = f.add(&["link1"], Options::default()).await;
        assert!(outcome.unwrap(), "{text}");
        assert_eq!(f.mods().len(), MAX_DEPTH + 1);

        // One mod that requires more projects than a command will look at.
        let f = fixture("wide");
        let ids: Vec<String> = (0..MAX_PROJECTS + 1).map(|n| format!("WIDE{n:04}")).collect();
        let hub = build(A, "A0000001", "alpha", "1.0", "release", OLD)
            .requires(ids.iter().map(|id| dependency(Some(id), None, "required")).collect());
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&hub]);
        for (n, id) in ids.iter().enumerate() {
            let b = build(id, &format!("W000{n:04}"), &format!("leaf{n}"), "1.0", "release", OLD);
            offer(&f.registry, &project(id, &format!("leaf{n}"), &format!("Leaf {n}")), &[&b]);
        }
        let before = f.snapshot();
        let (outcome, _) = f.add(&["alpha"], Options::default()).await;
        assert!(matches!(&outcome, Err(Error::Policy(m)) if m.contains("more than 24 projects")), "{outcome:?}");
        assert_eq!(f.snapshot(), before);
        assert_eq!(f.registry.count("cdn.modrinth.com"), 0);
        // And a command line cannot ask for more either.
        let many: Vec<String> = (0..MAX_REQUESTED + 1).map(|n| format!("mod{n}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(matches!(f.add(&many, Options::default()).await.0, Err(Error::Usage(_))));
        assert!(matches!(f.add(&[], Options::default()).await.0, Err(Error::Usage(_))));
    }

    #[tokio::test]
    async fn an_older_build_of_the_same_project_is_replaced_and_no_other_jar_is_touched() {
        let f = fixture("replace");
        let v1 =
            build_of(A, "A0000001", "alpha", "1.0", "release", OLD, mod_jar("alpha", "1.0", &["java/util/ArrayList"]));
        let v2 = build_of(
            A,
            "A0000002",
            "alpha",
            "2.0",
            "release",
            RECENT,
            mod_jar("alpha", "2.0", &["java/net/Socket", "java/lang/ProcessBuilder"]),
        );
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&v2, &v1]);
        // First v1 through the command, then a jar of the user's own that no registry knows.
        f.registry.json("GET", "/v2/project/AAAAAAAA/version", &json!([v1.json]));
        f.added(&["alpha"]).await;
        f.drop_in("my-own-mod.jar", &mod_jar("mine", "0", &[]));
        author::lock(&f.paths, "perf").unwrap();
        fs::write(f.dir.join(SIGNATURE_FILE), "signed at serial 3\n").unwrap();
        assert_eq!(
            (f.manifest().serial, f.mods()),
            (3, vec!["alpha-1.0.jar".to_string(), "my-own-mod.jar".to_string()])
        );
        let mine = f.manifest().mods.iter().find(|m| m.file == "my-own-mod.jar").unwrap().clone();

        // Now the registry has v2, and recognises the v1 in mods/ by its hash.
        f.registry.json("GET", "/v2/project/AAAAAAAA/version", &json!([v1.json, v2.json]));
        f.known(&[&v1]);
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-2.0.jar", "my-own-mod.jar"]);
        assert!(text.contains("replaces  alpha-1.0.jar (an older build of this project, by its hash)"), "{text}");
        let manifest = f.manifest();
        assert_eq!(manifest.serial, 4);
        assert!(!f.dir.join(SIGNATURE_FILE).exists());
        assert_eq!(
            manifest.mods.iter().find(|m| m.file == "my-own-mod.jar"),
            Some(&mine),
            "the user's own jar is as it was"
        );
        assert!(
            manifest
                .mods
                .iter()
                .find(|m| m.file == "alpha-2.0.jar")
                .unwrap()
                .source
                .as_deref()
                .unwrap()
                .contains("/A0000002/")
        );
        // The scan shows what the new build can reach, and what it gained over the old one.
        assert!(text.contains("network - opens its own network connections: alpha.Main"), "{text}");
        assert!(text.contains("runs programs - starts other processes: alpha.Main"), "{text}");
        assert!(text.contains("alpha: NEW since the last lock (alpha-1.0.jar → alpha-2.0.jar): network"), "{text}");
        assert!(!text.contains("mine"), "jars that were not added are not paraded:\n{text}");

        // The other way round: what is in mods/ is newer than anything the rules allow.
        let f = fixture("replace-newer");
        let beta = build(A, "A0000003", "alpha", "3.0-beta", "beta", RECENT);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&beta, &v1]);
        f.drop_in("alpha-3.0-beta.jar", &beta.content);
        f.known(&[&beta]);
        let before = f.snapshot();
        let (outcome, text) = f.add(&["alpha"], Options::default()).await;
        assert!(!outcome.unwrap());
        assert!(
            text.contains("the build in mods/ is not older than the newest one the rules allow (1.0, published"),
            "{text}"
        );
        assert_eq!(f.snapshot(), before, "no downgrade, no change");
    }

    #[tokio::test]
    async fn every_refusal_leaves_mods_the_manifest_and_the_signature_as_they_were() {
        fn good() -> Build {
            build(A, "A0000001", "alpha", "1.0", "release", OLD)
        }
        fn lib() -> Build {
            build(B, "B0000001", "beta-lib", "1.0", "release", OLD)
        }
        type Scenario = (&'static str, Box<dyn Fn(&Fixture)>, fn(&Error) -> bool);
        let policy: fn(&Error) -> bool = |e| matches!(e, Error::Policy(_));
        let integrity: fn(&Error) -> bool = |e| matches!(e, Error::Integrity(_));
        let format: fn(&Error) -> bool = |e| matches!(e, Error::Format(_));
        let net: fn(&Error) -> bool = |e| matches!(e, Error::Net(_));
        let plain = |change: fn(&mut Value)| -> Box<dyn Fn(&Fixture)> {
            Box::new(move |f: &Fixture| {
                let b = good().with(change);
                offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
            })
        };
        let scenarios: Vec<Scenario> = vec![
            (
                "the file server sends other bytes of the same length",
                Box::new(|f| {
                    let b = good();
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
                    let mut other = b.content.clone();
                    *other.last_mut().unwrap() ^= 0x20;
                    f.registry.on("GET", &b.path, 200, other);
                }),
                integrity,
            ),
            (
                "the file server sends more than the listed size",
                Box::new(|f| {
                    let b = good();
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
                    f.registry.on("GET", &b.path, 200, [b.content.clone(), vec![0u8; 4096]].concat());
                }),
                integrity,
            ),
            (
                "the file server sends less than the listed size",
                Box::new(|f| {
                    let b = good();
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
                    f.registry.on("GET", &b.path, 200, b.content[..b.content.len() / 2].to_vec());
                }),
                integrity,
            ),
            (
                "the file server has no such file",
                Box::new(|f| {
                    let b = good();
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
                    f.registry.on("GET", &b.path, 404, "");
                }),
                net,
            ),
            (
                "the file server redirects",
                Box::new(|f| {
                    let b = good();
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
                    f.registry.on("GET", &b.path, 302, "");
                }),
                net,
            ),
            (
                "the file server has held the file for a few hours only",
                Box::new(|f| {
                    let b = good();
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
                    f.registry.stored(&b.path, Some(NOW - 6 * 3600));
                }),
                policy,
            ),
            (
                "the second of two files is wrong",
                Box::new(|f| {
                    let a = good().requires(vec![dependency(Some(B), None, "required")]);
                    let b = lib();
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
                    offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&b]);
                    f.registry.on("GET", &b.path, 200, vec![7u8; b.content.len()]);
                }),
                integrity,
            ),
            (
                "a file name with a path in it",
                plain(|j| j["files"][0]["filename"] = json!("../../.bashrc.jar")),
                policy,
            ),
            ("a file name with a space", plain(|j| j["files"][0]["filename"] = json!("alpha mod.jar")), policy),
            ("a file that is not a jar", plain(|j| j["files"][0]["filename"] = json!("alpha-1.0.zip")), policy),
            (
                "an address on another host",
                plain(|j| {
                    j["files"][0]["url"] = json!("https://evil.example/data/AAAAAAAA/versions/A0000001/alpha-1.0.jar")
                }),
                policy,
            ),
            (
                "a plain-http address",
                plain(|j| {
                    j["files"][0]["url"] =
                        json!("http://cdn.modrinth.com/data/AAAAAAAA/versions/A0000001/alpha-1.0.jar")
                }),
                policy,
            ),
            (
                "an address under another project",
                plain(|j| {
                    j["files"][0]["url"] =
                        json!("https://cdn.modrinth.com/data/BBBBBBBB/versions/A0000001/alpha-1.0.jar")
                }),
                policy,
            ),
            (
                "a file larger than is accepted",
                plain(|j| j["files"][0]["size"] = json!(modrinth::MAX_MOD_BYTES + 1)),
                policy,
            ),
            ("an empty file", plain(|j| j["files"][0]["size"] = json!(0)), policy),
            ("a hash that is not one", plain(|j| j["files"][0]["hashes"]["sha512"] = json!("deadbeef")), policy),
            ("no file at all", plain(|j| j["files"] = json!([])), policy),
            (
                "a build list that is not JSON",
                Box::new(|f| {
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&good()]);
                    f.registry.on("GET", "/v2/project/AAAAAAAA/version", 200, "<html>maintenance</html>");
                }),
                format,
            ),
            ("a build list with a date that is none", plain(|j| j["date_published"] = json!("soon")), format),
            ("a build list that holds another project's build", plain(|j| j["project_id"] = json!("BBBBBBBB")), format),
            (
                "the registry answers with another project than asked for",
                Box::new(|f| {
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&good()]);
                    f.registry.json("GET", "/v2/project/alpha", &project(B, "something-else", "Something Else"));
                }),
                format,
            ),
            (
                "a project description that is not JSON",
                Box::new(|f| {
                    f.registry.on("GET", "/v2/project/alpha", 200, "{\"id\":");
                }),
                format,
            ),
            (
                "the project is a modpack",
                Box::new(|f| {
                    let mut p = project(A, "alpha", "Alpha Pack");
                    p["project_type"] = json!("modpack");
                    offer(&f.registry, &p, &[&good()]);
                }),
                policy,
            ),
            (
                "the project is withheld by the registry",
                Box::new(|f| {
                    let mut p = project(A, "alpha", "Alpha Mod");
                    p["status"] = json!("withheld");
                    offer(&f.registry, &p, &[&good()]);
                }),
                policy,
            ),
            (
                "the registry's server fails",
                Box::new(|f| {
                    f.registry.on("GET", "/v2/project/alpha", 503, "try later");
                }),
                net,
            ),
            (
                "the hash lookup fails",
                Box::new(|f| {
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&good()]);
                    f.registry.on("POST", "/v2/version_files", 500, "");
                }),
                net,
            ),
            (
                "the hash lookup answers for a hash nobody asked about",
                Box::new(|f| {
                    let b = good();
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
                    f.known(&[&b]);
                }),
                format,
            ),
            (
                "what was downloaded is not a jar",
                Box::new(|f| {
                    let b = build_of(
                        A,
                        "A0000001",
                        "alpha",
                        "1.0",
                        "release",
                        OLD,
                        b"MZ this is not a zip archive".to_vec(),
                    );
                    offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&b]);
                }),
                format,
            ),
        ];
        for (what, arrange, expected) in scenarios {
            let f = fixture("refusal");
            // Something to lose: a jar that is in the manifest, under a signature.
            f.drop_in("resident.jar", &mod_jar("resident", "1", &[]));
            author::lock(&f.paths, "perf").unwrap();
            fs::write(f.dir.join(SIGNATURE_FILE), "a signature\n").unwrap();
            arrange(&f);
            let before = f.snapshot();
            let (outcome, text) = f.add(&["alpha"], Options::default()).await;
            let error = outcome.err().unwrap_or_else(|| panic!("{what}: accepted\n{text}"));
            assert!(expected(&error), "{what}: {error:?}");
            assert_eq!(f.snapshot(), before, "{what}: something changed");
            let message = error.to_string();
            assert!(message.len() < 900 && !message.contains('\u{1b}'), "{what}: {message}");
        }
    }

    #[tokio::test]
    async fn a_dry_run_shows_the_plan_and_fetches_nothing() {
        let f = fixture("dry");
        let a =
            build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![dependency(Some(B), None, "required")]);
        let b = build(B, "B0000001", "beta-lib", "1.0", "release", OLD);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
        offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&b]);
        let before = f.snapshot();
        let (outcome, text) = f.add(&["alpha"], Options { dry_run: true, ..Default::default() }).await;
        assert!(!outcome.unwrap());
        assert!(text.contains("add   Alpha Mod") && text.contains("add   Beta Library"), "{text}");
        assert!(text.contains(&format!("sha512    {}", sha512_hex(&b.content))), "{text}");
        assert!(
            text.contains("--dry-run: nothing was downloaded; mods/, the manifest and its signature were not touched.")
        );
        assert!(text.contains("The file server's date for each file is read when the file is downloaded"), "{text}");
        assert_eq!(f.snapshot(), before);
        assert_eq!(f.registry.count("cdn.modrinth.com"), 0);
    }

    #[tokio::test]
    async fn a_jar_the_registry_does_not_know_is_never_removed_or_written_over() {
        let a =
            build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![dependency(Some(B), None, "required")]);
        let b = build(B, "B0000001", "beta-lib", "1.0", "release", OLD);
        let publish = |f: &Fixture| {
            offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
            offer(&f.registry, &project(B, "beta-lib", "Beta Library"), &[&b]);
        };
        // A stranger under the very name the new file would get (in other case, too).
        for name in ["alpha-1.0.jar", "ALPHA-1.0.jar"] {
            let f = fixture("clash-name");
            publish(&f);
            f.drop_in(name, &mod_jar("unrelated", "1", &[]));
            let before = f.snapshot();
            let (outcome, _) = f.add(&["alpha"], Options::default()).await;
            assert!(
                matches!(&outcome, Err(Error::Policy(m)) if m.contains("already holds a file of that name")),
                "{outcome:?}"
            );
            assert_eq!(f.snapshot(), before);
            assert_eq!(f.registry.count("cdn.modrinth.com"), 0, "found out before anything was fetched");
        }
        // A stranger that declares the mod id of what was asked for: the user's call.
        let f = fixture("clash-id");
        publish(&f);
        f.drop_in("alpha-fork.jar", &mod_jar("alpha", "fork", &[]));
        let before = f.snapshot();
        let (outcome, _) = f.add(&["alpha"], Options::default()).await;
        assert!(
            matches!(&outcome, Err(Error::Policy(m)) if m.contains("mods/alpha-fork.jar already provides the mod id `alpha`")),
            "{outcome:?}"
        );
        assert_eq!(f.snapshot(), before);
        // A stranger that declares the mod id of a *dependency*: taken to be it, and said so.
        let f = fixture("clash-dep");
        publish(&f);
        f.drop_in("beta-lib-homebuilt.jar", &mod_jar("beta-lib", "home", &[]));
        let text = f.added(&["alpha"]).await;
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-homebuilt.jar"]);
        assert!(text.contains("not adding beta-lib-1.0.jar: mods/beta-lib-homebuilt.jar already provides the mod id `beta-lib`. Whether it satisfies Alpha Mod is for the loader to say."), "{text}");
        // Two projects in one command that both claim one mod id.
        let f = fixture("clash-two");
        publish(&f);
        let twin = build_of(C, "C0000001", "gamma", "1.0", "release", OLD, mod_jar("alpha", "twin", &[]));
        offer(&f.registry, &project(C, "gamma", "Gamma"), &[&twin]);
        let before = f.snapshot();
        let (outcome, _) = f.add(&["alpha", "gamma"], Options::default()).await;
        assert!(
            matches!(&outcome, Err(Error::Policy(m)) if m.contains("both provide the mod id `alpha`")),
            "{outcome:?}"
        );
        assert_eq!(f.snapshot(), before);
    }

    #[tokio::test]
    async fn a_jar_is_not_removed_on_the_registrys_word_alone() {
        // The registry claims that a jar of the user's own is an old build of Alpha Mod.
        let f = fixture("lying");
        let mine = mod_jar("mine", "private", &[]);
        let claimed = build_of(A, "A0000000", "alpha", "0.9", "release", OLD, mine.clone());
        let newer = build(A, "A0000001", "alpha", "1.0", "release", LATER);
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&newer, &claimed]);
        f.drop_in("my-private-mod.jar", &mine);
        f.known(&[&claimed]);
        let before = f.snapshot();
        let (outcome, text) = f.add(&["alpha"], Options::default()).await;
        // The plan proposes the replacement; looking into the two jars stops it.
        assert!(text.contains("replaces  my-private-mod.jar"), "{text}");
        let message = match outcome {
            Err(Error::Policy(message)) => message,
            other => panic!("{other:?}"),
        };
        assert!(
            message.contains("the registry calls mods/my-private-mod.jar a build of Alpha Mod")
                && message.contains("(`mine` and `alpha`)"),
            "{message}"
        );
        assert_eq!(f.snapshot(), before);

        // The same for a jar that declares no mod id at all, or is not a jar.
        for content in [jar(&[("readme.txt", b"hello".to_vec())]), b"not a zip".to_vec()] {
            let f = fixture("lying-blank");
            let claimed = build_of(A, "A0000000", "alpha", "0.9", "release", OLD, content.clone());
            offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&newer, &claimed]);
            f.drop_in("something.jar", &content);
            f.known(&[&claimed]);
            let before = f.snapshot();
            let (outcome, _) = f.add(&["alpha"], Options::default()).await;
            assert!(matches!(&outcome, Err(Error::Policy(m)) if m.contains("(none and `alpha`)")), "{outcome:?}");
            assert_eq!(f.snapshot(), before);
        }
    }

    #[tokio::test]
    async fn an_alias_names_its_project_by_id() {
        let mut names: Vec<&str> = ALIASES.iter().map(|a| a.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALIASES.len(), "an alias is listed twice");
        for a in ALIASES {
            modrinth::check_id(a.id).unwrap();
            modrinth::check_key(a.name).unwrap();
            assert_eq!(a.name, a.name.to_ascii_lowercase());
        }
        for wanted in ["viafabric", "simple-voice-chat", "voicechat", "fabric-api", "sodium", "lithium", "modmenu"] {
            assert!(alias(wanted).is_some(), "{wanted}");
        }
        assert_eq!(alias("voicechat").unwrap().id, alias("simple-voice-chat").unwrap().id);
        assert_eq!(alias("ViaFabric").unwrap().id, "YlKdE5VK");
        assert!(alias("not-an-alias").is_none());

        // The slug of the day is not asked; the id is. Somebody else now owns the old slug.
        let f = fixture("alias");
        let real = build("9eGKb6K1", "V0000001", "voicechat", "2.6", "release", OLD);
        offer(&f.registry, &project("9eGKb6K1", "voice-chat-renamed", "Simple Voice Chat"), &[&real]);
        let squatter = build("SQUATTER", "V0000002", "evil", "9.9", "release", OLD);
        offer(&f.registry, &project("SQUATTER", "simple-voice-chat", "Simple Voice Chat"), &[&squatter]);
        let text = f.added(&["voicechat"]).await;
        assert_eq!(f.mods(), ["voicechat-2.6.jar"]);
        let seen = f.registry.seen();
        assert!(seen.contains(&"GET api.modrinth.com/v2/project/9eGKb6K1".to_string()), "{seen:?}");
        assert_eq!(f.registry.count("/v2/project/simple-voice-chat"), 0);
        assert!(
            text.contains("the registry now calls this project \"voice-chat-renamed\"; it was \"simple-voice-chat\""),
            "{text}"
        );
        // An alias that is answered with another project is an error, whatever its slug.
        let f = fixture("alias-swapped");
        offer(&f.registry, &project("SQUATTER", "simple-voice-chat", "Simple Voice Chat"), &[&squatter]);
        f.registry.json("GET", "/v2/project/9eGKb6K1", &project("SQUATTER", "simple-voice-chat", "Simple Voice Chat"));
        assert!(matches!(f.add(&["simple-voice-chat"], Options::default()).await.0, Err(Error::Format(_))));
        // A name that is neither alias nor project: the aliases are offered.
        let (outcome, _) = f.add(&["no-such-mod"], Options::default()).await;
        assert!(
            matches!(&outcome, Err(Error::Usage(m)) if m.contains("viafabric, simple-voice-chat, voicechat")),
            "{outcome:?}"
        );
        assert!(matches!(f.add(&["../../etc/passwd"], Options::default()).await.0, Err(Error::Usage(_))));
        assert_eq!(f.registry.count("passwd"), 0, "a name like that is never sent");
        // Nor is anything else sent on its account: not the hashes of what is in mods/,
        // and not the lookup of a good name that stands before it on the command line.
        let f = fixture("bad-name");
        f.drop_in("resident.jar", &mod_jar("resident", "1", &[]));
        assert!(matches!(f.add(&["sodium", "a b"], Options::default()).await.0, Err(Error::Usage(_))));
        assert!(f.registry.seen().is_empty(), "{:?}", f.registry.seen());
    }

    #[tokio::test]
    async fn only_a_draft_profile_with_a_loader_takes_mods() {
        let f = fixture("kinds");
        instance::create(&f.paths, "vanilla", Some("1.21.4")).unwrap();
        let mut out = Output::silent();
        let wanted = ["alpha".to_string()];
        for (id, expect) in [("vanilla", "no profile manifest"), ("missing", "no instance called")] {
            let outcome = add(&f.paths, &f.registry, id, &wanted, Options::default(), NOW, &mut out).await;
            assert!(matches!(&outcome, Err(Error::Usage(m)) if m.contains(expect)), "{id}: {outcome:?}");
        }
        assert!(add(&f.paths, &f.registry, "../perf", &wanted, Options::default(), NOW, &mut out).await.is_err());
        assert!(f.registry.seen().is_empty(), "nothing is asked of the registry for a profile that cannot take mods");
        // Something in mods/ that is not a plain jar stops the command before the network.
        fs::write(f.dir.join("mods/notes.txt"), "x").unwrap();
        assert!(matches!(f.add(&["alpha"], Options::default()).await.0, Err(Error::Policy(_))));
        assert!(f.registry.seen().is_empty());
    }

    #[tokio::test]
    async fn quilt_profiles_take_builds_for_quilt_or_fabric() {
        let project = modrinth::parse_project(&serde_json::to_vec(&project(A, "alpha", "Alpha Mod")).unwrap()).unwrap();
        let listed = |builds: &[&Build]| {
            let list: Vec<Value> = builds.iter().map(|b| b.json.clone()).collect();
            modrinth::parse_versions(&serde_json::to_vec(&list).unwrap()).unwrap()
        };
        let fabric = build(A, "A0000001", "alpha", "1.0-fabric", "release", RECENT);
        let quilt =
            build(A, "A0000002", "alpha", "1.0-quilt", "release", OLD).with(|j| j["loaders"] = json!(["quilt"]));
        let forge =
            build(A, "A0000003", "alpha", "1.0-forge", "release", RECENT).with(|j| j["loaders"] = json!(["forge"]));
        let silent = AtomicU64::new(0);
        let rules = |loader| Rules {
            minecraft: "1.21.4",
            loader,
            options: Options::default(),
            local: NOW,
            registry_clock: &silent,
        };
        let all = listed(&[&forge, &quilt, &fabric]);
        assert_eq!(choose(&project, &all, &rules(LoaderKind::Fabric)).unwrap().version.number, "1.0-fabric");
        assert_eq!(
            choose(&project, &all, &rules(LoaderKind::Quilt)).unwrap().version.number,
            "1.0-fabric",
            "the newest of the two kinds"
        );
        assert_eq!(
            choose(&project, &listed(&[&forge, &quilt]), &rules(LoaderKind::Quilt)).unwrap().version.number,
            "1.0-quilt"
        );
        let none = choose(&project, &listed(&[&forge, &quilt]), &rules(LoaderKind::Fabric)).err().unwrap();
        assert!(none.to_string().contains("no build for Minecraft 1.21.4 on Fabric"), "{none}");
        assert_eq!(rules(LoaderKind::Quilt).target(), "Minecraft 1.21.4 on Quilt");
    }

    #[tokio::test]
    async fn check_says_what_the_registry_knows_about_each_jar_and_what_that_is_worth() {
        let f = fixture("check");
        let old = build(A, "A0000001", "alpha", "1.0", "release", OLD);
        let fresh = build(B, "B0000001", "beta-lib", "2.0-rc", "beta", FRESH_12H);
        let wrong = build(C, "C0000001", "gamma", "0.5", "release", OLD).with(|j| {
            j["game_versions"] = json!(["1.20.1", "1.20.4"]);
            j["loaders"] = json!(["quilt"]);
        });
        let gone = build(D, "D0000001", "delta", "1.0", "release", OLD);
        // alpha came through `profile add`; the rest was copied in, one under another name.
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&old]);
        f.added(&["alpha"]).await;
        f.drop_in("beta-lib-2.0-rc.jar", &fresh.content);
        f.drop_in("renamed.jar", &wrong.content);
        f.drop_in("private-build.jar", &mod_jar("mine", "1", &[]));
        f.drop_in("delta-1.0.jar", &gone.content);
        // delta is recorded as coming from the registry, which no longer lists that file.
        let sources = BTreeMap::from([("delta-1.0.jar".to_string(), format!("https://cdn.modrinth.com{}", gone.path))]);
        author::lock_with(&f.paths, "perf", &sources).unwrap();
        // … and after the lock one jar is swapped for another build.
        fs::write(f.dir.join(SIGNATURE_FILE), "signed\n").unwrap();
        f.known(&[&old, &fresh, &wrong]);
        let mut withheld = project(B, "beta-lib", "Beta Library");
        withheld["status"] = json!("withheld");
        f.registry.json("GET", "/v2/projects", &json!([project(A, "alpha", "Alpha Mod"), withheld]));

        let before = f.snapshot();
        let requests = f.registry.seen().len();
        let mut out = Output::silent();
        let summary = check(&f.paths, &f.registry, "perf", NOW, &mut out).await.unwrap();
        let text = out.lines.join("\n");
        assert_eq!(summary, CheckSummary { jars: 5, known: 3, unknown: 2, fresh: 1 }, "{text}");
        assert_eq!(f.snapshot(), before, "a check changes nothing");
        // Two requests: the hashes, and the names of the projects they belong to.
        let asked = &f.registry.seen()[requests..];
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(
            asked[0].starts_with("POST api.modrinth.com/v2/version_files")
                && asked[0].contains(&sha512_hex(&old.content))
        );
        assert!(asked[1].starts_with("GET api.modrinth.com/v2/projects?ids="), "{asked:?}");

        let about = |name: &str| -> String {
            let mut lines = text.lines().skip_while(|line| *line != format!("  {name}")).skip(1);
            let mut own = Vec::new();
            for line in &mut lines {
                if !line.starts_with("      ") {
                    break;
                }
                own.push(line);
            }
            own.join("\n")
        };
        let alpha = about("alpha-1.0.jar");
        assert!(alpha.contains("known: Alpha Mod (alpha · project AAAAAAAA · licence MIT), version 1.0 [release], published 2026-09-01 00:00 UTC, 74 days ago"), "{alpha}");
        assert!(!alpha.contains("flag:"), "{alpha}");
        let beta = about("beta-lib-2.0-rc.jar");
        assert!(beta.contains("flag: FRESH: published less than 48 hours ago"), "{beta}");
        assert!(beta.contains("flag: a pre-release build (beta)"), "{beta}");
        assert!(beta.contains("flag: the registry lists the project with the status \"withheld\""), "{beta}");
        let renamed = about("renamed.jar");
        assert!(renamed.contains("known: project CCCCCCCC, version 0.5 [release]"), "{renamed}");
        assert!(renamed.contains("flag: its project is not public at the registry (any more)"), "{renamed}");
        assert!(renamed.contains("flag: the registry lists this build for Minecraft 1.20.1, 1.20.4 on quilt, not for this profile's 1.21.4 on fabric"), "{renamed}");
        assert!(renamed.contains("flag: the registry's name for this file is gamma-0.5.jar"), "{renamed}");
        let private = about("private-build.jar");
        assert!(
            private.contains(&format!("UNKNOWN to the registry (sha512 {})", sha512_hex(&mod_jar("mine", "1", &[])))),
            "{private}"
        );
        assert!(!private.contains("flag:"), "{private}");
        let delta = about("delta-1.0.jar");
        assert!(delta.contains("UNKNOWN to the registry"), "{delta}");
        assert!(delta.contains("flag: the manifest records https://cdn.modrinth.com/data/DDDDDDDD/versions/D0000001/delta-1.0.jar as its source, yet the registry lists no file with this hash now"), "{delta}");
        assert!(text.contains("3 known, 2 unknown, 1 published less than 48 hours ago."), "{text}");
        // The caveat is part of every report.
        for needle in [
            "What it does not mean: that the file is harmless",
            "took the account over",
            "CurseForge (its API needs a key) and GitHub",
            "tampered with or withdrawn",
            "\"Fresh\" goes by the date of the build",
        ] {
            assert!(text.contains(needle), "{needle:?} missing in\n{text}");
        }

        // A jar that changed after the lock is pointed out; so is a source that does not fit.
        fs::write(f.dir.join("mods/private-build.jar"), mod_jar("mine", "2", &[])).unwrap();
        let mut manifest = f.manifest();
        manifest.mods.iter_mut().find(|m| m.file == "alpha-1.0.jar").unwrap().source =
            Some("https://cdn.modrinth.com/data/AAAAAAAA/versions/A0000009/alpha-1.0.jar".into());
        fs::write(f.dir.join(MANIFEST_FILE), manifest.to_bytes().unwrap()).unwrap();
        let mut out = Output::silent();
        check(&f.paths, &f.registry, "perf", NOW, &mut out).await.unwrap();
        let text = out.lines.join("\n");
        assert!(text.contains("flag: not what the manifest lists (`profile lock` records what is in mods/)"), "{text}");
        assert!(text.contains("flag: the manifest records another source for it: https://cdn.modrinth.com/data/AAAAAAAA/versions/A0000009/alpha-1.0.jar"), "{text}");

        // Nothing to look up: nothing is asked.
        let empty = fixture("check-empty");
        let mut out = Output::silent();
        assert_eq!(check(&empty.paths, &empty.registry, "perf", NOW, &mut out).await.unwrap(), CheckSummary::default());
        assert!(empty.registry.seen().is_empty() && out.lines == ["perf: no mod jars"]);
        // Hostile answers are refused here as everywhere.
        let hostile = fixture("check-hostile");
        hostile.drop_in("x.jar", &old.content);
        hostile.known(&[&fresh]);
        let mut out = Output::silent();
        assert!(matches!(check(&hostile.paths, &hostile.registry, "perf", NOW, &mut out).await, Err(Error::Format(_))));
    }

    #[tokio::test]
    async fn incompatible_and_archived_projects_are_pointed_out() {
        let f = fixture("remarks");
        let a = build(A, "A0000001", "alpha", "1.0", "release", OLD).requires(vec![dependency(
            Some(B),
            None,
            "incompatible",
        )]);
        let b = build(B, "B0000001", "beta-lib", "1.0", "release", OLD);
        let mut archived = project(B, "beta-lib", "Beta Library");
        archived["status"] = json!("archived");
        offer(&f.registry, &project(A, "alpha", "Alpha Mod"), &[&a]);
        offer(&f.registry, &archived, &[&b]);
        let text = f.added(&["alpha", "beta-lib"]).await;
        assert!(
            text.contains(
                "note: WARNING: Alpha Mod declares itself incompatible with Beta Library, which is in this profile"
            ),
            "{text}"
        );
        assert!(text.contains("note      the registry lists this project as archived"), "{text}");
        assert_eq!(f.mods(), ["alpha-1.0.jar", "beta-lib-1.0.jar"], "a warning, not a refusal: the loader decides");
    }

    #[test]
    fn a_mods_directory_the_staged_files_cannot_be_linked_into_is_refused_before_anything_is_removed() {
        let f = fixture("devices");
        let staging = Staging::create(&f.dir).unwrap();
        assert!(same_file_system(&staging.0, &f.dir.join("mods")).unwrap());
        assert!(!same_file_system(&staging.0, Path::new("/proc")).unwrap(), "procfs is never where a profile lives");
        assert!(same_file_system(&staging.0, &f.dir.join("no-such-directory")).is_err());

        // The real thing needs a second file system to stage on: the build directory, or
        // the shared-memory tmpfs. Where both are on the profile's own, there is nothing to try.
        let candidates = [PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"), PathBuf::from("/dev/shm")];
        let other = candidates.into_iter().find(|dir| same_file_system(dir, &f.dir).is_ok_and(|same| !same));
        let Some(other) = other else {
            eprintln!("skipped: no second file system to stage a file on");
            return;
        };
        let elsewhere =
            other.join(format!("bastion-elsewhere-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        fs::create_dir(&elsewhere).unwrap();
        let path = elsewhere.join("alpha-1.0.jar");
        fs::write(&path, mod_jar("alpha", "1.0", &[])).unwrap();
        let file = VersionFile {
            filename: "alpha-1.0.jar".into(),
            url: String::new(),
            size: 1,
            sha512: String::new(),
            sha1: None,
        };
        let before = f.snapshot();
        let outcome = install(&f.dir, &Plan::default(), &[Staged { step: 0, path, file, sha256: String::new() }]);
        fs::remove_dir_all(&elsewhere).unwrap();
        assert!(matches!(&outcome, Err(Error::Policy(m)) if m.contains("another file system")), "{outcome:?}");
        assert_eq!(f.snapshot(), before, "the signature is still there and mods/ is as it was");
    }

    #[test]
    fn stale_staging_directories_are_swept_and_fresh_ones_left_alone() {
        let f = fixture("sweep");
        let stale = f.dir.join(format!("{STAGING_PREFIX}0123456789abcdef"));
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("half.jar"), b"partial").unwrap();
        let two_hours_ago = SystemTime::now() - Duration::from_secs(7200);
        File::open(&stale).unwrap().set_modified(two_hours_ago).unwrap();
        let running = Staging::create(&f.dir).unwrap();
        let unrelated = f.dir.join("saves");
        File::open(&unrelated).unwrap().set_modified(two_hours_ago).unwrap();
        sweep_stale(&f.dir);
        assert!(!stale.exists(), "a leftover of a killed command is removed");
        assert!(running.0.is_dir(), "another command's staging area is not");
        assert!(unrelated.is_dir());
        let path = running.0.clone();
        drop(running);
        assert!(!path.exists(), "a staging area goes with the command that made it");
    }
}
