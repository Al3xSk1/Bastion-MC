//! A small client for Modrinth's public API (version 2), used only by the authoring
//! commands `profile add` and `profile check` (`profile::catalog`).
//!
//! The registry is untrusted twice over: what it says decides which bytes are put in front
//! of the user to sign, and its text ends up on the user's terminal. So:
//!
//! * Two hosts, exactly: `api.modrinth.com` for metadata, `cdn.modrinth.com` for files.
//!   They are on the allowlist of the client built *for these commands* ([`Live::new`]) and
//!   nowhere else: the launcher's built-in list, and with it every launch, is unchanged.
//!   Requests go through `net::http` — TLS 1.3, no redirects, bounded bodies.
//! * Every reply is parsed strictly and within bounds: a byte limit per endpoint, a limit
//!   on every list, identifiers restricted to base62, dates to one RFC 3339 form. Unknown
//!   *fields* are skipped, because the API grows; nothing that is used is taken on trust.
//!   What is used has a fixed shape, so nesting is bounded by the types; what is skipped
//!   is skipped by `serde_json` without recursion (a test nests 50 000 brackets).
//!   Text that is only shown is stripped of control characters and cut, and that
//!   includes the parser's own error messages, which can quote the input.
//! * A reply has to be about what was asked: the version with the id that was requested,
//!   builds of the project whose builds were listed, projects whose ids were sent, hashes
//!   that were looked up (an entry for any other hash ends the reading of the answer, so
//!   the question bounds the answer).
//! * A file is described by name, size, hashes and address. Before any of them is used the
//!   name must pass `paths::validate_file_name` and end in `.jar`, the size must be within
//!   [`MAX_MOD_BYTES`], and the address must be
//!   `https://cdn.modrinth.com/data/<the version's project>/versions/<x>/<the file name>`.
//!   [`Registry::download`] hashes while it writes and deletes the file unless size,
//!   SHA-512 and SHA-1 are the ones the API gave.
//! * The registry dates builds, not files, and a build's files can change after its date.
//!   A download therefore hands on the file server's own date for its copy of the bytes
//!   ([`Downloaded::stored`]); the rule made of it is `catalog`'s.
//! * Every request has a deadline, and one command makes a bounded number of them.
//!
//! What this does **not** give: the hash comes from the same party as the file. It shows
//! that the bytes are the ones the registry lists for that project and version — against
//! a broken transfer, a cache, a mirror — not that they are harmless. Whoever controls
//! the project's account publishes the bad file together with its hash. The decision to
//! run a jar stays where it was: the signature on the profile.
//!
//! API reference: <https://docs.modrinth.com> (`openapi.yaml`, read 2026-11-14). Its rules
//! that matter here: a user agent that identifies the program, 300 requests per minute per
//! address, and "slugs can change at any moment" — which is why the built-in aliases in
//! `catalog` name projects by their permanent id.

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::Write,
    marker::PhantomData,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use hyper::Method;
use serde::{Deserialize, de};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use crate::{
    config::Config,
    error::{Context, Error, Result},
    net::{
        http::{self, Connection, HttpClient, Sink},
        tls,
    },
    paths::validate_file_name,
    util,
};

pub const API_HOST: &str = "api.modrinth.com";
pub const CDN_HOST: &str = "cdn.modrinth.com";
/// The only hosts this module talks to.
pub const HOSTS: [&str; 2] = [API_HOST, CDN_HOST];
const API: &str = "https://api.modrinth.com/v2";
const FILE_PREFIX: &str = "https://cdn.modrinth.com/data/";
/// The registry asks for a user agent that identifies the program; a version helps them.
/// It carries nothing about the user or the machine.
pub const USER_AGENT: &str = concat!("bastion-mc/", env!("CARGO_PKG_VERSION"));

/// Largest mod file accepted. Among the newest builds of the 300 most-downloaded Fabric
/// mods the largest was 201 MB when this was written (2026-11-14); the median was 5 MB.
pub const MAX_MOD_BYTES: u64 = 512 << 20;
/// A project description carries its long text and the ids of all its versions.
const PROJECT_LIMIT: u64 = 4 << 20;
const PROJECTS_LIMIT: u64 = 32 << 20;
const VERSION_LIMIT: u64 = 2 << 20;
/// Version lists are asked for without change logs and filtered by the server; a project
/// with 3 700 versions answered 4.3 MB unfiltered.
const VERSIONS_LIMIT: u64 = 16 << 20;
/// Answers to hash lookups carry each version's change log.
const HASHES_LIMIT: u64 = 32 << 20;
const MAX_VERSIONS: usize = 8192;
const MAX_FILES: usize = 64;
const MAX_DEPENDENCIES: usize = 512;
/// Game versions or loaders one build lists (628 game versions were seen on one build).
const MAX_TAGS: usize = 4096;
/// Hashes per lookup, and ids per bulk request.
pub const MAX_HASHES: usize = 256;
pub const MAX_IDS: usize = 64;
/// Requests one command may make, downloads included. Far below the registry's limit of
/// 300 a minute, and what keeps a hostile dependency graph from turning into a crawl.
pub const MAX_REQUESTS: usize = 96;
const API_DEADLINE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------- checked values

/// Registry identifiers are base62; all seen so far have eight characters.
pub fn check_id(id: &str) -> Result<()> {
    if (8..=11).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(Error::Format(format!("{:?} is not a registry identifier", clean(id, 40))))
    }
}

/// What a project may be named by on the command line: a slug or an id. The registry
/// allows a few more characters in slugs (``!@$()`,"'`` and `+`); they would need escaping
/// inside a URL and are refused here. Such a project can be named by its id.
pub fn check_key(key: &str) -> Result<()> {
    let ok = (3..=64).contains(&key.len())
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !key.starts_with('.')
        && !key.contains("..");
    if ok {
        Ok(())
    } else {
        Err(Error::Usage(format!(
            "{:?} cannot be a project name here: use 3-64 characters from [A-Za-z0-9._-] (a slug or a project id)",
            clean(key, 80)
        )))
    }
}

/// Registry text that is only ever shown: one line, no control or bidi characters, cut.
fn clean(text: &str, max_chars: usize) -> String {
    util::sanitize_line(text.as_bytes(), max_chars)
}

fn hex_digest(what: &str, text: &str, hex_len: usize) -> Result<String> {
    if text.len() == hex_len && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(text.to_ascii_lowercase())
    } else {
        Err(Error::Policy(format!("the registry gives a {what} that is not {hex_len} hexadecimal digits")))
    }
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar (Howard Hinnant).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

/// The inverse: (year, month, day) of a day number.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// `2026-10-22T19:11:11.566967Z` to seconds since the epoch. Only UTC (`Z` or `+00:00`), an
/// optional fraction of up to nine digits, and only dates from 1970 on that exist.
pub fn parse_timestamp(text: &str) -> Result<u64> {
    let bad = || Error::Format(format!("{:?} is not a UTC time stamp", clean(text, 40)));
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes.len() > 35 {
        return Err(bad());
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = &bytes[range];
        digits.iter().all(u8::is_ascii_digit).then(|| digits.iter().fold(0i64, |n, d| n * 10 + i64::from(d - b'0')))
    };
    let punctuation = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    if punctuation.iter().any(|(at, byte)| bytes[*at] != *byte) {
        return Err(bad());
    }
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) =
        (number(0..4), number(5..7), number(8..10), number(11..13), number(14..16), number(17..19))
    else {
        return Err(bad());
    };
    let mut rest = &bytes[19..];
    if let Some(fraction) = rest.strip_prefix(b".") {
        let digits = fraction.iter().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=9).contains(&digits) {
            return Err(bad());
        }
        rest = &fraction[digits..];
    }
    if rest != b"Z" && rest != b"+00:00" {
        return Err(bad());
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_length = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err(bad()),
    };
    if year < 1970 || !(1..=month_length).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return Err(bad());
    }
    Ok((days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second) as u64)
}

/// `2026-10-22 19:11 UTC`.
pub fn utc(unix: u64) -> String {
    let (year, month, day) = civil_from_days((unix / 86_400) as i64);
    let seconds = unix % 86_400;
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", seconds / 3600, seconds % 3600 / 60)
}

/// How long ago `then` was, in words. A time after `now` is said to be in the future, never
/// rounded to "just now": it means a clock is wrong, and the caller treats it as fresh.
pub fn age(now: u64, then: u64) -> String {
    let plural = |n: u64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    let span = |seconds: u64| match seconds {
        s if s < 3600 => plural(s / 60, "minute"),
        s if s < 2 * 86_400 => plural(s / 3600, "hour"),
        s => plural(s / 86_400, "day"),
    };
    if then > now {
        format!("{} in the future by this machine's clock", span(then - now))
    } else {
        format!("{} ago", span(now - then))
    }
}

/// The `Date` header of an answer, in the one form servers are required to send
/// (`Sat, 14 Nov 2026 00:51:29 GMT`), as seconds since the epoch. Anything else is `None`.
pub fn parse_http_date(text: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let bytes = text.as_bytes();
    if bytes.len() != 29 || !text.is_ascii() || &text[3..5] != ", " || &text[25..] != " GMT" {
        return None;
    }
    let month = MONTHS.iter().position(|name| *name == &text[8..11])? + 1;
    let stamp = format!("{}-{month:02}-{}T{}Z", &text[12..16], &text[5..7], &text[17..25]);
    let spaced = bytes[7] == b' ' && bytes[11] == b' ' && bytes[16] == b' ';
    spaced.then(|| parse_timestamp(&stamp).ok()).flatten()
}

/// Percent-decode one path segment made of unreserved characters, `+` and escapes. `None`
/// for anything else — and for a segment that is empty, is a dot segment once decoded, or
/// decodes to a separator or a control byte.
fn decode_segment(segment: &str) -> Option<Vec<u8>> {
    let hex = |b: u8| char::from(b).to_digit(16).map(|d| d as u8);
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => {
                out.push(hex(*bytes.get(at + 1)?)? << 4 | hex(*bytes.get(at + 2)?)?);
                at += 3;
            }
            b if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+') => {
                out.push(b);
                at += 1;
            }
            _ => return None,
        }
    }
    let plain = !out.is_empty()
        && out.len() <= 255
        && out != b"."
        && out != b".."
        && out.iter().all(|b| !b.is_ascii_control() && !matches!(b, b'/' | b'\\'));
    plain.then_some(out)
}

/// A download address must be exactly
/// `https://cdn.modrinth.com/data/<project>/versions/<version id or number>/<file name>`:
/// lower-case scheme and host as the API writes them, so no port, no user part, no other
/// host; no query; and it must name the project the version belongs to and end in the
/// file's own name. (Old uploads have the version *number* in the fourth place, which is
/// why that one is only checked for shape.)
fn check_file_url(url: &str, project_id: &str, filename: &str) -> Result<()> {
    let refuse =
        |why: &str| Error::Policy(format!("the registry gives the download address {:?}, {why}", clean(url, 160)));
    if url.len() > 1024 {
        return Err(refuse("which is too long"));
    }
    let rest =
        url.strip_prefix(FILE_PREFIX).ok_or_else(|| refuse("which is not under https://cdn.modrinth.com/data/"))?;
    let segments: Vec<&str> = rest.split('/').collect();
    let [project, "versions", version, file] = segments.as_slice() else {
        return Err(refuse("which is not of the form data/<project>/versions/<version>/<file>"));
    };
    if *project != project_id {
        return Err(refuse("which belongs to another project"));
    }
    if decode_segment(version).is_none() {
        return Err(refuse("whose version part contains characters that are not accepted"));
    }
    if decode_segment(file).as_deref() != Some(filename.as_bytes()) {
        return Err(refuse("which does not end in the file's own name"));
    }
    Ok(())
}

// ---------------------------------------------------------------- the wire format

/// A JSON list with a ceiling on its length, enforced while reading (a list of a million
/// empty strings is small on the wire and large in memory).
fn bounded<'de, D, T, const MAX: usize>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: de::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const MAX: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAX: usize> de::Visitor<'de> for Bounded<T, MAX> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a list of at most {MAX} entries")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Vec<T>, A::Error> {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element()? {
                if out.len() == MAX {
                    return Err(de::Error::custom(format_args!("a list has more than {MAX} entries")));
                }
                out.push(item);
            }
            Ok(out)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, MAX>(PhantomData))
}

#[derive(Deserialize)]
struct RawLicense {
    #[serde(default)]
    id: String,
}

#[derive(Deserialize)]
struct RawProject {
    id: String,
    slug: String,
    title: String,
    project_type: String,
    status: String,
    #[serde(default)]
    license: Option<RawLicense>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawHashes {
    sha512: String,
    #[serde(default)]
    sha1: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawFile {
    hashes: RawHashes,
    url: String,
    filename: String,
    primary: bool,
    size: u64,
    #[serde(default)]
    file_type: Option<String>,
}

#[derive(Deserialize)]
struct RawDependency {
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    file_name: Option<String>,
    dependency_type: String,
}

#[derive(Deserialize)]
struct RawVersion {
    id: String,
    project_id: String,
    version_number: String,
    version_type: String,
    #[serde(default)]
    status: Option<String>,
    date_published: String,
    #[serde(deserialize_with = "bounded::<_, _, MAX_TAGS>")]
    game_versions: Vec<String>,
    #[serde(deserialize_with = "bounded::<_, _, MAX_TAGS>")]
    loaders: Vec<String>,
    #[serde(deserialize_with = "bounded::<_, _, MAX_FILES>")]
    files: Vec<RawFile>,
    #[serde(default, deserialize_with = "bounded::<_, _, MAX_DEPENDENCIES>")]
    dependencies: Vec<RawDependency>,
}

/// The parser's message can quote the input; it is cleaned like any other registry text.
fn unreadable(what: &str, error: serde_json::Error) -> Error {
    Error::Format(format!("the registry's {what} cannot be read: {}", clean(&error.to_string(), 200)))
}

fn json<'a, T: Deserialize<'a>>(what: &str, bytes: &'a [u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| unreadable(what, e))
}

/// A top-level JSON list of at most `MAX` entries.
fn json_list<'a, T: Deserialize<'a>, const MAX: usize>(what: &str, bytes: &'a [u8]) -> Result<Vec<T>> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    bounded::<_, T, MAX>(&mut deserializer)
        .and_then(|list| deserializer.end().map(|()| list))
        .map_err(|e| unreadable(what, e))
}

// ---------------------------------------------------------------- the checked model

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    /// For display. Slugs change; the id does not.
    pub slug: String,
    pub title: String,
    /// `mod`, `modpack`, `resourcepack`, `shader`, … as the registry says.
    pub kind: String,
    /// `approved`, `archived`, `withheld`, …
    pub status: String,
    /// SPDX identifier, or the registry's `LicenseRef-…` for anything else.
    pub license: String,
}

impl RawProject {
    fn check(self) -> Result<Project> {
        check_id(&self.id)?;
        let slug = clean(&self.slug, 64);
        let title = match clean(self.title.trim(), 80) {
            title if title.is_empty() => slug.clone(),
            title => title,
        };
        let license = self.license.map(|l| clean(l.id.trim(), 64)).filter(|id| !id.is_empty());
        Ok(Project {
            id: self.id,
            slug,
            title,
            kind: clean(&self.project_type, 32),
            status: clean(&self.status, 32),
            license: license.unwrap_or_else(|| "not stated".into()),
        })
    }
}

/// The release channel a build was published on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Channel {
    Release,
    Beta,
    Alpha,
    /// A channel this program does not know: never chosen.
    Other,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Channel::Release => "release",
            Channel::Beta => "beta",
            Channel::Alpha => "alpha",
            Channel::Other => "unknown channel",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Required,
    Optional,
    Incompatible,
    Embedded,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    /// For display: the registry's name for a dependency it does not host.
    pub file_name: Option<String>,
    pub kind: DependencyKind,
}

/// One downloadable file, every field checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionFile {
    pub filename: String,
    pub url: String,
    pub size: u64,
    /// Lower-case hexadecimal.
    pub sha512: String,
    pub sha1: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Version {
    pub id: String,
    pub project_id: String,
    /// For display.
    pub number: String,
    pub channel: Channel,
    /// `listed` is the only status a build is taken with.
    pub status: String,
    /// Seconds since the epoch.
    pub published: u64,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub dependencies: Vec<Dependency>,
    /// As received. A file is checked when it is about to be used ([`Version::primary_jar`],
    /// [`Version::file_with_sha512`]): one oddly named extra file among thousands of
    /// versions must not make a whole project unusable, but no unchecked file is ever used.
    files: Vec<RawFile>,
}

impl RawFile {
    fn check(&self, project_id: &str) -> Result<VersionFile> {
        let shown = clean(&self.filename, 80);
        if validate_file_name(&self.filename).is_err() {
            return Err(Error::Policy(format!(
                "the registry names the file {shown:?}; only names made of [A-Za-z0-9._+-] without a leading dot \
                 are accepted (download such a mod by hand and use `profile lock`)"
            )));
        }
        if !self.filename.ends_with(".jar") {
            return Err(Error::Policy(format!("the registry's file {shown:?} is not a .jar")));
        }
        if self.size == 0 || self.size > MAX_MOD_BYTES {
            return Err(Error::Policy(format!(
                "the registry says {shown} has {} bytes; accepted are 1 to {MAX_MOD_BYTES}",
                self.size
            )));
        }
        let sha512 = hex_digest("SHA-512", &self.hashes.sha512, 128)?;
        let sha1 = self.hashes.sha1.as_deref().map(|h| hex_digest("SHA-1", h, 40)).transpose()?;
        check_file_url(&self.url, project_id, &self.filename)?;
        Ok(VersionFile { filename: self.filename.clone(), url: self.url.clone(), size: self.size, sha512, sha1 })
    }
}

impl RawVersion {
    fn check(self) -> Result<Version> {
        check_id(&self.id)?;
        check_id(&self.project_id)?;
        let tags = |list: Vec<String>, what: &str| -> Result<Vec<String>> {
            if list.iter().any(|tag| tag.len() > 64) {
                return Err(Error::Format(format!("the registry lists a {what} of more than 64 bytes")));
            }
            Ok(list.iter().map(|tag| clean(tag, 64)).collect())
        };
        let dependencies = self
            .dependencies
            .into_iter()
            .map(|d| {
                for id in d.project_id.iter().chain(&d.version_id) {
                    check_id(id)?;
                }
                Ok(Dependency {
                    project_id: d.project_id,
                    version_id: d.version_id,
                    file_name: d.file_name.map(|name| clean(&name, 100)),
                    kind: match d.dependency_type.as_str() {
                        "required" => DependencyKind::Required,
                        "optional" => DependencyKind::Optional,
                        "incompatible" => DependencyKind::Incompatible,
                        "embedded" => DependencyKind::Embedded,
                        _ => DependencyKind::Other,
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Version {
            published: parse_timestamp(&self.date_published)?,
            number: match clean(self.version_number.trim(), 80) {
                number if number.is_empty() => "(unnamed)".into(),
                number => number,
            },
            channel: match self.version_type.as_str() {
                "release" => Channel::Release,
                "beta" => Channel::Beta,
                "alpha" => Channel::Alpha,
                _ => Channel::Other,
            },
            // The live API always states it. A build without a status is not taken.
            status: self.status.map(|s| clean(&s, 32)).unwrap_or_else(|| "not stated".into()),
            game_versions: tags(self.game_versions, "game version")?,
            loaders: tags(self.loaders, "loader")?,
            dependencies,
            files: self.files,
            id: self.id,
            project_id: self.project_id,
        })
    }
}

impl Version {
    pub fn is_listed(&self) -> bool {
        self.status == "listed"
    }

    /// The mod file of this build, checked: the file marked primary, else the first — in
    /// both cases only among files without a special type (sources, dev and javadoc jars,
    /// signatures and packs carry one).
    pub fn primary_jar(&self) -> Result<VersionFile> {
        let plain = || self.files.iter().filter(|f| f.file_type.is_none());
        plain()
            .find(|f| f.primary)
            .or_else(|| plain().next())
            .ok_or_else(|| Error::Policy(format!("the registry lists no mod file for version {}", self.number)))?
            .check(&self.project_id)
    }

    /// The file of this build that has the given SHA-512, checked. `None` if there is none.
    pub fn file_with_sha512(&self, sha512: &str) -> Option<Result<VersionFile>> {
        self.files
            .iter()
            .find(|f| f.hashes.sha512.eq_ignore_ascii_case(sha512))
            .map(|file| file.check(&self.project_id))
    }

    /// The registry's name for the file with that hash, for display only.
    pub fn file_name_for(&self, sha512: &str) -> Option<String> {
        self.files.iter().find(|f| f.hashes.sha512.eq_ignore_ascii_case(sha512)).map(|f| clean(&f.filename, 100))
    }

    pub fn supports(&self, minecraft: &str, loaders: &[&str]) -> bool {
        self.game_versions.iter().any(|v| v == minecraft) && self.loaders.iter().any(|l| loaders.contains(&l.as_str()))
    }
}

pub fn parse_project(bytes: &[u8]) -> Result<Project> {
    json::<RawProject>("project description", bytes)?.check()
}

pub fn parse_projects(bytes: &[u8]) -> Result<Vec<Project>> {
    json_list::<RawProject, MAX_IDS>("project list", bytes)?.into_iter().map(RawProject::check).collect()
}

pub fn parse_version(bytes: &[u8]) -> Result<Version> {
    json::<RawVersion>("version description", bytes)?.check()
}

pub fn parse_versions(bytes: &[u8]) -> Result<Vec<Version>> {
    json_list::<RawVersion, MAX_VERSIONS>("version list", bytes)?.into_iter().map(RawVersion::check).collect()
}

/// The entries of a hash lookup's answer, held to the question while they are read: a
/// key that was not asked about, or that comes a second time, ends the reading there. So
/// the answer can never hold more entries than there were hashes in the question, however
/// many it claims to have.
struct Lookup<'a>(&'a [String]);

impl<'de> de::Visitor<'de> for Lookup<'_> {
    type Value = Vec<(String, RawVersion)>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "an object with an entry for at most {} hashes", self.0.len())
    }

    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> std::result::Result<Self::Value, A::Error> {
        let mut entries: Vec<(String, RawVersion)> = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            let hash = key.to_ascii_lowercase();
            if !self.0.contains(&hash) {
                return Err(de::Error::custom("an entry for a hash that was not asked about"));
            }
            if entries.iter().any(|(seen, _)| *seen == hash) {
                return Err(de::Error::custom("two entries for one hash"));
            }
            entries.push((hash, map.next_value()?));
        }
        Ok(entries)
    }
}

/// The answer to a hash lookup: hash → the version that has a file with it. Only hashes
/// that were asked about are accepted, each once, and only with a version that really
/// lists a file with that hash. `asked` holds lower-case hexadecimal.
pub fn parse_hash_lookup(bytes: &[u8], asked: &[String]) -> Result<BTreeMap<String, Version>> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let entries = de::Deserializer::deserialize_map(&mut deserializer, Lookup(asked))
        .and_then(|entries| deserializer.end().map(|()| entries))
        .map_err(|e| unreadable("answer to a hash lookup", e))?;
    let mut out = BTreeMap::new();
    for (hash, version) in entries {
        let version = version.check()?;
        if !version.files.iter().any(|f| f.hashes.sha512.eq_ignore_ascii_case(&hash)) {
            return Err(Error::Format(format!(
                "the registry names version {} for a hash that none of its files has",
                version.id
            )));
        }
        out.insert(hash, version);
    }
    Ok(out)
}

// ---------------------------------------------------------------- transport

/// One request to the registry or its file server.
pub struct Call<'a> {
    pub method: Method,
    pub url: &'a str,
    /// A JSON body.
    pub body: Option<&'a [u8]>,
    /// Most bytes of answer accepted.
    pub limit: u64,
    /// Time allowed for the whole exchange.
    pub deadline: Duration,
}

/// What came back besides the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    /// The server's `Date` header, when it sent a readable one: seconds since the epoch.
    pub date: Option<u64>,
    /// Its `Last-Modified` header, likewise: when the server says it stored what it sends.
    pub last_modified: Option<u64>,
}

/// The seam between the registry logic and the network, so that the logic can be run
/// against scripted replies. The body goes to `sink`.
pub trait Transport: Sync {
    fn call(&self, call: Call<'_>, sink: &mut (dyn Sink + Send)) -> impl Future<Output = Result<Answer>> + Send;
}

/// The real thing: the launcher's HTTP client with the two registry hosts allowed, one
/// kept-alive connection per host.
pub struct Live {
    http: HttpClient,
    idle: Mutex<Vec<Connection>>,
}

struct Counting<'a> {
    inner: &'a mut (dyn Sink + Send),
    bytes: u64,
}

impl Sink for Counting<'_> {
    fn accept(&mut self, chunk: &[u8]) -> Result<()> {
        self.bytes += chunk.len() as u64;
        self.inner.accept(chunk)
    }
}

impl Live {
    /// A client for the authoring commands: the launcher's TLS policy, pins and proxy
    /// setting, and an allowlist that additionally holds the two registry hosts. Nothing
    /// else in the program is given a client that can reach them.
    pub fn new(cfg: &Config) -> Result<Self> {
        let roots = tls::load_system_roots()?;
        let pins = tls::PinSet::from_config(&cfg.tls.pins, cfg.tls.builtin_pins)?;
        let proxy = cfg.network.launcher_proxy.as_deref().map(crate::config::parse_socks5).transpose()?;
        let hosts: Vec<String> = HOSTS.iter().map(|h| h.to_string()).collect();
        Ok(Self::with_client(HttpClient::new(tls::client_config(roots, pins)?, &hosts).with_socks5(proxy)))
    }

    /// For tests: a client with its own trust anchors and dial address.
    #[doc(hidden)]
    pub fn with_client(http: HttpClient) -> Self {
        Live { http, idle: Mutex::new(Vec::new()) }
    }

    /// The SOCKS5 proxy every connection of this client goes through, if one is configured.
    pub fn proxy(&self) -> Option<std::net::SocketAddr> {
        self.http.socks5()
    }

    fn take(&self, host: &str) -> Option<Connection> {
        let mut idle = self.idle.lock().expect("connection list");
        let at = idle.iter().position(|c| c.host() == host)?;
        Some(idle.swap_remove(at)).filter(|c| !c.is_closed())
    }
}

impl Transport for Live {
    async fn call(&self, call: Call<'_>, sink: &mut (dyn Sink + Send)) -> Result<Answer> {
        let target = self.http.parse_url(call.url)?;
        // The client also knows the launcher's own hosts; this module has no business there.
        if !HOSTS.contains(&target.host.as_str()) {
            return Err(Error::Policy(format!("{:?} is not a registry host", target.host)));
        }
        let mut headers = vec![http::header("user-agent", USER_AGENT)?];
        if call.body.is_some() {
            headers.push(http::header("content-type", "application/json")?);
        }
        let body = call.body.map(Bytes::copy_from_slice);
        let exchange = async {
            let mut kept = self.take(&target.host);
            loop {
                let reused = kept.is_some();
                let mut connection = match kept.take() {
                    Some(connection) => connection,
                    None => self.http.connect(&target.host).await?,
                };
                let mut counting = Counting { inner: &mut *sink, bytes: 0 };
                let outcome = connection
                    .call(call.method.clone(), &target.path, &headers, body.clone(), call.limit, &mut counting)
                    .await;
                match outcome {
                    Ok(head) => {
                        if !connection.is_closed() {
                            self.idle.lock().expect("connection list").push(connection);
                        }
                        let time = |name: hyper::header::HeaderName| {
                            head.headers.get(name).and_then(|value| value.to_str().ok()).and_then(parse_http_date)
                        };
                        return Ok(Answer {
                            status: head.status,
                            date: time(hyper::header::DATE),
                            last_modified: time(hyper::header::LAST_MODIFIED),
                        });
                    }
                    // A kept connection the server has closed in the meantime fails before
                    // any byte of an answer: that, and only that, is tried again, once.
                    Err(Error::Net(_)) if reused && counting.bytes == 0 => continue,
                    Err(e) => return Err(e),
                }
            }
        };
        tokio::time::timeout(call.deadline, exchange).await.map_err(|_| {
            Error::Net(format!("{}: no complete answer within {} seconds", target.host, call.deadline.as_secs()))
        })?
    }
}

// ---------------------------------------------------------------- the API

/// The earlier of a local time and a registry clock cell (0: the registry has not answered).
pub fn earlier(local: u64, registry_clock: &AtomicU64) -> u64 {
    match registry_clock.load(Ordering::Relaxed) {
        0 => local,
        server => server.min(local),
    }
}

/// What a verified download produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    /// What the profile manifest records.
    pub sha256: String,
    pub size: u64,
    /// Since when the file server says it has held these bytes (its `Last-Modified`), in
    /// seconds since the epoch; `None` if it stated no readable date.
    ///
    /// The registry dates a *build*, and its API lets a project's members put a file into
    /// a build that is already published (`POST /version/{id}/file`) and make it the
    /// primary one (`primary_file` in `PATCH /version/{id}`), while no answer of the API
    /// dates a file. This is the only date there is for the bytes themselves. It is the
    /// file server's statement about its own copy, no more: never earlier than the upload,
    /// and later whenever the file server rewrote its storage (`catalog::file_date` has
    /// what was observed on 2026-11-14).
    pub stored: Option<u64>,
}

struct FileSink {
    file: File,
    sha512: Sha512,
    sha256: Sha256,
    sha1: Sha1,
    written: u64,
    expected: u64,
}

impl Sink for FileSink {
    fn accept(&mut self, chunk: &[u8]) -> Result<()> {
        self.written += chunk.len() as u64;
        if self.written > self.expected {
            return Err(Error::Integrity(format!(
                "the file server sends more than the {} bytes the registry lists for this file",
                self.expected
            )));
        }
        self.sha512.update(chunk);
        self.sha256.update(chunk);
        self.sha1.update(chunk);
        self.file.write_all(chunk).ctx(|| "writing the download")
    }
}

/// The JSON routes of the API and the verified download, over any [`Transport`].
pub struct Registry<'a, T> {
    transport: &'a T,
    requests: AtomicUsize,
    /// The `Date` of the API's most recent answer; 0 until there was one.
    clock: AtomicU64,
}

impl<'a, T: Transport> Registry<'a, T> {
    pub fn new(transport: &'a T) -> Self {
        Registry { transport, requests: AtomicUsize::new(0), clock: AtomicU64::new(0) }
    }

    /// The registry's own clock, as of its last answer: 0 while there has been none.
    ///
    /// The age of a build is the registry's publication date against "now". This machine's
    /// clock may be wrong, and a clock that runs ahead would make a fresh build look old —
    /// so callers measure against the *earlier* of the two clocks ([`now`](Self::now)):
    /// a wrong clock on either side can then only make the quarantine stricter.
    pub fn clock(&self) -> &AtomicU64 {
        &self.clock
    }

    pub fn server_time(&self) -> Option<u64> {
        Some(self.clock.load(Ordering::Relaxed)).filter(|seconds| *seconds != 0)
    }

    /// The time to measure the age of a build against: the earlier of this machine's
    /// clock (`local`) and the registry's, once the registry has answered.
    pub fn now(&self, local: u64) -> u64 {
        earlier(local, &self.clock)
    }

    /// Requests made so far.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    fn count(&self) -> Result<()> {
        if self.requests.fetch_add(1, Ordering::Relaxed) >= MAX_REQUESTS {
            return Err(Error::Policy(format!(
                "this command would need more than {MAX_REQUESTS} requests to the registry; refusing to go on \
                 (a dependency graph this large is better resolved by hand)"
            )));
        }
        Ok(())
    }

    async fn exchange(&self, method: Method, url: &str, body: Option<&[u8]>, limit: u64) -> Result<(u16, Vec<u8>)> {
        self.count()?;
        let mut answer = Vec::new();
        let head = self.transport.call(Call { method, url, body, limit, deadline: API_DEADLINE }, &mut answer).await?;
        if let Some(date) = head.date {
            self.clock.store(date, Ordering::Relaxed);
        }
        Ok((head.status, answer))
    }

    fn refused(what: &str, status: u16) -> Error {
        let why = match status {
            410 => " (this version of the registry's API has been retired; the launcher needs an update)",
            429 => " (too many requests; wait a minute)",
            _ => "",
        };
        Error::Net(format!("the registry answered HTTP {status} for {what}{why}"))
    }

    /// A project by slug or id. `None` when the registry has no such project.
    pub async fn project(&self, key: &str) -> Result<Option<Project>> {
        check_key(key)?;
        match self.exchange(Method::GET, &format!("{API}/project/{key}"), None, PROJECT_LIMIT).await? {
            (200, body) => parse_project(&body).map(Some),
            (404, _) => Ok(None),
            (status, _) => Err(Self::refused(&format!("project {key}"), status)),
        }
    }

    /// Several projects by id, in one request. Ids the registry does not know are absent.
    pub async fn projects(&self, ids: &[String]) -> Result<Vec<Project>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if ids.len() > MAX_IDS {
            return Err(Error::Policy(format!("more than {MAX_IDS} projects in one request")));
        }
        ids.iter().try_for_each(|id| check_id(id))?;
        let list = serde_json::to_vec(ids).map_err(|e| Error::Other(format!("encoding a request: {e}")))?;
        let query = http::form_body(&[("ids", list.as_slice())]);
        let url = format!("{API}/projects?{}", String::from_utf8_lossy(&query));
        match self.exchange(Method::GET, &url, None, PROJECTS_LIMIT).await? {
            (200, body) => {
                let projects = parse_projects(&body)?;
                // The answer has to be about what was asked: those ids, each at most once.
                let about_the_question = projects.iter().enumerate().all(|(at, project)| {
                    ids.contains(&project.id) && projects[..at].iter().all(|earlier| earlier.id != project.id)
                });
                if !about_the_question {
                    return Err(Error::Format(
                        "the registry's list of projects holds one that was not asked for, or one twice".into(),
                    ));
                }
                Ok(projects)
            }
            (status, _) => Err(Self::refused("a list of projects", status)),
        }
    }

    /// The builds of a project for one game version and a set of loaders, as the registry
    /// filters them. The caller filters again: the server's filter is a saving, not a check.
    pub async fn versions(&self, project_id: &str, minecraft: &str, loaders: &[&str]) -> Result<Vec<Version>> {
        check_id(project_id)?;
        let encode =
            |value: &[&str]| serde_json::to_vec(value).map_err(|e| Error::Other(format!("encoding a request: {e}")));
        let (loaders, game_versions) = (encode(loaders)?, encode(&[minecraft])?);
        let query = http::form_body(&[
            ("loaders", loaders.as_slice()),
            ("game_versions", game_versions.as_slice()),
            ("include_changelog", b"false".as_slice()),
        ]);
        let url = format!("{API}/project/{project_id}/version?{}", String::from_utf8_lossy(&query));
        match self.exchange(Method::GET, &url, None, VERSIONS_LIMIT).await? {
            (200, body) => {
                let versions = parse_versions(&body)?;
                if versions.iter().any(|v| v.project_id != project_id) {
                    return Err(Error::Format(format!(
                        "the registry's list of builds of project {project_id} contains a build of another project"
                    )));
                }
                Ok(versions)
            }
            (status, _) => Err(Self::refused(&format!("the versions of project {project_id}"), status)),
        }
    }

    /// One version by id. `None` when the registry has no such version.
    pub async fn version(&self, id: &str) -> Result<Option<Version>> {
        check_id(id)?;
        match self.exchange(Method::GET, &format!("{API}/version/{id}"), None, VERSION_LIMIT).await? {
            (200, body) => match parse_version(&body)? {
                version if version.id == id => Ok(Some(version)),
                _ => Err(Error::Format(format!("asked for version {id}, the registry answered with another one"))),
            },
            (404, _) => Ok(None),
            (status, _) => Err(Self::refused(&format!("version {id}"), status)),
        }
    }

    /// Which versions the files with these SHA-512 hashes belong to. Hashes the registry
    /// does not know are absent from the answer.
    pub async fn by_hash(&self, sha512: &[String]) -> Result<BTreeMap<String, Version>> {
        let mut known = BTreeMap::new();
        for chunk in sha512.chunks(MAX_HASHES) {
            let asked: Vec<String> = chunk.iter().map(|h| hex_digest("SHA-512", h, 128)).collect::<Result<_>>()?;
            let body = serde_json::to_vec(&serde_json::json!({ "hashes": asked, "algorithm": "sha512" }))
                .map_err(|e| Error::Other(format!("encoding a request: {e}")))?;
            match self.exchange(Method::POST, &format!("{API}/version_files"), Some(&body), HASHES_LIMIT).await? {
                (200, answer) => known.extend(parse_hash_lookup(&answer, &asked)?),
                (status, _) => return Err(Self::refused("a hash lookup", status)),
            }
        }
        Ok(known)
    }

    /// Fetch `file` into the new file `dest` (mode 0600), and keep it only if its size,
    /// SHA-512 and — when the registry gave one — SHA-1 are what the registry said. `dest`
    /// must not exist: nothing is ever written over, and a file that was there before is
    /// not touched. What this call created is removed again on any failure, and also when
    /// the call is abandoned half way.
    pub async fn download(&self, file: &VersionFile, dest: &Path) -> Result<Downloaded> {
        // A transfer that dies is tried once more from the start. Wrong bytes are not.
        match self.download_once(file, dest).await {
            Err(Error::Net(_)) => self.download_once(file, dest).await,
            outcome => outcome,
        }
    }

    async fn download_once(&self, file: &VersionFile, dest: &Path) -> Result<Downloaded> {
        /// Removes the file unless the download was completed and verified.
        struct Unfinished<'p> {
            path: &'p Path,
            keep: bool,
        }
        impl Drop for Unfinished<'_> {
            fn drop(&mut self) {
                if !self.keep {
                    let _ = fs::remove_file(self.path);
                }
            }
        }
        self.count()?;
        let out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dest)
            .ctx(|| format!("creating {}", dest.display()))?;
        let mut unfinished = Unfinished { path: dest, keep: false };
        let mut sink = FileSink {
            file: out,
            sha512: Sha512::new(),
            sha256: Sha256::new(),
            sha1: Sha1::new(),
            written: 0,
            expected: file.size,
        };
        // At least 64 KiB a second on top of two minutes, and never more than an hour.
        let deadline = Duration::from_secs((120 + file.size / (64 << 10)).min(3600));
        let call = Call { method: Method::GET, url: &file.url, body: None, limit: MAX_MOD_BYTES, deadline };
        let answer = self.transport.call(call, &mut sink).await?;
        let status = answer.status;
        if status != 200 {
            return Err(Error::Net(format!("the file server answered HTTP {status} for {}", file.filename)));
        }
        if sink.written != file.size {
            return Err(Error::Integrity(format!(
                "{}: received {} bytes, the registry lists {}",
                file.filename, sink.written, file.size
            )));
        }
        let FileSink { file: out, sha512, sha256, sha1, .. } = sink;
        let got = util::hex_encode(&sha512.finalize());
        if got != file.sha512 {
            return Err(Error::Integrity(format!(
                "{}: the downloaded file has SHA-512 {got}, the registry lists {}",
                file.filename, file.sha512
            )));
        }
        let got = util::hex_encode(&sha1.finalize());
        if file.sha1.as_ref().is_some_and(|listed| *listed != got) {
            return Err(Error::Integrity(format!(
                "{}: the downloaded file has SHA-1 {got}, the registry lists {}",
                file.filename,
                file.sha1.as_deref().unwrap_or_default()
            )));
        }
        out.sync_all().ctx(|| format!("writing {}", dest.display()))?;
        unfinished.keep = true;
        Ok(Downloaded { sha256: util::hex_encode(&sha256.finalize()), size: file.size, stored: answer.last_modified })
    }
}

// ---------------------------------------------------------------- robustness harness

/// Arbitrary bytes as every kind of registry answer. Whatever is accepted must hold what
/// the rest of the program relies on: identifiers that are safe inside a URL, text that is
/// safe on a terminal, and files whose name, size, hashes and address passed the checks.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    let shown = |text: &str| !text.chars().any(|c| c.is_control()) && text.chars().count() <= 120;
    let id = |text: &str| (8..=11).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric());
    let project = |p: &Project| {
        assert!(id(&p.id), "{:?}", p.id);
        assert!(!p.title.is_empty() || p.slug.is_empty());
        for text in [&p.slug, &p.title, &p.kind, &p.status, &p.license] {
            assert!(shown(text), "{text:?}");
        }
    };
    let file = |f: &VersionFile, version: &Version| {
        assert!(validate_file_name(&f.filename).is_ok() && f.filename.ends_with(".jar"), "{:?}", f.filename);
        assert!((1..=MAX_MOD_BYTES).contains(&f.size));
        assert!(f.sha512.len() == 128 && f.sha512.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        assert!(f.sha1.as_ref().is_none_or(|h| h.len() == 40));
        let prefix = format!("{FILE_PREFIX}{}/versions/", version.project_id);
        let rest = f.url.strip_prefix(&prefix).unwrap_or_else(|| panic!("{:?}", f.url));
        assert!(!rest.contains(['?', '#', '\\', ' ']) && rest.matches('/').count() == 1, "{rest:?}");
        assert!(f.url.is_ascii() && !f.url.bytes().any(|b| b.is_ascii_control()));
    };
    let version = |v: &Version| {
        assert!(id(&v.id) && id(&v.project_id));
        assert!(shown(&v.number) && shown(&v.status) && !v.number.is_empty());
        assert!(v.game_versions.iter().chain(&v.loaders).all(|tag| shown(tag)));
        for d in &v.dependencies {
            assert!(d.project_id.iter().chain(&d.version_id).all(|i| id(i)));
            assert!(d.file_name.as_ref().is_none_or(|name| shown(name)));
        }
        // Rendering a date never fails, whatever the number.
        assert_eq!(utc(v.published).len(), 20);
        if let Ok(f) = v.primary_jar() {
            file(&f, v);
            let again = v.file_with_sha512(&f.sha512).expect("found by its own hash").expect("checked before");
            assert_eq!(again.sha512, f.sha512);
        }
    };
    if let Ok(p) = parse_project(data) {
        project(&p);
    }
    if let Ok(list) = parse_projects(data) {
        assert!(list.len() <= MAX_IDS);
        list.iter().for_each(project);
    }
    if let Ok(v) = parse_version(data) {
        version(&v);
    }
    if let Ok(list) = parse_versions(data) {
        assert!(list.len() <= MAX_VERSIONS);
        list.iter().for_each(version);
    }
    // A hash lookup that asks for what the answer happens to contain, and one that does not.
    let text = String::from_utf8_lossy(data);
    let asked: Vec<String> = text
        .split('"')
        .filter(|piece| piece.len() == 128 && piece.bytes().all(|b| b.is_ascii_hexdigit()))
        .take(4)
        .map(str::to_ascii_lowercase)
        .collect();
    if let Ok(map) = parse_hash_lookup(data, &asked) {
        assert!(map.len() <= asked.len());
        for (hash, v) in &map {
            assert!(asked.contains(hash));
            version(v);
            assert!(v.file_name_for(hash).is_some_and(|name| shown(&name)));
        }
    }
    if let Ok(map) = parse_hash_lookup(data, &[]) {
        assert!(map.is_empty());
    }
    // The small parsers on their own.
    if let Ok(seconds) = parse_timestamp(&text) {
        let printed = utc(seconds);
        assert_eq!(&text[..10], &printed[..10], "a date that does not print as it was read");
    }
    if let Some(seconds) = parse_http_date(&text) {
        assert_eq!(&utc(seconds)[..4], &text[12..16], "a date header that does not print as it was read");
    }
    if let Some(decoded) = decode_segment(&text) {
        assert!(!decoded.is_empty() && !decoded.contains(&b'/') && decoded != b"..");
    }
    let _ = check_key(&text);
    let _ = check_file_url(&text, "AABBCCDD", "a.jar");
}

#[cfg(test)]
pub(crate) mod testing {
    //! A scripted registry and builders for its answers, shared with the tests of `catalog`.
    use super::*;
    use serde_json::{Value, json};

    /// Answers by method and path (the query is ignored for matching, recorded for
    /// inspection). A path nothing was scripted for answers 404 with an empty body, as the
    /// real API does for an unknown project.
    #[derive(Default)]
    pub struct Scripted {
        routes: Mutex<Vec<Route>>,
        seen: Mutex<Vec<String>>,
        /// The `Date` every answer carries, if any.
        date: Mutex<Option<u64>>,
        /// The `Last-Modified` of the answers for a path, where one was scripted.
        stored: Mutex<BTreeMap<String, u64>>,
    }

    struct Route {
        method: String,
        path: String,
        status: u16,
        body: Vec<u8>,
    }

    impl Scripted {
        pub fn on(&self, method: &str, path: &str, status: u16, body: impl Into<Vec<u8>>) -> &Self {
            self.routes.lock().unwrap().push(Route {
                method: method.into(),
                path: path.into(),
                status,
                body: body.into(),
            });
            self
        }

        pub fn json(&self, method: &str, path: &str, value: &Value) -> &Self {
            self.on(method, path, 200, serde_json::to_vec(value).unwrap())
        }

        /// From now on every answer says it was sent at this time.
        pub fn clock(&self, date: Option<u64>) -> &Self {
            *self.date.lock().unwrap() = date;
            self
        }

        /// From now on answers for `path` say when the server stored what it sends
        /// (`Last-Modified`), or with `None` say nothing about it.
        pub fn stored(&self, path: &str, when: Option<u64>) -> &Self {
            let mut stored = self.stored.lock().unwrap();
            match when {
                Some(when) => stored.insert(path.to_string(), when),
                None => stored.remove(path),
            };
            self
        }

        /// Every request so far: `METHOD host/path?query body`.
        pub fn seen(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }

        pub fn count(&self, needle: &str) -> usize {
            self.seen().iter().filter(|request| request.contains(needle)).count()
        }
    }

    impl Transport for Scripted {
        async fn call(&self, call: Call<'_>, sink: &mut (dyn Sink + Send)) -> Result<Answer> {
            let rest = call.url.strip_prefix("https://").expect("only https is ever asked for");
            let (host, path_and_query) = rest.split_once('/').expect("a path");
            assert!(HOSTS.contains(&host), "a request to {host}");
            assert!(call.deadline >= Duration::from_secs(60) && call.deadline <= Duration::from_secs(3600));
            let path = format!("/{}", path_and_query.split('?').next().unwrap());
            let body = call.body.map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            let request = format!("{} {host}/{path_and_query} {body}", call.method);
            self.seen.lock().unwrap().push(request.trim_end().to_string());
            let scripted = self
                .routes
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|route| route.method == call.method.as_str() && route.path == path)
                .map(|route| (route.status, route.body.clone()));
            let (status, body) = scripted.unwrap_or((404, Vec::new()));
            if body.len() as u64 > call.limit {
                return Err(Error::Net(format!("response exceeds the {} byte limit", call.limit)));
            }
            // In pieces, as a network delivers it.
            for chunk in body.chunks(997) {
                sink.accept(chunk)?;
            }
            let last_modified = self.stored.lock().unwrap().get(&path).copied();
            Ok(Answer { status, date: *self.date.lock().unwrap(), last_modified })
        }
    }

    pub fn sha512_hex(content: &[u8]) -> String {
        util::hex_encode(&Sha512::digest(content))
    }

    pub fn project(id: &str, slug: &str, title: &str) -> Value {
        json!({
            "id": id, "slug": slug, "title": title, "description": "short", "body": "a long text",
            "project_type": "mod", "status": "approved", "team": "TTTTTTTT",
            "license": { "id": "MIT", "name": "MIT License", "url": null },
            "client_side": "required", "server_side": "optional", "versions": ["VVVVVVVV"],
            "game_versions": ["1.21.4"], "loaders": ["fabric"]
        })
    }

    /// The path of a file on the file server, as the registry writes it (`+` escaped).
    pub fn file_path(project: &str, version: &str, name: &str) -> String {
        format!("/data/{project}/versions/{version}/{}", name.replace('+', "%2B"))
    }

    pub fn file(project: &str, version: &str, name: &str, content: &[u8]) -> Value {
        json!({
            "hashes": { "sha512": sha512_hex(content), "sha1": util::hex_encode(&Sha1::digest(content)) },
            "url": format!("https://cdn.modrinth.com{}", file_path(project, version, name)),
            "filename": name, "primary": true, "size": content.len(), "file_type": null
        })
    }

    /// A listed build for Minecraft 1.21.4 on Fabric, without dependencies.
    pub fn version(id: &str, project: &str, number: &str, channel: &str, published: &str, file: Value) -> Value {
        json!({
            "id": id, "project_id": project, "author_id": "UUUUUUUU", "name": number,
            "version_number": number, "version_type": channel, "status": "listed", "requested_status": null,
            "date_published": published, "downloads": 12, "featured": false, "changelog_url": null,
            "environment": "client_and_server", "game_versions": ["1.21.4"], "loaders": ["fabric"],
            "files": [file], "dependencies": []
        })
    }

    pub fn dependency(project: Option<&str>, version: Option<&str>, kind: &str) -> Value {
        json!({ "project_id": project, "version_id": version, "file_name": null, "dependency_type": kind })
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};
    use serde_json::{Value, json};

    const SHA512: &str = "090e8b47b413523b8682c3953beb6d99e5a92473a8d0580bdbf6a057352aadb8ad8b7a2d08f36c99c14ed3294923102c45724c88b24d697a05b729925ce03de3";

    /// A build as `api.modrinth.com` returned it on 2026-11-14 (ViaFabric, change log cut).
    const REAL_VERSION: &str = r#"{"game_versions":["26.1","26.1.1","26.1.2","26.2","26.3"],"loaders":["fabric"],"id":"z1FVDMnq","project_id":"YlKdE5VK","author_id":"Hy1rq6bx","featured":false,"name":"[26.x] ViaFabric 0.4.22+188","version_number":"0.4.22+188-26.x","changelog":"cut","changelog_url":null,"date_published":"2026-10-22T19:11:11.566967Z","downloads":1,"version_type":"alpha","status":"listed","requested_status":null,"files":[{"hashes":{"sha1":"602797ada2a9bcc4aaecbfdd2f89e4c9397787cc","sha512":"090e8b47b413523b8682c3953beb6d99e5a92473a8d0580bdbf6a057352aadb8ad8b7a2d08f36c99c14ed3294923102c45724c88b24d697a05b729925ce03de3"},"url":"https://cdn.modrinth.com/data/YlKdE5VK/versions/z1FVDMnq/ViaFabric-0.4.22%2B188-26.x.jar","filename":"ViaFabric-0.4.22+188-26.x.jar","primary":true,"size":5916758,"file_type":null}],"dependencies":[{"version_id":null,"project_id":"P1OZGk5p","file_name":null,"dependency_type":"embedded"},{"version_id":null,"project_id":"P7dR8mSH","file_name":null,"dependency_type":"required"}],"environment":"client_and_server"}"#;

    /// A project as returned on the same day (long text and version list cut).
    const REAL_PROJECT: &str = r#"{"client_side":"optional","server_side":"optional","game_versions":["1.8.9","26.3"],"id":"YlKdE5VK","slug":"viafabric","project_type":"mod","team":"rwBnRd6Q","organization":"DaPoZTlV","title":"ViaFabric","description":"Client-side and server-side ViaVersion for Fabric","body":"cut","body_url":null,"published":"2021-01-04T14:38:39.325273Z","updated":"2026-10-22T19:11:13.386478Z","approved":"2021-01-04T14:38:39.325273Z","queued":null,"status":"approved","requested_status":"approved","moderator_message":null,"license":{"id":"GPL-3.0-only","name":"GNU General Public License v3.0 only","url":null},"downloads":1,"followers":1,"categories":["utility"],"additional_categories":[],"loaders":["fabric"],"versions":["z1FVDMnq"],"icon_url":null,"issues_url":null,"source_url":null,"wiki_url":null,"discord_url":null,"donation_urls":[],"gallery":[],"color":null,"thread_id":"x","monetization_status":"monetized","environment":["client_and_server"]}"#;

    fn good() -> Value {
        version(
            "VVVVVVVV",
            "PPPPPPPP",
            "1.2.3",
            "release",
            "2026-11-01T10:00:00Z",
            file("PPPPPPPP", "VVVVVVVV", "mod-1.2.3+mc.jar", b"jar"),
        )
    }

    fn parsed(value: &Value) -> Result<Version> {
        parse_version(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn real_answers_are_read() {
        let version = parse_version(REAL_VERSION.as_bytes()).unwrap();
        assert_eq!((version.id.as_str(), version.project_id.as_str()), ("z1FVDMnq", "YlKdE5VK"));
        assert_eq!(version.number, "0.4.22+188-26.x");
        assert_eq!(version.channel, Channel::Alpha);
        assert!(version.is_listed());
        assert_eq!(utc(version.published), "2026-10-22 19:11 UTC");
        assert!(version.supports("26.3", &["fabric"]) && !version.supports("26.3", &["quilt"]));
        assert!(!version.supports("1.21.4", &["fabric"]));
        let kinds: Vec<_> = version.dependencies.iter().map(|d| (d.project_id.as_deref(), d.kind)).collect();
        assert_eq!(kinds, [(Some("P1OZGk5p"), DependencyKind::Embedded), (Some("P7dR8mSH"), DependencyKind::Required)]);
        let jar = version.primary_jar().unwrap();
        assert_eq!(jar.filename, "ViaFabric-0.4.22+188-26.x.jar");
        assert_eq!((jar.size, jar.sha512.as_str()), (5_916_758, SHA512));
        assert_eq!(jar.sha1.as_deref(), Some("602797ada2a9bcc4aaecbfdd2f89e4c9397787cc"));
        assert_eq!(jar.url, "https://cdn.modrinth.com/data/YlKdE5VK/versions/z1FVDMnq/ViaFabric-0.4.22%2B188-26.x.jar");
        assert_eq!(version.file_with_sha512(&SHA512.to_uppercase()).unwrap().unwrap(), jar);
        assert!(version.file_with_sha512(&"0".repeat(128)).is_none());

        let project = parse_project(REAL_PROJECT.as_bytes()).unwrap();
        assert_eq!(
            project,
            Project {
                id: "YlKdE5VK".into(),
                slug: "viafabric".into(),
                title: "ViaFabric".into(),
                kind: "mod".into(),
                status: "approved".into(),
                license: "GPL-3.0-only".into(),
            }
        );
        // The same objects inside lists and inside a hash lookup.
        assert_eq!(parse_versions(format!("[{REAL_VERSION},{REAL_VERSION}]").as_bytes()).unwrap().len(), 2);
        assert_eq!(parse_projects(format!("[{REAL_PROJECT}]").as_bytes()).unwrap(), [project]);
        let lookup = parse_hash_lookup(format!("{{\"{SHA512}\":{REAL_VERSION}}}").as_bytes(), &[SHA512.to_string()]);
        assert_eq!(lookup.unwrap()[SHA512].id, "z1FVDMnq");
        assert!(parse_versions(b"[]").unwrap().is_empty());
        assert!(parse_hash_lookup(b"{}", &[SHA512.to_string()]).unwrap().is_empty());
    }

    #[test]
    fn time_stamps_are_read_strictly_and_printed_back() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(parse_timestamp("2026-11-14T00:00:00Z").unwrap(), 1_794_614_400);
        assert_eq!(parse_timestamp("2000-02-29T12:00:00.5Z").unwrap(), 951_825_600);
        assert_eq!(
            parse_timestamp("2026-10-22T19:11:11.566967Z").unwrap(),
            parse_timestamp("2026-10-22T19:11:11+00:00").unwrap()
        );
        assert_eq!(parse_timestamp("2021-01-23T14:29:26.654Z").unwrap() % 60, 26);
        for bad in [
            "",
            "2026-11-14",
            "2026-11-14T00:00:00",       // no zone
            "2026-11-14T00:00:00+01:00", // not UTC
            "2026-11-14T00:00:00z",
            "2026-11-14t00:00:00Z",
            "2026-11-14 00:00:00Z",
            "2026-02-29T00:00:00Z", // not a leap year
            "2026-04-31T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-00-10T00:00:00Z",
            "2026-11-14T24:00:00Z",
            "2026-11-14T00:60:00Z",
            "2026-11-14T00:00:60Z",
            "1969-12-31T23:59:59Z",
            "2026-11-14T00:00:00.Z",
            "2026-11-14T00:00:00.1234567890Z",
            "2026-11-14T00:00:00Zjunk",
            "+026-11-14T00:00:00Z",
            "2026-11-14T00:00:0\u{661}Z",
        ] {
            assert!(matches!(parse_timestamp(bad), Err(Error::Format(_))), "{bad:?}");
        }
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_794_614_400 + 13 * 3600 + 38 * 60 + 52), "2026-11-14 13:38 UTC");
        assert_eq!(utc(951_825_600), "2000-02-29 12:00 UTC");
        // Every day of four centuries survives the round trip through both conversions.
        for day in (0..146_097 * 2).step_by(13) {
            let (year, month, dom) = civil_from_days(day);
            assert_eq!(days_from_civil(year, month, dom), day);
        }
        let now = 1_794_614_400;
        assert_eq!(age(now, now - 90), "1 minute ago");
        assert_eq!(age(now, now - 38 * 3600 - 5), "38 hours ago");
        assert_eq!(age(now, now - 47 * 3600), "47 hours ago");
        assert_eq!(age(now, now - 22 * 86_400), "22 days ago");
        assert!(age(now, now + 7200).contains("2 hours in the future"));
    }

    #[tokio::test]
    async fn the_registrys_clock_is_read_from_its_answers_and_only_ever_makes_now_earlier() {
        assert_eq!(parse_http_date("Sat, 14 Nov 2026 00:00:00 GMT"), Some(1_794_614_400));
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("Tue, 29 Feb 2000 12:00:00 GMT"), Some(951_825_600));
        for bad in [
            "",
            "Sat, 14 Nov 2026 00:00:00 UTC",
            "Sat, 14 Nov 2026 00:00:00 +0000",
            "Saturday, 14-Nov-26 00:00:00 GMT", // the obsolete forms are not accepted
            "Sat Nov 14 00:00:00 2026",
            "Sat, 14 Nov 2026 00:00:00  GMT",
            "Sat, 14 nov 2026 00:00:00 GMT",
            "Sat, 31 Nov 2026 00:00:00 GMT",
            "Sat, 14 Nov 2026 24:00:00 GMT",
            "Sat, 14 Nov 2026 00:00:00 GMT ",
            "Sat,\u{a0}14 Nov 2026 00:00:00 GMT",
            "1794614400",
        ] {
            assert_eq!(parse_http_date(bad), None, "{bad:?}");
        }

        let scripted = Scripted::default();
        let registry = Registry::new(&scripted);
        let local = 1_794_614_400;
        assert_eq!((registry.server_time(), registry.now(local)), (None, local), "nothing heard yet");
        registry.project("nothing-here").await.unwrap();
        assert_eq!(registry.now(local), local, "an answer without a date changes nothing");
        // The registry's clock is behind ours: its time is the one to measure against.
        scripted.clock(Some(local - 7200));
        registry.project("nothing-here").await.unwrap();
        assert_eq!((registry.server_time(), registry.now(local)), (Some(local - 7200), local - 7200));
        // It is ahead of ours: ours stays. Neither clock can make a build look older.
        scripted.clock(Some(local + 7200));
        registry.project("nothing-here").await.unwrap();
        assert_eq!((registry.server_time(), registry.now(local)), (Some(local + 7200), local));
    }

    #[test]
    fn a_file_is_used_only_if_its_name_size_hashes_and_address_all_pass() {
        assert!(parsed(&good()).unwrap().primary_jar().is_ok());
        let refused = |change: &dyn Fn(&mut Value)| {
            let mut value = good();
            change(&mut value["files"][0]);
            match parsed(&value).expect("the list itself is well formed").primary_jar() {
                Err(Error::Policy(message)) => message,
                other => panic!("accepted or wrong error: {other:?}"),
            }
        };
        // Names. The address is made to agree, so that the name alone is what is refused.
        for name in [
            "../evil.jar",
            "a b.jar",
            ".hidden.jar",
            "x/y.jar",
            "évil.jar",
            "fabric-api-0.28.5%2B1.14.jar",
            "",
            "a\u{1b}[31m.jar",
        ] {
            let message = refused(&|f| {
                f["filename"] = json!(name);
                f["url"] = json!(format!("https://cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/{name}"));
            });
            assert!(message.contains("only names made of"), "{name:?}: {message}");
            assert!(!message.contains('\u{1b}'), "a refused name is not echoed raw");
        }
        for name in ["mod.zip", "mod.jar.exe", "mod.JAR", "jar"] {
            assert!(refused(&|f| f["filename"] = json!(name)).contains("not a .jar"), "{name}");
        }
        let long_name = format!("{}.jar", "a".repeat(100_000));
        assert!(refused(&|f| f["filename"] = json!(long_name)).len() < 400, "a long name must not flood the terminal");
        // Sizes.
        for size in [0u64, MAX_MOD_BYTES + 1, u64::MAX] {
            assert!(refused(&|f| f["size"] = json!(size)).contains("bytes"), "{size}");
        }
        // Hashes.
        for hash in ["", "abc", &"g".repeat(128), &"a".repeat(127), &"a".repeat(129)] {
            assert!(refused(&|f| f["hashes"]["sha512"] = json!(hash)).contains("SHA-512"), "{hash:?}");
        }
        assert!(refused(&|f| f["hashes"]["sha1"] = json!("abc")).contains("SHA-1"));
        // Addresses.
        for url in [
            "http://cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "https://evil.example/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com.evil.example/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com:8443/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "https://user@cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "https://CDN.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "https://api.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com/data/QQQQQQQQ/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar", // another project
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/other.jar",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/x/mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/../mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/%2e%2e/mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/a%2Fb/mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions//mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/V V/mod-1.2.3%2Bmc.jar",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar?x=1",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar#frag",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar%00",
            "https://cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2",
            "https://cdn.modrinth.com/data/PPPPPPPP/Versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "//cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/mod-1.2.3%2Bmc.jar",
            "",
        ] {
            assert!(refused(&|f| f["url"] = json!(url)).contains("download address"), "{url}");
        }
        let long_url =
            format!("https://cdn.modrinth.com/data/PPPPPPPP/versions/{}/mod-1.2.3%2Bmc.jar", "v".repeat(2000));
        assert!(refused(&|f| f["url"] = json!(long_url)).len() < 500);
        // Old uploads carry the version *number* in the address, escaped; that is accepted.
        let mut old = good();
        old["files"][0]["url"] = json!("https://cdn.modrinth.com/data/PPPPPPPP/versions/1.2.3%2Bmc/mod-1.2.3%2Bmc.jar");
        assert!(parsed(&old).unwrap().primary_jar().is_ok());
    }

    #[test]
    fn the_mod_file_of_a_build_is_its_primary_plain_file() {
        let extra = |name: &str, primary: bool, kind: Value| {
            let mut f = file("PPPPPPPP", "VVVVVVVV", name, name.as_bytes());
            f["primary"] = json!(primary);
            f["file_type"] = kind;
            f
        };
        let pick = |files: Vec<Value>| {
            let mut value = good();
            value["files"] = json!(files);
            parsed(&value).unwrap().primary_jar().map(|f| f.filename)
        };
        // The primary one wins wherever it stands; typed extras are never the mod.
        assert_eq!(
            pick(vec![
                extra("a-sources.jar", false, json!("sources-jar")),
                extra("b.jar", false, Value::Null),
                extra("c.jar", true, Value::Null)
            ])
            .unwrap(),
            "c.jar"
        );
        // Without a primary: the first plain file.
        assert_eq!(
            pick(vec![extra("a-dev.jar", false, json!("dev-jar")), extra("b.jar", false, Value::Null)]).unwrap(),
            "b.jar"
        );
        // A typed file is not taken even when it is marked primary.
        assert_eq!(
            pick(vec![extra("a-sources.jar", true, json!("sources-jar")), extra("b.jar", false, Value::Null)]).unwrap(),
            "b.jar"
        );
        assert!(matches!(pick(vec![extra("a-sources.jar", true, json!("sources-jar"))]), Err(Error::Policy(_))));
        assert!(matches!(pick(vec![]), Err(Error::Policy(_))));
        // An unusable extra file does not make the build unusable …
        assert_eq!(
            pick(vec![extra("good.jar", true, Value::Null), extra("read me (1).txt", false, Value::Null)]).unwrap(),
            "good.jar"
        );
        // … but an unusable primary does: nothing else is picked in its place.
        assert!(pick(vec![extra("bad name.jar", true, Value::Null), extra("good.jar", false, Value::Null)]).is_err());
    }

    #[test]
    fn malformed_and_oversized_answers_are_refused() {
        let text = |value: &Value| serde_json::to_string(value).unwrap();
        let broken = |change: &dyn Fn(&mut Value)| {
            let mut value = good();
            change(&mut value);
            matches!(parsed(&value), Err(Error::Format(_)))
        };
        assert!(broken(&|v| v["id"] = json!("../../etc")));
        assert!(broken(&|v| v["id"] = json!("short")));
        assert!(broken(&|v| v["id"] = json!("AAAAAAAAAAAA"))); // twelve characters
        assert!(broken(&|v| v["project_id"] = json!("PPPP PPP")));
        assert!(broken(&|v| v["project_id"] = Value::Null));
        assert!(broken(&|v| v["date_published"] = json!("yesterday")));
        assert!(broken(&|v| v["date_published"] = json!(1_794_614_400u64)));
        assert!(broken(&|v| v["files"] = json!("none")));
        assert!(broken(&|v| v["files"][0]["size"] = json!("12")));
        assert!(broken(&|v| v["files"][0]["size"] = json!(-1)));
        assert!(broken(&|v| v["files"][0]["size"] = json!(1.5)));
        assert!(broken(&|v| v["files"][0]["primary"] = json!("yes")));
        assert!(broken(&|v| v["files"][0]["hashes"] = json!({ "sha1": "x" })));
        assert!(broken(&|v| v["game_versions"] = json!(["x".repeat(65)])));
        assert!(broken(&|v| v["loaders"] = json!([1, 2])));
        assert!(broken(&|v| v["dependencies"] = json!([{ "project_id": "bad id!", "dependency_type": "required" }])));
        assert!(broken(&|v| v["dependencies"] = json!([{ "project_id": "PPPPPPPP" }])));
        assert!(broken(&|v| {
            v.as_object_mut().unwrap().remove("version_type");
        }));
        // What is not used may be anything, and what is unknown is not chosen.
        let mut odd = good();
        odd["version_type"] = json!("nightly");
        odd["status"] = json!("archived");
        odd["something_new"] = json!({ "nested": [1, 2, { "deep": true }] });
        odd["dependencies"] = json!([{ "project_id": null, "version_id": null, "file_name": "x\u{7}y.jar", "dependency_type": "mystery" }]);
        let odd = parsed(&odd).unwrap();
        assert_eq!((odd.channel, odd.is_listed()), (Channel::Other, false));
        assert_eq!(odd.dependencies[0].kind, DependencyKind::Other);
        assert!(!odd.dependencies[0].file_name.as_deref().unwrap().contains('\u{7}'));
        let mut unstated = good();
        unstated.as_object_mut().unwrap().remove("status");
        assert!(!parsed(&unstated).unwrap().is_listed(), "a build without a status is not taken");

        // Not JSON, the wrong shape, trailing bytes, a key twice.
        let one = text(&good());
        for bad in
            ["", "null", "[]", "{", "<html>502 Bad Gateway</html>", &format!("{one} trailing"), &format!("{one}{one}")]
        {
            assert!(matches!(parse_version(bad.as_bytes()), Err(Error::Format(_))), "{bad:.40}");
        }
        assert!(parse_version(one.replacen("{", "{\"id\":\"VVVVVVVV\",", 1).as_bytes()).is_err(), "a key given twice");
        assert!(parse_versions(format!("[{one}] x").as_bytes()).is_err());
        assert!(parse_versions(one.as_bytes()).is_err(), "an object where a list belongs");
        // Tens of thousands of brackets in a field nobody reads: skipped without recursion
        // (serde_json keeps its place in a heap buffer), so there is no stack to overflow.
        // Where a value is used, the first bracket too many is a type error.
        let nest = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        let deep = one.replacen("{", &format!("{{\"x\":{},", nest(50_000)), 1);
        assert_eq!(parse_version(deep.as_bytes()).unwrap().id, "VVVVVVVV");
        assert!(parse_version(one.replacen("\"listed\"", &nest(50_000), 1).as_bytes()).is_err());
        assert!(parse_versions(nest(50_000).as_bytes()).is_err());
        // The parser's complaint is cleaned before it is shown.
        let hostile = one.replace("\"12\"", "1").replacen("\"VVVVVVVV\"", "{\"\\u001b[2J\":1}", 1);
        let message = parse_version(hostile.as_bytes()).unwrap_err().to_string();
        assert!(!message.contains('\u{1b}') && message.len() < 400, "{message:?}");

        // Lists have ceilings that hold while reading.
        let many = |n: usize| format!("[{}]", vec![one.as_str(); n].join(","));
        assert_eq!(parse_versions(many(50).as_bytes()).unwrap().len(), 50);
        let error = parse_versions(many(MAX_VERSIONS + 1).as_bytes()).unwrap_err().to_string();
        assert!(error.contains("more than 8192 entries"), "{error}");
        let mut crowded = good();
        crowded["files"] = json!(vec![good()["files"][0].clone(); MAX_FILES + 1]);
        assert!(parsed(&crowded).is_err());
        let mut crowded = good();
        crowded["game_versions"] = json!(vec!["1.21.4"; MAX_TAGS + 1]);
        assert!(parsed(&crowded).is_err());
        let mut crowded = good();
        crowded["dependencies"] = json!(vec![dependency(Some("PPPPPPPP"), None, "optional"); MAX_DEPENDENCIES + 1]);
        assert!(parsed(&crowded).is_err());
        let projects = vec![project("PPPPPPPP", "p", "P"); MAX_IDS + 1];
        assert!(parse_projects(text(&json!(projects)).as_bytes()).is_err());
    }

    #[test]
    fn text_from_the_registry_is_cleaned_before_it_is_shown() {
        let mut hostile = project("PPPPPPPP", "sl\u{1b}[31mug", "  Ti\u{202e}tle\u{7} \u{9b}\n second line  ");
        hostile["license"] = json!({ "id": "MIT\u{1b}]0;x\u{7}" });
        hostile["status"] = json!("appro\u{0}ved");
        let project = parse_project(&serde_json::to_vec(&hostile).unwrap()).unwrap();
        for text in [&project.slug, &project.title, &project.license, &project.status] {
            assert!(!text.chars().any(|c| c.is_control() || c == '\u{202e}'), "{text:?}");
        }
        assert!(project.title.starts_with("Ti") && project.title.contains("second line"));
        // Cut to length, and never empty.
        let mut long = self::testing::project("PPPPPPPP", "slug", &"T".repeat(5000));
        assert!(parse_project(&serde_json::to_vec(&long).unwrap()).unwrap().title.chars().count() < 100);
        long["title"] = json!("   ");
        long.as_object_mut().unwrap().remove("license");
        let bare = parse_project(&serde_json::to_vec(&long).unwrap()).unwrap();
        assert_eq!((bare.title.as_str(), bare.license.as_str()), ("slug", "not stated"));
        let mut odd = good();
        odd["version_number"] = json!("1.0\u{1b}[0m\r\n");
        assert!(!parsed(&odd).unwrap().number.chars().any(char::is_control));
        // An id is not "cleaned": it is right or the answer is refused.
        let mut bad = self::testing::project("PPPP\u{7}PPP", "slug", "Title");
        assert!(matches!(parse_project(&serde_json::to_vec(&bad).unwrap()), Err(Error::Format(_))));
        bad["id"] = json!("PPPPPPPP");
        bad["project_type"] = json!(7);
        assert!(parse_project(&serde_json::to_vec(&bad).unwrap()).is_err());
    }

    #[test]
    fn a_hash_lookup_is_accepted_only_for_what_was_asked() {
        let content = b"the jar";
        let hash = sha512_hex(content);
        let one = version(
            "VVVVVVVV",
            "PPPPPPPP",
            "1.0",
            "release",
            "2026-11-01T10:00:00Z",
            file("PPPPPPPP", "VVVVVVVV", "a.jar", content),
        );
        let answer = |key: &str| serde_json::to_vec(&json!({ key: one })).unwrap();
        let asked = vec![hash.clone()];
        let known = parse_hash_lookup(&answer(&hash), &asked).unwrap();
        assert_eq!(known[&hash].file_name_for(&hash).as_deref(), Some("a.jar"));
        // Upper case in the answer is the same hash.
        assert!(parse_hash_lookup(&answer(&hash.to_uppercase()), &asked).unwrap().contains_key(&hash));
        // A hash nobody asked about, and a version that does not have the hash it is named for.
        let other = sha512_hex(b"something else");
        assert!(matches!(parse_hash_lookup(&answer(&other), &asked), Err(Error::Format(_))));
        assert!(matches!(parse_hash_lookup(&answer(&other), std::slice::from_ref(&other)), Err(Error::Format(_))));
        assert!(matches!(parse_hash_lookup(&answer(&hash), &[]), Err(Error::Format(_))));
        assert!(parse_hash_lookup(b"[]", &asked).is_err());
        // One hash twice (here: in two spellings) is not an answer to the question either.
        let twice = format!(
            "{{\"{hash}\":{one},\"{}\":{one}}}",
            hash.to_uppercase(),
            one = serde_json::to_string(&one).unwrap()
        );
        let error = parse_hash_lookup(twice.as_bytes(), &asked).unwrap_err().to_string();
        assert!(error.contains("two entries for one hash"), "{error}");
        assert!(
            parse_hash_lookup(format!("{} x", String::from_utf8(answer(&hash)).unwrap()).as_bytes(), &asked).is_err()
        );
        // The question bounds the answer while it is read: a flood of entries nobody asked
        // for is given up at its first key, not parsed to the end and counted afterwards.
        let one = serde_json::to_string(&one).unwrap();
        let flood: Vec<String> = (0..2000u32).map(|n| format!("\"{}\":{one}", sha512_hex(&n.to_le_bytes()))).collect();
        let error = parse_hash_lookup(format!("{{{}}}", flood.join(",")).as_bytes(), &asked).unwrap_err().to_string();
        assert!(error.contains("a hash that was not asked about at line 1 column 13"), "{error}");
    }

    #[test]
    fn names_and_ids_are_restricted_before_they_reach_a_url() {
        for good in ["sodium", "simple-voice-chat", "fabric_api", "YlKdE5VK", "a.b", "abc"] {
            assert!(check_key(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "ab",
            "..",
            "a/b",
            "../x",
            "a..b",
            ".abc",
            "a b",
            "a?b=c",
            "a#b",
            "a%2fb",
            "mod!",
            "ünï",
            &"x".repeat(65),
        ] {
            assert!(matches!(check_key(bad), Err(Error::Usage(_))), "{bad:?}");
        }
        for good in ["YlKdE5VK", "P7dR8mSH", "abcdefgh123"] {
            assert!(check_id(good).is_ok());
        }
        for bad in ["", "short", "abcdefghijkl", "abc-efgh", "abc/efgh", "abcdefg\u{e9}"] {
            assert!(check_id(bad).is_err(), "{bad:?}");
        }
        assert_eq!(decode_segment("ViaFabric-0.4.22%2B188-26.x.jar").unwrap(), b"ViaFabric-0.4.22+188-26.x.jar");
        assert_eq!(decode_segment("a+b~c_d").unwrap(), b"a+b~c_d");
        for bad in
            ["", ".", "..", "%2e%2E", "a%2Fb", "a%5Cb", "a%00", "a%0a", "a%", "a%2", "a%zz", "a b", "a/b", "a?b", "é"]
        {
            assert!(decode_segment(bad).is_none(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn the_routes_ask_what_the_documentation_says_and_bound_what_they_take() {
        let scripted = Scripted::default();
        scripted
            .json("GET", "/v2/project/viafabric", &project("YlKdE5VK", "viafabric", "ViaFabric"))
            .on("GET", "/v2/project/broken", 500, "oops")
            .on("GET", "/v2/project/retired", 410, "")
            .json("GET", "/v2/project/YlKdE5VK/version", &json!([good()]))
            .json("GET", "/v2/version/VVVVVVVV", &good())
            .json("GET", "/v2/version/WWWWWWWW", &good())
            .json("GET", "/v2/projects", &json!([project("YlKdE5VK", "viafabric", "ViaFabric")]));
        let registry = Registry::new(&scripted);

        assert_eq!(registry.project("viafabric").await.unwrap().unwrap().id, "YlKdE5VK");
        assert!(registry.project("nothing-here").await.unwrap().is_none(), "404 means: no such project");
        assert!(matches!(registry.project("broken").await, Err(Error::Net(m)) if m.contains("HTTP 500")));
        assert!(matches!(registry.project("retired").await, Err(Error::Net(m)) if m.contains("retired")));
        assert!(matches!(registry.project("../v2/user").await, Err(Error::Usage(_))));

        // A list of builds must be about the project that was asked for.
        let error = registry.versions("YlKdE5VK", "1.21.4", &["quilt", "fabric"]).await.unwrap_err();
        assert!(matches!(error, Error::Format(m) if m.contains("another project")));
        assert!(registry.versions("../x", "1.21.4", &["fabric"]).await.is_err());
        assert_eq!(registry.version("VVVVVVVV").await.unwrap().unwrap().number, "1.2.3");
        assert!(matches!(registry.version("WWWWWWWW").await, Err(Error::Format(_))), "another version than asked for");
        assert!(registry.version("XXXXXXXX").await.unwrap().is_none());
        assert_eq!(registry.projects(&["YlKdE5VK".to_string()]).await.unwrap().len(), 1);
        assert!(registry.projects(&[]).await.unwrap().is_empty());
        assert!(registry.projects(&vec!["YlKdE5VK".to_string(); MAX_IDS + 1]).await.is_err());
        assert!(registry.projects(&["bad id".to_string()]).await.is_err());
        // A list of projects must be about the ids that were asked for, each at most once.
        let error = registry.projects(&["P7dR8mSH".to_string()]).await.unwrap_err();
        assert!(matches!(error, Error::Format(m) if m.contains("not asked for")), "another project than asked for");
        let both = ["YlKdE5VK".to_string(), "P7dR8mSH".to_string()];
        assert_eq!(registry.projects(&both).await.unwrap().len(), 1, "fewer than asked for is an answer");
        let viafabric = project("YlKdE5VK", "viafabric", "ViaFabric");
        scripted.json("GET", "/v2/projects", &json!([viafabric, viafabric]));
        assert!(matches!(registry.projects(&both).await, Err(Error::Format(_))), "one project twice");
        scripted.json("GET", "/v2/projects", &json!([viafabric]));

        let seen = scripted.seen();
        assert!(seen.contains(&"GET api.modrinth.com/v2/project/viafabric".to_string()), "{seen:?}");
        assert!(
            seen.contains(
                &"GET api.modrinth.com/v2/project/YlKdE5VK/version?loaders=%5B%22quilt%22%2C%22fabric%22%5D&game_versions=%5B%221.21.4%22%5D&include_changelog=false"
                    .to_string()
            ),
            "{seen:?}"
        );
        assert!(seen.contains(&"GET api.modrinth.com/v2/projects?ids=%5B%22YlKdE5VK%22%5D".to_string()), "{seen:?}");
        assert!(!seen.iter().any(|request| request.contains("..") || request.contains("bad id")), "{seen:?}");
        assert_eq!(registry.requests(), seen.len(), "every request that left was counted");

        // Hash lookups: one request per 256 hashes, each naming the algorithm.
        let hashes: Vec<String> = (0..MAX_HASHES + 1).map(|n| sha512_hex(&n.to_le_bytes())).collect();
        scripted.json("POST", "/v2/version_files", &json!({}));
        assert!(registry.by_hash(&hashes).await.unwrap().is_empty());
        assert_eq!(scripted.count("POST api.modrinth.com/v2/version_files"), 2);
        assert!(
            scripted
                .seen()
                .last()
                .unwrap()
                .ends_with(&format!("{{\"algorithm\":\"sha512\",\"hashes\":[\"{}\"]}}", hashes[MAX_HASHES]))
        );
        assert!(registry.by_hash(&["not a hash".to_string()]).await.is_err());
    }

    #[tokio::test]
    async fn one_command_makes_a_bounded_number_of_requests() {
        let scripted = Scripted::default();
        let registry = Registry::new(&scripted);
        for _ in 0..MAX_REQUESTS {
            assert!(registry.project("nothing-here").await.unwrap().is_none());
        }
        assert!(matches!(registry.project("nothing-here").await, Err(Error::Policy(m)) if m.contains("more than 96")));
        assert_eq!(scripted.seen().len(), MAX_REQUESTS, "the refused request never left");
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bastion-modrinth-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_download_is_kept_only_if_size_and_hashes_match() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("download");
        let content: Vec<u8> = (0..50_000u32).flat_map(|n| n.to_le_bytes()).collect();
        let listed = parsed(&version(
            "VVVVVVVV",
            "PPPPPPPP",
            "1.0",
            "release",
            "2026-11-01T10:00:00Z",
            file("PPPPPPPP", "VVVVVVVV", "a+b.jar", &content),
        ))
        .unwrap()
        .primary_jar()
        .unwrap();
        let path = file_path("PPPPPPPP", "VVVVVVVV", "a+b.jar");
        let dest = dir.join("a+b.jar");
        let attempt = |body: Vec<u8>, status: u16| {
            let (listed, dest, path) = (listed.clone(), dest.clone(), path.clone());
            async move {
                let scripted = Scripted::default();
                scripted.on("GET", &path, status, body);
                let outcome = Registry::new(&scripted).download(&listed, &dest).await;
                (outcome, scripted.seen())
            }
        };

        // The right bytes: kept, private, and described by the hash the manifest will hold.
        let (outcome, seen) = attempt(content.clone(), 200).await;
        let downloaded = outcome.unwrap();
        assert_eq!(downloaded.stored, None, "the file server stated no date, and none is made up");
        assert_eq!(seen, ["GET cdn.modrinth.com/data/PPPPPPPP/versions/VVVVVVVV/a%2Bb.jar"]);
        assert_eq!(downloaded.size, content.len() as u64);
        assert_eq!(downloaded.sha256, util::hex_encode(&util::hash_bytes(util::HashAlg::Sha256, &content)));
        assert_eq!(fs::read(&dest).unwrap(), content);
        assert_eq!(fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o600);

        // A file that is already there is neither written over nor removed.
        let (outcome, _) = attempt(b"other".to_vec(), 200).await;
        assert!(matches!(outcome, Err(Error::Io { .. })), "{outcome:?}");
        assert_eq!(fs::read(&dest).unwrap(), content);
        fs::remove_file(&dest).unwrap();

        // One byte different, same length.
        let mut flipped = content.clone();
        flipped[40_000] ^= 1;
        let (outcome, seen) = attempt(flipped, 200).await;
        assert!(matches!(&outcome, Err(Error::Integrity(m)) if m.contains("SHA-512")), "{outcome:?}");
        assert_eq!(seen.len(), 1, "wrong bytes are not asked for a second time");
        assert!(!dest.exists());
        // Too short, too long (the excess is refused when it arrives, not read to the end).
        let (outcome, _) = attempt(content[..content.len() - 1].to_vec(), 200).await;
        assert!(matches!(&outcome, Err(Error::Integrity(m)) if m.contains("received")), "{outcome:?}");
        let (outcome, _) = attempt([content.clone(), vec![0; 100_000]].concat(), 200).await;
        assert!(matches!(&outcome, Err(Error::Integrity(m)) if m.contains("more than")), "{outcome:?}");
        let (outcome, _) = attempt(Vec::new(), 200).await;
        assert!(matches!(outcome, Err(Error::Integrity(_))));
        assert!(!dest.exists());
        // The file server says no: asked twice, then given up, nothing left behind.
        for status in [404, 403, 500, 302] {
            let (outcome, seen) = attempt(b"<html>".to_vec(), status).await;
            assert!(matches!(&outcome, Err(Error::Net(m)) if m.contains(&status.to_string())), "{outcome:?}");
            assert_eq!(seen.len(), 2);
            assert!(!dest.exists());
        }
        // The right SHA-512 does not excuse a wrong SHA-1: the registry contradicts itself.
        let mut confused = listed.clone();
        confused.sha1 = Some("0".repeat(40));
        let scripted = Scripted::default();
        scripted.on("GET", &path, 200, content.clone());
        let outcome = Registry::new(&scripted).download(&confused, &dest).await;
        assert!(matches!(&outcome, Err(Error::Integrity(m)) if m.contains("SHA-1")), "{outcome:?}");
        assert!(!dest.exists());
        // Without a SHA-1 the SHA-512 alone decides. What the file server says about when
        // it stored the file is handed on as it is: what to make of it is the caller's rule.
        confused.sha1 = None;
        scripted.stored(&path, Some(1_794_000_000));
        let dated = Registry::new(&scripted).download(&confused, &dest).await.unwrap();
        assert_eq!(dated.stored, Some(1_794_000_000));
        // A destination that is a dangling symlink is not followed.
        fs::remove_file(&dest).unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere.jar"), &dest).unwrap();
        assert!(Registry::new(&scripted).download(&listed, &dest).await.is_err());
        assert!(!dir.join("elsewhere.jar").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_registry_hosts_are_reachable_only_through_the_client_built_for_it() {
        let cfg = Config::default();
        let live = Live::new(&cfg).unwrap();
        for host in HOSTS {
            assert!(live.http.check_host(host).is_ok(), "{host}");
            assert!(!crate::net::http::BUILTIN_HOSTS.contains(&host), "{host} must not be a built-in host");
            assert!(crate::launch::http_client(&cfg).unwrap().check_host(host).is_err(), "a launch can reach {host}");
        }
        assert!(live.http.check_host("staging-api.modrinth.com").is_err());
        assert!(live.http.check_host("modrinth.com").is_err());
        assert!(USER_AGENT.starts_with("bastion-mc/") && USER_AGENT.is_ascii());
    }

    #[tokio::test]
    async fn the_live_client_refuses_everything_but_its_two_hosts_before_connecting() {
        // Nothing listens on the dial address: every refusal here happens before a connection.
        let cfg = Config::default();
        let roots = tls::load_system_roots().unwrap();
        let config = tls::client_config(roots, tls::PinSet::from_config(&cfg.tls.pins, true).unwrap()).unwrap();
        let hosts: Vec<String> = HOSTS.iter().map(|h| h.to_string()).collect();
        let live =
            Live::with_client(HttpClient::new(config, &hosts).with_dial_override("127.0.0.1:1".parse().unwrap()));
        for url in [
            "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json", // allowlisted for the launcher, not for this
            "https://evil.example/x",
            "http://api.modrinth.com/v2/project/x",
            "https://api.modrinth.com:8443/v2/project/x",
            "https://user@cdn.modrinth.com/data/x",
        ] {
            let mut sink = Vec::new();
            let call = Call { method: Method::GET, url, body: None, limit: 10, deadline: Duration::from_secs(60) };
            assert!(matches!(live.call(call, &mut sink).await, Err(Error::Policy(_))), "{url}");
        }
        // An allowed host gets as far as the (refused) connection.
        let mut sink = Vec::new();
        let call = Call {
            method: Method::GET,
            url: "https://api.modrinth.com/v2/project/x",
            body: None,
            limit: 10,
            deadline: Duration::from_secs(60),
        };
        assert!(matches!(live.call(call, &mut sink).await, Err(Error::Net(_))));
    }

    #[test]
    fn the_harness_accepts_the_real_answers() {
        for input in [
            REAL_VERSION.to_string(),
            REAL_PROJECT.to_string(),
            format!("[{REAL_VERSION}]"),
            format!("{{\"{SHA512}\":{REAL_VERSION}}}"),
        ] {
            fuzz(input.as_bytes());
        }
        fuzz(b"2026-10-22T19:11:11.566967Z");
        fuzz(b"ViaFabric-0.4.22%2B188-26.x.jar");
        fuzz(b"");
    }
}
