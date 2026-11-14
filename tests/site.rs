//! The project website (`site/`), as far as the launcher has a say in it.
//!
//! `site/build.py --check` builds the site from README.md, `docs/` and `site/src/` and
//! verifies what it wrote: every internal link and fragment, the Content-Security-Policy
//! on every page, nothing loaded from another origin, the commands on the hand-written
//! pages against the help text, two runs byte-identical. This test runs it both ways it is
//! used (a local preview, and with the two addresses the deployment passes), runs the
//! generator's own unit tests, and then holds the generator to the compiled launcher in
//! the two places where it reads *source text*:
//!
//! * the emergency page links to `bastion_mc_proto::runbook::LINKS`, exactly: the list
//!   the launcher and the front end open links from, not a regular expression's reading
//!   of `runbook.rs`;
//! * the help text the pages' commands are checked against is what `bastion-mc --help`
//!   prints.
//!
//! Requirements: python3 (standard library only). Nothing is fetched and no window is
//! opened; the two sites are written to `target/tmp/site-preview` and `site-deployed`.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
};

use bastion_mc_proto::runbook;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");
const PYTHON: &str = "/usr/bin/python3";

/// Stand-ins for the two addresses that no committed file contains (the deployment
/// workflow passes the real ones).
const REPO_URL: &str = "https://code.example.org/owner/project";
const SITE_URL: &str = "https://owner.example.org/project/";

/// What the generator leaves in a source it did not fill in, and what it writes for an
/// address it was not given.
const TOKEN: &str = "{{";
const PREVIEW_ADDRESSES: [&str; 2] = ["repository.invalid", "site.invalid"];

fn repository() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// python3 in the repository: isolated from the user's Python settings (`-I`), leaving no
/// bytecode in the tree (`-B`).
fn python(args: &[&str]) -> Output {
    Command::new(PYTHON)
        .args(["-I", "-B"])
        .args(args)
        .current_dir(repository())
        .output()
        .unwrap_or_else(|e| panic!("the website tests need {PYTHON}: {e}"))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Build the site with `--check` into `target/tmp/NAME`; the build and every check must pass.
fn build(name: &str, addresses: &[&str]) -> PathBuf {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let mut args = vec!["site/build.py", "--check", "--out", out.to_str().expect("a UTF-8 path")];
    args.extend_from_slice(addresses);
    let run = python(&args);
    let (report, errors) = (text(&run.stdout), text(&run.stderr));
    assert!(run.status.success(), "site/build.py --check {addresses:?} failed:\n{report}{errors}");
    assert!(report.contains("check passed: ") && report.contains("two runs byte-identical"), "{report}{errors}");
    assert!(!errors.contains("error:") && !errors.contains("check failed:"), "{errors}");
    out
}

/// The site as `python3 site/build.py` writes it without options. (Built once per run.)
fn preview() -> &'static Path {
    static OUT: OnceLock<PathBuf> = OnceLock::new();
    OUT.get_or_init(|| build("site-preview", &[]))
}

/// The site as the deployment builds it: both addresses given.
fn deployed() -> &'static Path {
    static OUT: OnceLock<PathBuf> = OnceLock::new();
    OUT.get_or_init(|| build("site-deployed", &["--repo-url", REPO_URL, "--site-url", SITE_URL]))
}

/// Every file below `dir`: (path relative to `dir`, content), in a stable order.
fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, base: &Path, found: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, base, found);
            } else {
                let name = path.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                found.push((name, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found
}

/// The files of a generated site that are text, decoded.
fn text_files(dir: &Path) -> Vec<(String, String)> {
    files(dir)
        .into_iter()
        .filter(|(name, _)| [".html", ".css", ".svg", ".xml", ".txt"].iter().any(|suffix| name.ends_with(suffix)))
        .map(|(name, content)| {
            let content = String::from_utf8(content).unwrap_or_else(|_| panic!("{name} is not UTF-8"));
            (name, content)
        })
        .collect()
}

/// HTML's escaping of text and attribute values, as the generator writes it.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#x27;")
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#x27;", "'").replace("&amp;", "&")
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let value = &tag[start..];
    Some(unescape(&value[..value.find('"')?]))
}

struct Link {
    /// Where its `<a` stands in the page.
    at: usize,
    href: String,
    rel: String,
}

impl Link {
    /// Does it lead away from the site? Inside the site every link is relative.
    fn outbound(&self) -> bool {
        let first = self.href.split(['/', '?', '#']).next().unwrap_or_default();
        first.contains(':') || self.href.starts_with('/')
    }
}

/// Every `<a …>` of a generated page, in order.
fn links(page: &str) -> Vec<Link> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = page[from..].find("<a ") {
        let at = from + offset;
        let end = at + page[at..].find('>').expect("an <a> tag that never ends");
        let tag = &page[at..end];
        let href = attribute(tag, "href").unwrap_or_else(|| panic!("no href in {tag}"));
        found.push(Link { at, href, rel: attribute(tag, "rel").unwrap_or_default() });
        from = end;
    }
    found
}

#[test]
fn the_site_builds_and_passes_its_checks_as_a_preview_and_as_deployed() {
    for out in [preview(), deployed()] {
        let names: BTreeSet<String> = files(out).into_iter().map(|(name, _)| name).collect();
        for page in [
            "index.html",
            "getting-started.html",
            "security.html",
            "manual.html",
            "threat-model.html",
            "token-injection.html",
            "dedicated-account.html",
            "mods.html",
            "companion.html",
            "phone.html",
            "bedrock.html",
            "emergency.html",
            "404.html",
            "sitemap.xml",
            "robots.txt",
        ] {
            assert!(names.contains(page), "{page} is missing from {}", out.display());
        }
        // No script anywhere: there is no file a page could load one from.
        assert!(names.iter().all(|name| !name.ends_with(".js")), "{names:?}");
    }
}

#[test]
fn the_emergency_page_links_to_exactly_the_launchers_allowlist() {
    let allowed: BTreeSet<&str> = runbook::LINKS.iter().map(|(address, _)| *address).collect();
    assert_eq!(allowed.len(), runbook::LINKS.len(), "runbook::LINKS lists an address twice");

    for out in [preview(), deployed()] {
        let page = std::fs::read_to_string(out.join("emergency.html")).unwrap();
        let all = links(&page);
        let outbound: Vec<&Link> = all.iter().filter(|link| link.outbound()).collect();
        let on_page: BTreeSet<&str> = outbound.iter().map(|link| link.href.as_str()).collect();
        assert_eq!(on_page, allowed, "the outbound links of emergency.html and runbook::LINKS differ");

        for link in &outbound {
            assert!(runbook::is_runbook_link(&link.href), "{}", link.href);
            assert_eq!(link.rel, "noopener noreferrer", "{}", link.href);
        }
        // Every other link stays on the site: a relative address of a page that was written.
        for link in all.iter().filter(|link| !link.outbound()) {
            let file = link.href.split(['#', '?']).next().unwrap_or_default();
            assert!(file.is_empty() || out.join(file).is_file(), "{} leads nowhere", link.href);
        }

        // Each address is shown as what the launcher says it is.
        for (address, what) in runbook::LINKS {
            let start = page
                .find(&format!("href=\"{}\"", escape(address)))
                .unwrap_or_else(|| panic!("{address} is not linked"));
            let target = &page[start..start + page[start..].find("</a>").expect("</a>")];
            assert!(target.contains(&escape(what)), "{address} is not described as {what:?}: {target}");
        }

        // The warning to use another device stands before the first address (the launcher's
        // own test says the same of the runbook's text).
        let warning = page.find("from a different device").expect("the clean-device warning");
        let first = outbound.iter().map(|link| link.at).min().expect("an outbound link");
        assert!(warning < first, "an address comes before the warning to use another device");
    }
}

#[test]
fn no_generated_file_contains_an_unsubstituted_token() {
    for out in [preview(), deployed()] {
        let all = text_files(out);
        assert!(all.len() >= 12, "only {} text files in {}", all.len(), out.display());
        for (name, content) in &all {
            assert!(!content.contains(TOKEN), "{name} in {} contains an unsubstituted token", out.display());
        }
    }

    // The deployed site carries the addresses it was given and nothing of the preview's.
    let all = text_files(deployed());
    for (name, content) in &all {
        for preview in PREVIEW_ADDRESSES {
            assert!(!content.contains(preview), "{name} of the deployed site still contains {preview}");
        }
    }
    assert!(all.iter().any(|(_, content)| content.contains(REPO_URL)), "no page links to the repository");
    let file = |name: &str| &all.iter().find(|(found, _)| found == name).unwrap_or_else(|| panic!("no {name}")).1;
    assert!(file("index.html").contains(&format!("<link rel=\"canonical\" href=\"{SITE_URL}\">")));
    assert!(file("sitemap.xml").contains(&format!("<loc>{SITE_URL}</loc>")));
    assert!(file("robots.txt").contains(&format!("Sitemap: {SITE_URL}sitemap.xml\n")));
    // The error page is served for any missing address, at any depth, so nothing on it is
    // relative. Every other page stays relative, so that the site also works from a disk.
    let not_found = file("404.html");
    for link in links(not_found).iter().filter(|link| !link.href.starts_with('#')) {
        assert!(link.href.starts_with("https://"), "404.html: {} is relative", link.href);
    }
    assert!(not_found.contains(&format!("<a href=\"{SITE_URL}getting-started.html\"")), "404.html has no navigation");
    assert!(!not_found.contains("=\"assets/"), "404.html loads something by a relative address");
    for link in links(file("index.html")) {
        assert!(!link.href.starts_with(SITE_URL), "index.html: {} is not relative", link.href);
    }
}

#[test]
fn the_generators_unit_tests_pass() {
    let run = python(&["-m", "unittest", "discover", "-v", "-s", "site", "-p", "test_*.py"]);
    let report = text(&run.stderr); // (unittest reports on standard error)
    assert!(run.status.success(), "the Python unit tests failed:\n{report}");
    assert!(report.trim_end().ends_with("OK"), "{report}");
    for module in ["test_md.", "test_build."] {
        assert!(report.contains(module), "no test of {module}py ran:\n{report}");
    }
}

#[test]
fn the_generator_reads_the_help_text_the_launcher_prints() {
    let read = python(&[
        "-c",
        "import sys; sys.path.insert(0, 'site'); import build; \
         sys.stdout.buffer.write(build.HelpText(build.Path('.')).text.encode('utf-8'))",
    ]);
    assert!(read.status.success(), "{}", text(&read.stderr));
    let printed = Command::new(LAUNCHER).arg("--help").output().expect("bastion-mc --help");
    assert!(printed.status.success(), "{}", text(&printed.stderr));
    let (read, printed) = (text(&read.stdout), text(&printed.stdout));
    assert!(read.contains("USAGE: bastion-mc"), "{read}");
    assert_eq!(
        read,
        printed.trim_end_matches('\n'),
        "site/build.py reads another help text than `bastion-mc --help` prints"
    );
}
