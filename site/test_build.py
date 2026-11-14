#!/usr/bin/env python3
"""Unit tests for site/build.py, the generator of the website.

    python3 -m unittest discover -s site -p "test_*.py"

Every test builds a small site of its own in a temporary directory: nine short documents,
a help text, a list of emergency links, a page shell, four pages, a style sheet, a mark and
one picture (`FIXTURE` below). That site passes every check. A test then changes one thing
and states exactly what must happen: which error stops the build, or which problems
`--check` reports, and no others. So the tests do not depend on the real pages, which other
people write; the real site is checked by `python3 site/build.py --check` and by
tests/site.rs.

The last group reads what the generator takes from the launcher's own code and documents
(the help text, the emergency links, the documents as they are), because a
reader that only understands a made-up example proves little.

Only the Python standard library is used. Nothing is written outside temporary directories.
"""

from __future__ import annotations

import contextlib
import io
import os
import re
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import build  # noqa: E402  (the generator lives next to this file)
import md  # noqa: E402

REAL_ROOT = HERE.parent

#: The addresses of a deployable build in these tests.
REPO = "https://code.example.org/owner/project"
SITE = "https://owner.example.org/project/"

# --------------------------------------------------------------------------------------
# The small site
# --------------------------------------------------------------------------------------


def png(width=4, height=3, extra=(), after=b"", header=None, end=b""):
    """A complete PNG picture; `extra` chunks go between the header and the image data.
    `header` and `end` replace the content of IHDR and of IEND (for pictures that are wrong)."""

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = b"".join(b"\x00" + b"\x10\x20\x30" * width for _ in range(height))
    if header is None:
        header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    parts = [b"\x89PNG\r\n\x1a\n", chunk(b"IHDR", header)]
    parts += [chunk(kind, data) for kind, data in extra]
    parts += [chunk(b"IDAT", zlib.compress(rows)), chunk(b"IEND", end)]
    return b"".join(parts) + after


README = """\
# bastion-mc

A launcher under test. Read the [threat model](docs/THREAT_MODEL.md), its section
[not defended](docs/THREAT_MODEL.md#5-not-defended), the [runbook](docs/SCRAM_RUNBOOK.md),
the [licence](LICENSE), the [sources](src/) and the [status](#status) below.

## Status

| | |
|---|---|
| Sandbox | **Tested** on one machine. |
| Sign-in | Implemented. |

Sign-in is implemented but *not yet run* against the
`live` services.

## Requirements

Built and tested on Arch Linux.

```sh
cargo build --release              # builds bastion-mc and its front end
target/release/bastion-mc doctor
```

## Playing

Start with `bastion-mc launch <id> --demo`.

### The display

A bare address, https://example.org/display, and `bastion-mc status --account NAME`.
"""

THREAT_MODEL = """\
# Threat model

Out of scope: a kernel that is already compromised.

## 4. Defences

| # | Attack | Defence |
|---|---|---|
| J1 | reads `/proc` | hidden |
| J2 | x32 calls | refused |

## 5. Not defended

1. **Kernel bugs.** A kernel bug defeats every layer.
2. **You.** See the [status](../README.md#status) and row [J1](#row-j1).

Not exercised: the live sign-in.
"""

TOKEN_INJECTION = """\
# Token path

## How it gets there

Never on a command line.
"""

DEDICATED_ACCOUNT = """\
# A dedicated account

## Option 1

A second UNIX account.
"""

MODS = """\
# Mods

## Signed profiles

A manifest you sign; see the [threat model](THREAT_MODEL.md).
"""

COMPANION = """\
# The companion mod

## What it does

It brings [the phone](PHONE.md#the-apps).
"""

PHONE = """\
# The in-game phone

## The apps

Part of [the companion mod](COMPANION.md).
"""

BEDROCK = """\
# Minecraft: Bedrock Edition

## Signing in, every session

Outside the game, unlike [the phone's apps](PHONE.md#the-apps); see the [threat model](THREAT_MODEL.md).
"""

RUNBOOK = """\
# If you think the account is compromised

Two phases. The first is local.

## Phase 1 — this machine

```sh
bastion-mc scram
```

## Read this first: which device are you on?

Do the account steps **from a different device** if this machine may be compromised.

## Phase 2 — the account

1. **Change the password.**
   https://account.example.com/password
2. **Sign out everywhere**: https://account.example.com/sessions?all=1&now=1
"""

RUNBOOK_RS = """\
//! The emergency runbook and the only links that may be opened from it.

/// Every address the runbook refers to: (address, what it is).
pub const LINKS: &[(&str, &str)] = &[
    ("https://account.example.com/password", "Account: change the password"),
    (
        "https://account.example.com/sessions?all=1&now=1",
        "Account: sign out \\"everywhere\\" & more",
    ),
];

pub fn is_runbook_link(url: &str) -> bool {
    LINKS.iter().any(|(link, _)| *link == url)
}
"""

CLI_RS = """\
//! Command line.

const HELP: &str = "\\
bastion-mc — a launcher under test

USAGE: bastion-mc [--events] <command> [options]

  status [--account NAME]             sign-in state and instances at a glance
  doctor                              check kernel, sandbox and trust setup (starts a test sandbox)
  config path | example               show the config file location / print a commented example
  account default [NAME]              show or set the account used when --account is not given

  skin show   [--account NAME]        the account's skins and capes
  skin set <file.png> [--slim] [--account NAME]   upload a skin (metadata is stripped first)
  cape set <alias|none> [--account NAME]          wear one of the account's capes, or none

  scram [--yes]                       EMERGENCY: remove every stored token (no network)
  wiki scram [--links]                the built-in recovery runbook
  instance new <id> --minecraft <version>     create a vanilla instance
  instance list
  launch  <id> [--account NAME] [--join HOST[:PORT]] [--demo] [--quiet]
  profile lock <id>                   record the current mods/ directory in the manifest (and say which
                                      sensitive Java APIs each updated mod newly refers to)

  --help, --version
  --events    (first argument) report progress as JSON lines on standard output

--demo starts the client in demo mode with no account.";

fn usage() {}
"""

TEMPLATE = """\
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="{{CSP}}">
<meta name="referrer" content="no-referrer">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{TITLE}}</title>
<meta name="description" content="{{DESCRIPTION}}">
{{HEAD}}
<link rel="icon" href="{{ROOT}}assets/mark.svg" type="image/svg+xml">
<link rel="stylesheet" href="{{ROOT}}assets/site.css">
</head>
<body class="{{BODY_CLASS}}">
<a class="skip" href="#site-content">Skip to the content</a>
<header class="site-header">
<nav class="site-nav" aria-label="Site">
{{NAVIGATION}}
</nav>
</header>
<main id="site-content">
{{CONTENT}}
</main>
{{FOOTER}}
</body>
</html>
"""

FOOTER = """\
<footer class="site-footer">
<p>Built from its <a href="{{REPO_URL}}" rel="noopener noreferrer">source code</a>. Back to the <a href="{{ROOT}}index.html">overview</a>.</p>
<p class="legal">Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.</p>
<p class="legal">The host of this site necessarily sees your IP address.</p>
</footer>
"""

FOOTER_MINIMAL = """\
<footer class="site-footer">
<p>Back to the <a href="{{ROOT}}index.html">overview</a>.</p>
<p class="legal">Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.</p>
<p class="legal">The host of this site necessarily sees your IP address.</p>
</footer>
"""

DOCUMENT_SHELL = """\
<p class="label">{{LABEL}}</p>
<h1 id="{{TITLE_ID}}">{{TITLE}}</h1>
<p>Generated from <a href="{{SOURCE}}" rel="noopener noreferrer"><code>{{PATH}}</code></a> (<a href="{{HISTORY}}" rel="noopener noreferrer">its history</a>).</p>
<nav class="toc" aria-label="On this page">
{{TOC}}
</nav>
<article class="doc">
{{BODY}}
</article>
"""

EMERGENCY_SHELL = """\
<h1 id="{{TITLE_ID}}">{{TITLE}}</h1>
<div class="callout danger">
{{WARNING}}
</div>
<article class="doc">
{{LEAD}}
{{BODY}}
</article>
<p>Generated from <code>{{PATH}}</code>; {{COUNT}} addresses.</p>
"""

INDEX = """\
title: Overview | bastion-mc
description: What the launcher under test is.
---
<section class="hero">
<h1>bastion-mc</h1>
<p class="lede">A launcher. Its <a href="{{REPO_URL}}" rel="noopener noreferrer">source</a>, and <a href="getting-started.html#build">how to build it</a>.</p>
</section>
<section class="section" id="not-defended">
<h2>What it does not defend</h2>
{{SECTION docs/THREAT_MODEL.md "5. Not defended"}}
{{PARAGRAPH docs/THREAT_MODEL.md "Out of scope:"}}
</section>
<section class="section" id="status">
<h2>Status</h2>
{{REQUIRE README.md "not yet run against the live services"}}
<p>Sign-in has not been run against the live services.</p>
{{SECTION README.md "Status"}}
</section>
<figure class="shot">
{{SHOT one "The window as the front end's tests draw it."}}
<figcaption>Drawn by a test.</figcaption>
</figure>
{{INCLUDE partials/aside.html}}
"""

GETTING_STARTED = """\
title: Get started | bastion-mc
description: How to build the launcher under test.
nav: start
---
<h1>Get started</h1>
<section class="section" id="build">
<h2>Build</h2>
<pre tabindex="0"><code>cargo build --release
bastion-mc doctor
bastion-mc launch &lt;id&gt; --demo</code></pre>
<p>Then <code>bastion-mc status</code>; see the <a href="manual.html#requirements">requirements</a>.</p>
</section>
"""

SECURITY = """\
title: Security | bastion-mc
description: The model in plain language.
nav: security
class: page-security
---
<h1>Security</h1>
<p>Read <a href="threat-model.html#5-not-defended">what is not defended</a> and row <a href="threat-model.html#row-j2">J2</a>.</p>
<p>This site is served from <span class="readout">{{SITE_URL}}</span>.</p>
"""

NOT_FOUND = """\
title: Page not found | bastion-mc
description: There is no page at this address.
---
<h1>Page not found</h1>
<p>Go to the <a href="{{ROOT}}index.html">overview</a>.</p>
"""

ASIDE = """\
<aside class="callout note">
<p>An included note.</p>
</aside>
"""

MARK = """\
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8" width="8" height="8">
<title>mark</title>
<path fill="#3fd9e6" d="M0 0h8v8H0z"/>
</svg>
"""

FIXTURE = {
    "README.md": README,
    "LICENSE": "MIT\n",
    "docs/THREAT_MODEL.md": THREAT_MODEL,
    "docs/TOKEN_INJECTION.md": TOKEN_INJECTION,
    "docs/MC_VAULT_ACCOUNT.md": DEDICATED_ACCOUNT,
    "docs/MODS.md": MODS,
    "docs/COMPANION.md": COMPANION,
    "docs/PHONE.md": PHONE,
    "docs/BEDROCK.md": BEDROCK,
    "docs/SCRAM_RUNBOOK.md": RUNBOOK,
    "crates/proto/src/runbook.rs": RUNBOOK_RS,
    "src/cli.rs": CLI_RS,
    "site/src/template.html": TEMPLATE,
    "site/src/partials/footer.html": FOOTER,
    "site/src/partials/footer-minimal.html": FOOTER_MINIMAL,
    "site/src/partials/aside.html": ASIDE,
    "site/src/document.html": DOCUMENT_SHELL,
    "site/src/emergency.html": EMERGENCY_SHELL,
    "site/src/robots.txt": "User-agent: *\nAllow: /\n\nSitemap: {{SITE_URL}}sitemap.xml\n",
    "site/src/pages/index.html": INDEX,
    "site/src/pages/getting-started.html": GETTING_STARTED,
    "site/src/pages/security.html": SECURITY,
    "site/src/pages/404.html": NOT_FOUND,
    "site/src/assets/site.css": "body { margin: 0; }\n",
    "site/src/assets/mark.svg": MARK,
    "site/src/assets/shot-one.png": png(),
}

PAGES = ("404.html", "bedrock.html", "companion.html", "dedicated-account.html", "emergency.html", "getting-started.html",
         "index.html", "manual.html", "mods.html", "phone.html", "security.html", "threat-model.html", "token-injection.html")
LINKS = ("https://account.example.com/password", "https://account.example.com/sessions?all=1&now=1")


class SiteCase(unittest.TestCase):
    """A test with its own copy of the small site."""

    maxDiff = None

    def setUp(self):
        scratch = tempfile.TemporaryDirectory(prefix="bastion-site-test-")
        self.addCleanup(scratch.cleanup)
        self.scratch = Path(scratch.name)
        self.root = self.scratch / "root"
        self.out = self.scratch / "out"
        self.notices = []
        for name, content in FIXTURE.items():
            self.write(name, content)

    # -- changing the sources -----------------------------------------------------------

    def write(self, relative, content):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            path.write_bytes(content)
        else:
            path.write_text(content, encoding="utf-8")

    def read(self, relative):
        return (self.root / relative).read_text(encoding="utf-8")

    def edit(self, relative, old, new):
        """Replace the one occurrence of `old`. (A change that no longer applies must fail
        the test instead of leaving it green for no reason.)"""
        text = self.read(relative)
        self.assertEqual(text.count(old), 1, "%r must occur exactly once in %s" % (old, relative))
        self.write(relative, text.replace(old, new))

    def add(self, relative, text):
        """Append to a page, a shell or a document."""
        self.write(relative, self.read(relative) + text)

    def remove(self, relative):
        (self.root / relative).unlink()

    # -- building -----------------------------------------------------------------------

    def site(self, deploy=False):
        if deploy:
            return build.Site(self.root, REPO, SITE, absolute_error_page=True)
        return build.Site(self.root, build.PREVIEW_REPO_URL, build.PREVIEW_SITE_URL, absolute_error_page=False)

    def generate(self, deploy=False):
        site = self.site(deploy)
        build.write_output(site.build(), self.out)
        self.notices = site.notices
        return site

    def page(self, name, deploy=False):
        """The text of one generated file."""
        self.generate(deploy)
        return (self.out / name).read_text(encoding="utf-8")

    def check(self, deploy=False):
        site = self.generate(deploy)
        problems, _ = build.check_output(self.out, site, explicit_urls=deploy)
        self.notices = site.notices
        return problems

    def run_main(self, *arguments):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            status = build.main(["--root", str(self.root), "--out", str(self.out)] + list(arguments))
        return status, out.getvalue(), err.getvalue()

    # -- stating the result -------------------------------------------------------------

    def assertStops(self, *parts, deploy=False):
        """The build stops, and its message contains every one of `parts`."""
        with self.assertRaises((build.BuildError, md.MarkdownError)) as caught:
            self.site(deploy).build()
        for part in parts:
            self.assertIn(part, str(caught.exception))
        return str(caught.exception)

    def assertFails(self, *parts, deploy=False, pages=None):
        """`--check` reports problems, each of `parts` is found in at least one of them,
        and every problem contains one of `parts`: the change breaks this and nothing
        else. With `pages`, exactly these pages are named."""
        problems = self.check(deploy)
        self.assertTrue(problems, "the check passed")
        for part in parts:
            self.assertTrue(any(part in problem for problem in problems), "%r is in none of %r" % (part, problems))
        for problem in problems:
            self.assertTrue(any(part in problem for part in parts), "unexpected problem: %s (all: %r)" % (problem, problems))
        if pages is not None:
            self.assertEqual(sorted({problem.split(":")[0] for problem in problems}), sorted(pages), problems)
        return problems


# --------------------------------------------------------------------------------------
# The small site as it is
# --------------------------------------------------------------------------------------


class TheSmallSite(SiteCase):
    def test_it_passes_every_check(self):
        self.assertEqual(self.check(), [])
        self.assertEqual(self.notices, [])

    def test_it_passes_as_a_deployable_build_too(self):
        self.assertEqual(self.check(deploy=True), [])

    def test_the_files_it_consists_of(self):
        self.generate()
        files = sorted(path.relative_to(self.out).as_posix() for path in self.out.rglob("*") if path.is_file())
        self.assertEqual(files, sorted(PAGES + (".bastion-site-output", "assets/mark.svg", "assets/shot-one.png",
                                                "assets/site.css", "robots.txt", "sitemap.xml")))

    def test_assets_are_copied_byte_for_byte(self):
        self.generate()
        for name in ("mark.svg", "shot-one.png", "site.css"):
            self.assertEqual((self.out / "assets" / name).read_bytes(), (self.root / "site/src/assets" / name).read_bytes())

    def test_the_command_line(self):
        status, out, err = self.run_main("--check")
        self.assertEqual((status, err), (0, ""))
        self.assertIn("wrote 18 files to %s\n" % self.out, out)
        self.assertIn("check passed: 13 pages, ", out)
        self.assertIn("two runs byte-identical\n", out)
        self.assertIn("preview build: pass --repo-url and --site-url for a deployable one\n", out)

    def test_the_command_line_of_a_deployable_build(self):
        status, out, err = self.run_main("--check", "--repo-url", REPO, "--site-url", SITE)
        self.assertEqual((status, err), (0, ""))
        self.assertIn("check passed: 13 pages, ", out)
        self.assertNotIn("preview build", out)

    def test_without_check_nothing_is_checked(self):
        self.add("site/src/pages/security.html", '<p><a href="nowhere.html">a broken link</a></p>\n')
        status, out, err = self.run_main()
        self.assertEqual((status, err), (0, ""))
        self.assertNotIn("check passed", out)
        status, out, err = self.run_main("--check")
        self.assertEqual(status, 1)
        self.assertIn("check failed: security.html: link 'nowhere.html': nowhere.html does not exist in the output\n", err)
        self.assertIn("error: 1 problem(s) in the generated site\n", err)


# --------------------------------------------------------------------------------------
# What a page source may contain
# --------------------------------------------------------------------------------------


class Addresses(SiteCase):
    def test_a_preview_points_to_names_that_cannot_resolve(self):
        index = self.page("index.html")
        self.assertIn('<a href="https://repository.invalid" rel="noopener noreferrer">source</a>', index)
        self.assertIn('<link rel="canonical" href="https://site.invalid/">', index)
        self.assertIn("https://site.invalid/</span>", self.page("security.html"))

    def test_a_deployable_build_carries_the_addresses_it_was_given(self):
        index = self.page("index.html", deploy=True)
        self.assertIn('<a href="%s" rel="noopener noreferrer">source</a>' % REPO, index)
        self.assertIn('<link rel="canonical" href="%s">' % SITE, index)
        self.assertIn('<link rel="canonical" href="%smanual.html">' % SITE, self.page("manual.html", deploy=True))
        for name in PAGES + ("robots.txt", "sitemap.xml"):
            with self.subTest(file=name):
                text = (self.out / name).read_text(encoding="utf-8")
                self.assertNotIn(".invalid", text)
                self.assertNotIn("{{", text)

    def test_links_inside_the_site_stay_relative_in_a_deployable_build(self):
        index = self.page("index.html", deploy=True)
        self.assertIn('<a href="getting-started.html#build">', index)
        self.assertIn('<link rel="stylesheet" href="assets/site.css">', index)
        self.assertIn('<li><a href="manual.html">Manual</a></li>', index)

    def test_the_error_page_is_relative_in_a_preview_and_absolute_when_deployed(self):
        # GitHub Pages serves 404.html for a missing address at any depth, where relative
        # references would point nowhere.
        preview = self.page("404.html")
        self.assertIn('<link rel="stylesheet" href="assets/site.css">', preview)
        self.assertIn('<a href="index.html">overview</a>', preview)
        deployed = self.page("404.html", deploy=True)
        self.assertIn('<link rel="stylesheet" href="%sassets/site.css">' % SITE, deployed)
        self.assertIn('<link rel="icon" href="%sassets/mark.svg" type="image/svg+xml">' % SITE, deployed)
        self.assertIn('<p>Go to the <a href="%sindex.html">overview</a>.</p>' % SITE, deployed)
        self.assertIn('<li><a href="%smanual.html">Manual</a></li>' % SITE, deployed)
        self.assertIn('Back to the <a href="%sindex.html">overview</a>' % SITE, deployed)

    def test_the_error_page_is_not_indexed_and_has_no_canonical_address(self):
        for deploy in (False, True):
            page = self.page("404.html", deploy=deploy)
            self.assertIn('<meta name="robots" content="noindex">', page)
            self.assertNotIn("canonical", page)
        self.assertNotIn("noindex", self.page("index.html"))

    def test_the_sitemap_lists_every_page_but_the_error_page(self):
        self.generate(deploy=True)
        expected = "".join("  <url><loc>%s%s</loc></url>\n" % (SITE, page) for page in (
            "", "getting-started.html", "security.html", "manual.html", "threat-model.html", "token-injection.html",
            "dedicated-account.html", "mods.html", "companion.html", "phone.html", "bedrock.html", "emergency.html"))
        self.assertEqual(
            (self.out / "sitemap.xml").read_text(encoding="utf-8"),
            '<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
            + expected + "</urlset>\n",
        )
        self.assertEqual((self.out / "robots.txt").read_text(encoding="utf-8"),
                         "User-agent: *\nAllow: /\n\nSitemap: %ssitemap.xml\n" % SITE)

    def test_the_addresses_are_normalised(self):
        self.assertEqual(build.normalise_site_url("https://owner.example.org/project"), "https://owner.example.org/project/")
        self.assertEqual(build.normalise_site_url("https://owner.example.org/project/"), "https://owner.example.org/project/")
        self.assertEqual(build.normalise_site_url("https://owner.example.org"), "https://owner.example.org/")
        self.assertEqual(build.normalise_repo_url("https://code.example.org/owner/project/"), REPO)
        self.assertEqual(build.normalise_repo_url(REPO), REPO)

    def test_addresses_that_are_refused(self):
        for url in ("http://owner.example.org/project/", "owner.example.org/project", "https://owner.example.org/p?x=1",
                    "https://owner.example.org/p#top", "https://user@owner.example.org/", "https://owner.example.org:8443/",
                    "https://localhost/", 'https://owner.example.org/"x', "https://owner.example.org/ x", "javascript:alert(1)"):
            with self.subTest(url=url):
                with self.assertRaises(build.BuildError):
                    build.normalise_site_url(url)
                with self.assertRaises(build.BuildError):
                    build.normalise_repo_url(url)

    def test_the_command_line_refuses_a_bad_address_before_it_writes_anything(self):
        for arguments, message in (
            (("--site-url", "http://owner.example.org/"), "error: --site-url 'http://owner.example.org/': only https:// addresses are allowed\n"),
            (("--repo-url", "https://code.example.org/x?y"), "error: --repo-url 'https://code.example.org/x?y' must not have a query or a fragment\n"),
        ):
            with self.subTest(arguments=arguments):
                self.assertEqual(self.run_main("--check", *arguments), (1, "", message))
                self.assertFalse(self.out.exists())

    def test_an_address_that_is_given_but_empty_is_an_error_not_a_preview(self):
        # A deployment script whose variable is unset must not quietly publish a preview.
        for option in ("--site-url", "--repo-url"):
            for value in ("", "  "):
                with self.subTest(option=option, value=value):
                    for arguments in ((option, value), ("--check", option, value)):
                        self.assertEqual(self.run_main(*arguments),
                                         (1, "", "error: %s is empty (leave the option out for a local preview)\n" % option))
                    self.assertFalse(self.out.exists())

    def test_one_address_alone_is_still_a_preview(self):
        status, out, err = self.run_main("--check", "--site-url", SITE)
        self.assertEqual((status, err), (0, ""))
        self.assertIn("preview build", out)
        self.assertIn("https://repository.invalid", (self.out / "index.html").read_text(encoding="utf-8"))
        self.assertIn('href="%sindex.html"' % SITE, (self.out / "404.html").read_text(encoding="utf-8"))


class Directives(SiteCase):
    def test_a_section_of_a_document_is_included_without_its_heading(self):
        index = self.page("index.html")
        self.assertIn(
            "<h2>What it does not defend</h2>\n<ol>\n"
            "<li><strong>Kernel bugs.</strong> A kernel bug defeats every layer.</li>\n"
            '<li><strong>You.</strong> See the <a href="manual.html#status">status</a> and row '
            '<a href="threat-model.html#row-j1">J1</a>.</li>\n</ol>\n'
            "<p>Not exercised: the live sign-in.</p>\n"
            "<p>Out of scope: a kernel that is already compromised.</p>\n</section>",
            index,
        )
        self.assertNotIn("5. Not defended", index)

    def test_an_included_table_keeps_its_wrapper(self):
        self.assertIn(
            '<div class="table-wrap">\n<table class="cols-2" role="table">\n<tbody role="rowgroup">\n'
            '<tr role="row"><td role="cell">Sandbox</td><td role="cell"><strong>Tested</strong> on one machine.</td></tr>\n',
            self.page("index.html"),
        )

    def test_the_section_must_exist_exactly_once(self):
        self.edit("site/src/pages/index.html", '"5. Not defended"', '"6. Something else"')
        self.assertStops("docs/THREAT_MODEL.md: expected exactly one heading that starts with '6. Something else', found 0")

    def test_two_headings_that_start_alike_are_refused(self):
        self.add("docs/THREAT_MODEL.md", "\n## 5. Not defended, again\n\nMore.\n")
        self.assertStops("expected exactly one heading that starts with '5. Not defended', found 2")

    def test_a_section_with_a_heading_inside_cannot_be_included(self):
        self.add("docs/THREAT_MODEL.md", "\n### 5.1 Detail\n\nMore.\n")
        self.assertStops("docs/THREAT_MODEL.md:19: the section included in index.html has a sub-heading; the site cannot place it")

    def test_a_section_ends_at_the_next_heading_of_its_level(self):
        self.add("docs/THREAT_MODEL.md", "\n## 6. Afterwards\n\nNot part of section five.\n")
        self.assertNotIn("Not part of section five", self.page("index.html"))
        self.assertIn("Not part of section five", self.page("threat-model.html"))

    def test_a_paragraph_must_exist_exactly_once(self):
        self.edit("docs/THREAT_MODEL.md", "Out of scope: a kernel", "Outside the scope: a kernel")
        self.assertStops("docs/THREAT_MODEL.md: expected exactly one paragraph that starts with 'Out of scope:', found 0")

    def test_two_paragraphs_that_start_alike_are_refused(self):
        self.add("docs/THREAT_MODEL.md", "\nOut of scope: also a compromised compositor.\n")
        self.assertStops("docs/THREAT_MODEL.md: expected exactly one paragraph that starts with 'Out of scope:', found 2")

    def test_only_the_rendered_documents_can_be_included(self):
        self.edit("site/src/pages/index.html", '{{PARAGRAPH docs/THREAT_MODEL.md "Out of scope:"}}',
                  '{{PARAGRAPH LICENSE "MIT"}}')
        self.assertStops("LICENSE is not one of the documents the site renders")

    def test_require_writes_nothing(self):
        self.assertIn("<h2>Status</h2>\n\n<p>Sign-in has not been run against the live services.</p>", self.page("index.html"))

    def test_require_reads_the_text_as_a_reader_does(self):
        # In the document the phrase runs over two lines, with emphasis and a code span in it.
        self.assertIn("*not yet run* against the\n`live` services", self.read("README.md"))
        self.edit("site/src/pages/index.html", '"not yet run against the live services"',
                  '"implemented but  not yet run\n against the live services."')
        self.assertEqual(self.check(), [])

    def test_require_does_not_see_an_html_comment(self):
        # GitHub shows no comment (README.md marks numbers with <!-- COUNTS -->), so a reader's
        # text, and with it a phrase, runs over it as if it were not there.
        self.edit("README.md", "| Sandbox | **Tested** on one machine. |", "| Sandbox | **Tested** <!-- C --> on one machine. |")
        self.edit("site/src/pages/index.html", '"not yet run against the live services"', '"Tested on one machine."')
        self.assertEqual(self.check(), [])
        self.assertIn('<td role="cell">Sandbox</td><td role="cell"><strong>Tested</strong>  on one machine.</td>',
                      self.page("manual.html"))

    def test_require_stops_the_build_when_the_document_no_longer_says_it(self):
        self.edit("README.md", "*not yet run* against the\n`live` services", "run against the live services")
        self.assertStops("README.md no longer says 'not yet run against the live services', which a statement in "
                         "site/src/pages/index.html relies on ({{REQUIRE …}})")

    def test_require_is_exact_about_capitals_and_punctuation(self):
        self.edit("site/src/pages/index.html", '"not yet run against the live services"',
                  '"Not yet run against the live services"')
        self.assertStops("README.md no longer says 'Not yet run against the live services'")

    def test_require_needs_a_phrase_and_a_rendered_document(self):
        self.edit("site/src/pages/index.html", '{{REQUIRE README.md "not yet run against the live services"}}',
                  '{{REQUIRE README.md " "}}')
        self.assertStops('site/src/pages/index.html: {{REQUIRE README.md ""}} names no phrase')
        self.edit("site/src/pages/index.html", '{{REQUIRE README.md " "}}', '{{REQUIRE TODO.md "anything"}}')
        self.assertStops("TODO.md is not one of the documents the site renders")

    ORDERED = """\
# Token path

## 1. What is kept

1. **First.** The first point ends here.
2. **Second.** The second point,
   over two lines.
   - a nested *item*

| Token | Lifetime | What it does |
|---|---|---|
| Access token | 24 h | Lets its holder play |
| Refresh token | weeks | Mints new sessions |

> A quoted remark.

```sh
bastion-mc scram
```

## 2. What is not
"""

    def required(self, phrase):
        """None if a page that requires `phrase` of the document above builds, else the error."""
        self.write("docs/TOKEN_INJECTION.md", self.ORDERED)
        self.write("site/src/pages/security.html", SECURITY + '{{REQUIRE docs/TOKEN_INJECTION.md "%s"}}\n' % phrase)
        try:
            self.site().build()
        except build.BuildError as problem:
            return str(problem)
        return None

    def test_require_reads_the_document_in_reading_order(self):
        # Promised to the people who write the pages: the text is every block in the order
        # of the document, joined by single spaces, so a phrase may run from one block into
        # the next. That holds the order of two list items, ties the label of a table row
        # to its value, and a heading to what stands under it.
        for phrase in (
            "Token path 1. What is kept First. The first point",  # the title, a heading, the first item
            "ends here. Second. The second point, over two lines. a nested item",  # item, item, the list inside it
            "a nested item Token Lifetime What it does Access token 24 h Lets its holder play",  # a table: header, then rows
            "Access token 24 h",
            "Refresh token weeks Mints new sessions A quoted remark. bastion-mc scram 2. What is not",  # quote, code, heading
        ):
            with self.subTest(phrase=phrase):
                self.assertIsNone(self.required(phrase))

    def test_require_in_another_order_does_not_match(self):
        for phrase in (
            "Second. The second point, over two lines. First.",  # the two items the other way round
            "ends here. a nested item",  # an item left out
            "Refresh token 24 h",  # the label of one row, the value of another
            "What it does Refresh token",  # a row left out
            "2. What is not 1. What is kept",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn("docs/TOKEN_INJECTION.md no longer says %r" % phrase, self.required(phrase))

    def test_every_phrase_that_is_gone_is_named_in_one_run(self):
        gone = ("%s no longer says %r, which a statement in site/src/pages/security.html relies on ({{REQUIRE …}}): "
                "read that statement against the document again")
        self.write("site/src/pages/security.html", SECURITY
                   + '{{REQUIRE README.md "Built and tested on Arch Linux."}}\n'
                   + '{{REQUIRE README.md "tested on three machines"}}\n'
                   + '{{REQUIRE docs/THREAT_MODEL.md "a kernel bug is harmless"}}\n'
                   + '{{REQUIRE README.md "tested on three machines"}}\n'
                   + "{{NOSUCH}}\n"
                   + '{{REQUIRE docs/THREAT_MODEL.md "A kernel bug defeats every layer."}}\n')
        self.edit("site/src/pages/index.html", '"not yet run against the live services"', '"run against the live services yesterday"')
        expected = [
            "README.md no longer says 'run against the live services yesterday', which a statement in "
            "site/src/pages/index.html relies on ({{REQUIRE …}}): read that statement against the document again",
            gone % ("README.md", "tested on three machines"),  # (once, although two directives say it)
            gone % ("docs/THREAT_MODEL.md", "a kernel bug is harmless"),
            "site/src/pages/security.html: unknown directive {{NOSUCH}}",
        ]
        with self.assertRaises(build.BuildErrors) as caught:
            self.site().build()
        self.assertEqual(caught.exception.problems, expected)
        status, out, err = self.run_main("--check")
        self.assertEqual((status, out), (1, ""))
        self.assertEqual(err, "".join("error: %s\n" % problem for problem in expected)
                         + "error: nothing was written (4 problem(s) in the sources; --keep-going writes the pages that can "
                           "be built)\n")

    def test_what_the_converter_refuses_in_an_included_section_is_one_of_the_problems(self):
        # The section is rendered where the page includes it, so a link in it that leads
        # nowhere is found there, with the file and the line; the page's other problems are
        # still named, before and after it.
        self.edit("README.md", "| Sign-in | Implemented. |", "| Sign-in | Implemented, see [the plan](docs/PLAN.md). |")
        self.edit("site/src/pages/index.html", '"not yet run against the live services"', '"run against the live services yesterday"')
        self.edit("site/src/pages/index.html", "{{INCLUDE partials/aside.html}}", "{{INCLUDE partials/gone.html}}")
        with self.assertRaises(build.BuildErrors) as caught:
            self.site().build()
        self.assertEqual(caught.exception.problems, [
            "README.md no longer says 'run against the live services yesterday', which a statement in "
            "site/src/pages/index.html relies on ({{REQUIRE …}}): read that statement against the document again",
            "README.md:12: link target 'docs/PLAN.md' does not exist in the repository",  # (once: the manual has it too)
            "site/src/partials/gone.html does not exist",
        ])

    def test_every_broken_directive_of_a_shell_is_named_too(self):
        self.add("site/src/emergency.html", "{{NOSUCH}}\n{{COUNT now}}\n")
        self.add("docs/SCRAM_RUNBOOK.md", "3. Or ask: https://support.example.com/help or https://support.example.com/more\n")
        with self.assertRaises(build.BuildErrors) as caught:
            self.site().build()
        self.assertEqual(caught.exception.problems, [
            "docs/SCRAM_RUNBOOK.md: https://support.example.com/help is not in runbook::LINKS (crates/proto/src/runbook.rs)",
            "site/src/emergency.html: unknown directive {{NOSUCH}}",
            caught.exception.problems[2],
        ])
        self.assertTrue(caught.exception.problems[2].startswith("site/src/emergency.html: wrong arguments in {{COUNT now}}: "))

    def test_a_screenshot_carries_its_size_and_its_text(self):
        self.assertIn(
            '<a class="shot" href="assets/shot-one.png"><img src="assets/shot-one.png" width="4" height="3" '
            'alt="The window as the front end&#x27;s tests draw it." loading="lazy" decoding="async"></a>',
            self.page("index.html"),
        )
        self.write("site/src/assets/shot-one.png", png(1280, 800))
        self.assertIn('width="1280" height="800"', self.page("index.html"))

    def test_screenshots_that_are_refused(self):
        directive = '{{SHOT one "The window as the front end\'s tests draw it."}}'
        for replacement, message in (
            ('{{SHOT two "A picture that does not exist."}}', "site/src/assets/shot-two.png is missing (run site/update-screenshots.sh)"),
            ('{{SHOT ../mark "Outside."}}', "screenshot name '../mark': lower-case letters, digits and single hyphens only"),
            ('{{SHOT One "Capitals."}}', "screenshot name 'One'"),
            ('{{SHOT one-.x "Dots."}}', "screenshot name 'one-.x'"),
            ('{{SHOT one "  "}}', "screenshot 'one' needs a text that says what it shows"),
            ("{{SHOT one}}", "site/src/pages/index.html: wrong arguments in {{SHOT one}}"),
            ('{{SHOT one "a" "b"}}', "wrong arguments in {{SHOT one \"a\" \"b\"}}"),
        ):
            with self.subTest(directive=replacement):
                self.write("site/src/pages/index.html", INDEX.replace(directive, replacement))
                self.assertStops(message)

    def test_a_screenshot_must_be_a_png_file(self):
        self.write("site/src/assets/shot-one.png", b"GIF89a" + b"\0" * 40)
        self.assertStops("shot-one.png is not a PNG file")
        self.write("site/src/assets/shot-one.png", b"\x89PNG\r\n\x1a\n")
        self.assertStops("shot-one.png is not a PNG file")

    def test_an_included_file_is_copied_as_it_is(self):
        self.assertIn('</figure>\n<aside class="callout note">\n<p>An included note.</p>\n</aside>\n</main>', self.page("index.html"))

    def test_includes_that_are_refused(self):
        for name, message in (
            ("partials/missing.html", "site/src/partials/missing.html does not exist"),
            ("../../README.md", "site/src/../../README.md: not a plain path below site/src/"),
            ("/etc/passwd", "site/src//etc/passwd: not a plain path below site/src/"),
            ("partials/.hidden", "not a plain path below site/src/"),
            ("partials/footer.html", "site/src/partials/footer.html: an included file cannot use directives itself"),
        ):
            with self.subTest(include=name):
                self.write("site/src/pages/index.html", INDEX.replace("{{INCLUDE partials/aside.html}}", "{{INCLUDE %s}}" % name))
                self.assertStops(message)

    def test_unknown_and_unreadable_directives(self):
        for directive, message in (
            ("{{NOSUCH thing}}", "site/src/pages/security.html: unknown directive {{NOSUCH thing}}"),
            ("{{}}", "site/src/pages/security.html: unknown directive {{}}"),
            ("{{CSP}}", "site/src/pages/security.html: unknown directive {{CSP}}"),
            ('{{SECTION README.md "unclosed}}', "site/src/pages/security.html: cannot read {{SECTION README.md \"unclosed}}"),
            ("{{ROOT now}}", "site/src/pages/security.html: wrong arguments in {{ROOT now}}"),
        ):
            with self.subTest(directive=directive):
                self.write("site/src/pages/security.html", SECURITY + "<p>%s</p>\n" % directive)
                self.assertStops(message)

    def test_a_directive_that_lost_a_brace_stops_the_build(self):
        # Neither half would be expanded, and the check of the output looks for whole ones.
        for text, message in (
            ('<a href="{{ROOT}index.html">x</a>',
             "site/src/pages/security.html: '{{' that is not part of a whole directive, at '{{ROOT}index.html\">x</a></p>'"),
            ('<a href="{ROOT}}index.html">x</a>',
             "site/src/pages/security.html: '}}' that is not part of a whole directive, at '}}index.html\">x</a></p>'"),
            ('{{ROOT}}{{ROOT}}}}', "site/src/pages/security.html: '}}' that is not part of a whole directive, at '}}</p>'"),
        ):
            with self.subTest(text=text):
                self.write("site/src/pages/security.html", SECURITY + "<p>%s</p>\n" % text)
                self.assertEqual(self.assertStops(message), message)
        # A single brace is text, also next to a directive.
        self.write("site/src/pages/security.html", SECURITY + "<p>{ <code>{{{ROOT}}}</code> }</p>\n")
        self.assertIn("<p>{ <code>{}</code> }</p>", self.page("security.html"))
        # The shells and the footers are read the same way, and every problem is named.
        self.write("site/src/pages/security.html", SECURITY)
        self.edit("site/src/template.html", "<title>{{TITLE}}</title>", "<title>{{TITLE}</title>")
        self.edit("site/src/partials/footer.html", 'href="{{REPO_URL}}"', 'href="{{REPO_URL}"')
        with self.assertRaises(build.BuildErrors) as caught:
            self.site().build()
        self.assertEqual(caught.exception.problems, [
            # (what follows the mark, 40 characters of it)
            "site/src/partials/footer.html: '{{' that is not part of a whole directive, at '{{REPO_URL}\" rel=\"noopener noreferrer\">s'",
            "site/src/template.html: '{{' that is not part of a whole directive, at '{{TITLE}</title>\\n<meta name=\"description'",
        ])

    def test_the_shells_have_their_own_directives(self):
        self.add("site/src/template.html", "{{SECTION README.md \"Status\"}}\n")
        self.assertStops("site/src/template.html: unknown directive {{SECTION README.md \"Status\"}}")
        self.write("site/src/template.html", TEMPLATE)
        self.add("site/src/partials/footer.html", "{{TITLE}}\n")
        self.assertStops("site/src/partials/footer.html: unknown directive {{TITLE}}")
        self.write("site/src/partials/footer.html", FOOTER)
        self.add("site/src/document.html", "{{WARNING}}\n")
        self.assertStops("site/src/document.html: unknown directive {{WARNING}}")

    def test_the_generators_description_lists_exactly_the_directives_there_are(self):
        seen = {}
        original = build.expand

        def recording(text, handlers, where):
            seen.setdefault(where, set()).update(handlers)
            return original(text, handlers, where)

        build.expand = recording
        try:
            self.site().build()
        finally:
            build.expand = original
        self.assertEqual(seen["site/src/pages/index.html"],
                         {"ROOT", "REPO_URL", "SITE_URL", "INCLUDE", "SECTION", "PARAGRAPH", "REQUIRE", "SHOT"})
        self.assertEqual(seen["site/src/partials/footer.html"], {"ROOT", "REPO_URL", "SITE_URL"})
        self.assertEqual(seen["site/src/robots.txt"], {"SITE_URL"})
        self.assertEqual(seen["site/src/document.html"],
                         {"LABEL", "TITLE", "TITLE_ID", "PATH", "SOURCE", "HISTORY", "TOC", "BODY"})
        self.assertEqual(seen["site/src/emergency.html"], {"TITLE", "TITLE_ID", "WARNING", "LEAD", "BODY", "PATH", "COUNT"})
        self.assertEqual(seen["site/src/template.html"], {"ROOT", "REPO_URL", "SITE_URL", "CSP", "TITLE", "DESCRIPTION", "HEAD",
                                                         "BODY_CLASS", "NAVIGATION", "CONTENT", "FOOTER"})
        # ... and the text at the top of build.py names every one of them, and no other.
        self.assertEqual(set(re.findall(r"\{\{([A-Z_]+)", build.__doc__)), set().union(*seen.values()))

    def test_text_from_a_document_is_never_taken_for_a_directive(self):
        # What a handler returns is not scanned again, and a document may show a directive
        # as text: it is published as it reads (`&#123;` is a brace to the reader), and the
        # check for placeholders left unsubstituted is not misled by it (renderer-11).
        self.edit("README.md", "Built and tested on Arch Linux.", "Built and tested at `{{REPO_URL}}` on Arch Linux {{NOSUCH}}.")
        manual = self.page("manual.html")
        self.assertIn("<p>Built and tested at <code>&#123;{REPO_URL}}</code> on Arch Linux &#123;{NOSUCH}}.</p>", manual)
        self.assertEqual(self.check(), [])


class PageHeaders(SiteCase):
    def test_title_description_class_and_navigation(self):
        page = self.page("security.html")
        self.assertIn("<title>Security | bastion-mc</title>", page)
        self.assertIn('<meta name="description" content="The model in plain language.">', page)
        self.assertIn('<body class="page-security">', page)
        self.assertIn('<li><a href="security.html" aria-current="page">Security</a></li>', page)
        self.assertEqual(page.count("aria-current"), 1)
        self.assertIn('<li><a href="emergency.html" class="nav-emergency">Emergency</a></li>', page)

    def test_a_page_without_a_navigation_entry_marks_none(self):
        self.assertNotIn("aria-current", self.page("index.html"))

    def test_the_navigation_is_the_same_five_entries_everywhere(self):
        self.generate()
        for name in PAGES:
            with self.subTest(page=name):
                text = (self.out / name).read_text(encoding="utf-8")
                found = re.findall(r'<li><a href="([a-z-]+\.html)"[^>]*>([^<]+)</a></li>', text.split("</nav>")[0])
                self.assertEqual(found, [("getting-started.html", "Get started"), ("security.html", "Security"),
                                         ("manual.html", "Manual"), ("threat-model.html", "Threat model"),
                                         ("emergency.html", "Emergency")])

    def test_what_a_header_says_is_escaped(self):
        self.edit("site/src/pages/security.html", "title: Security | bastion-mc", 'title: A <b> & "c" | bastion-mc')
        self.edit("site/src/pages/security.html", "class: page-security", 'class: x" onload="alert(1)')
        page = self.page("security.html")
        self.assertIn("<title>A &lt;b&gt; &amp; &quot;c&quot; | bastion-mc</title>", page)
        self.assertIn('<body class="x&quot; onload=&quot;alert(1)">', page)

    def test_another_footer_can_be_chosen(self):
        self.edit("site/src/pages/security.html", "nav: security\n", "nav: security\nfooter: footer-minimal\n")
        self.assertNotIn("source code", self.page("security.html"))
        self.assertIn("source code", self.page("index.html"))

    def test_headers_that_are_refused(self):
        for old, new, message in (
            ("---\n", "", "site/src/pages/security.html: missing the '---' line that ends the header"),
            ("nav: security\n", "nav security\n", "site/src/pages/security.html: header line 'nav security' is not 'key: value'"),
            ("nav: security\n", "nav:\n", "header line 'nav:' is not 'key: value'"),
            ("nav: security\n", "colour: red\n", "the header needs title and description, and knows nav, class and footer"),
            ("title: Security | bastion-mc\n", "", "the header needs title and description"),
            ("description: The model in plain language.\n", "", "the header needs title and description"),
            ("nav: security\n", "nav: Security\n",
             "site/src/pages/security.html: 'nav: Security' is not an entry of the navigation "
             "(start, security, manual, threat-model, emergency)"),
            ("nav: security\n", "nav: security\nfooter: ../template\n", "not a plain path below site/src/"),
            ("nav: security\n", "nav: security\nfooter: none\n", "site/src/partials/none.html does not exist"),
        ):
            with self.subTest(change=new):
                self.assertEqual(SECURITY.count(old), 1)
                self.write("site/src/pages/security.html", SECURITY.replace(old, new))
                self.assertStops(message)


class Documents(SiteCase):
    def test_a_document_page(self):
        page = self.page("token-injection.html")
        self.assertIn("<title>Token path | bastion-mc</title>", page)
        self.assertIn('<body class="page-document">', page)
        self.assertIn(
            '<main id="site-content">\n<p class="label">Token path</p>\n<h1 id="token-path">Token path</h1>\n'
            '<p>Generated from <a href="https://repository.invalid/blob/main/docs/TOKEN_INJECTION.md" '
            'rel="noopener noreferrer"><code>docs/TOKEN_INJECTION.md</code></a> '
            '(<a href="https://repository.invalid/commits/main/docs/TOKEN_INJECTION.md" rel="noopener noreferrer">its history</a>).</p>\n'
            '<nav class="toc" aria-label="On this page">\n<ol>\n<li><a href="#how-it-gets-there">How it gets there</a>\n</li>\n</ol>\n</nav>\n'
            '<article class="doc">\n'
            '<div class="heading"><h2 id="how-it-gets-there">How it gets there</h2>'
            '<a class="anchor" href="#how-it-gets-there" aria-label="Permalink: How it gets there">#</a></div>\n'
            "<p>Never on a command line.</p>\n</article>\n</main>",
            page,
        )

    def test_the_manual_is_called_the_manual_not_by_the_projects_name(self):
        page = self.page("manual.html")
        self.assertIn("<title>Manual | bastion-mc</title>", page)
        self.assertIn('<p class="label">Manual</p>\n<h1 id="bastion-mc">bastion-mc</h1>', page)
        self.assertIn('<li><a href="manual.html" aria-current="page">Manual</a></li>', page)

    def test_the_table_of_contents_nests_the_third_level(self):
        self.assertIn(
            '<ol>\n<li><a href="#status">Status</a>\n</li>\n<li><a href="#requirements">Requirements</a>\n</li>\n'
            '<li><a href="#playing">Playing</a>\n<ol>\n<li><a href="#the-display">The display</a></li>\n</ol>\n</li>\n</ol>',
            self.page("manual.html"),
        )

    def test_a_document_without_sections_has_an_empty_table_of_contents(self):
        self.write("docs/TOKEN_INJECTION.md", "# Token path\n\nOne paragraph.\n")
        self.assertIn('<nav class="toc" aria-label="On this page">\n\n</nav>', self.page("token-injection.html"))

    def test_where_the_links_of_a_document_go(self):
        manual = self.page("manual.html")
        for expected in (
            '<a href="threat-model.html">threat model</a>',  # another document: its page
            '<a href="threat-model.html#5-not-defended">not defended</a>',
            '<a href="emergency.html">runbook</a>',
            '<a href="https://repository.invalid/blob/main/LICENSE" rel="noopener noreferrer">licence</a>',  # a file
            '<a href="https://repository.invalid/tree/main/src" rel="noopener noreferrer">sources</a>',  # a directory
            '<a href="#status">status</a>',  # the page itself
            '<a href="https://example.org/display" rel="noopener noreferrer">https://example.org/display</a>',
        ):
            with self.subTest(link=expected):
                self.assertIn(expected, manual)
        threat_model = self.page("threat-model.html")
        self.assertIn('See the <a href="manual.html#status">status</a> and row <a href="#row-j1">J1</a>.', threat_model)
        self.assertIn('<tr role="row" id="row-j1"><td role="cell" data-label="#">J1</td>', threat_model)

    def test_the_documents_under_docs_link_to_each_others_pages(self):
        # (the manual's links to docs/PHONE.md, docs/COMPANION.md and docs/MODS.md land on the
        # site the same way, instead of in the repository)
        self.assertIn('It brings <a href="phone.html#the-apps">the phone</a>.', self.page("companion.html"))
        self.assertIn('Part of <a href="companion.html">the companion mod</a>.', self.page("phone.html"))
        self.assertIn('unlike <a href="phone.html#the-apps">the phone\'s apps</a>; see the <a href="threat-model.html">threat '
                      'model</a>.', self.page("bedrock.html"))
        self.assertIn('see the <a href="threat-model.html">threat model</a>.', self.page("mods.html"))
        self.edit("docs/COMPANION.md", "(PHONE.md#the-apps)", "(PHONE.md#no-such-section)")
        self.assertFails("companion.html: link 'phone.html#no-such-section': phone.html has no element with id "
                         "'no-such-section'")

    def test_repository_links_use_the_address_that_was_given(self):
        self.assertIn('<a href="%s/blob/main/LICENSE" rel="noopener noreferrer">licence</a>' % REPO,
                      self.page("manual.html", deploy=True))

    def test_links_that_stop_the_build(self):
        for target, message in (
            ("docs/NOT_THERE.md", "README.md:3: link target 'docs/NOT_THERE.md' does not exist in the repository"),
            ("../outside.md", "README.md:3: link target '../outside.md' leaves the repository"),
            ("docs/../../x", "README.md:3: link target 'docs/../../x' leaves the repository"),
            ("http://example.org/", "README.md:3: refused link target 'http://example.org/': only https:// addresses are allowed"),
            ("javascript:alert(1)", "README.md:3: refused link target 'javascript:alert(1)'"),
        ):
            with self.subTest(target=target):
                self.write("README.md", README.replace("(docs/THREAT_MODEL.md)", "(%s)" % target))
                self.assertStops(message)

    def test_a_document_has_exactly_one_title(self):
        self.write("docs/TOKEN_INJECTION.md", "Text first.\n\n# Token path\n")
        self.assertStops("docs/TOKEN_INJECTION.md: the document must begin with its one '# ' heading")
        self.write("docs/TOKEN_INJECTION.md", "# Token path\n\nText.\n\n# Another title\n")
        self.assertStops("docs/TOKEN_INJECTION.md:5: a second '# ' heading (a page has one)")
        self.write("docs/TOKEN_INJECTION.md", "# Token path\n\n### Too deep\n\nText.\n")
        self.assertStops("a document has a '###' heading before its first '##' heading")

    def test_markdown_the_converter_does_not_implement_stops_the_build_with_file_and_line(self):
        self.add("docs/TOKEN_INJECTION.md", "\n![a picture](picture.png)\n")
        self.assertEqual(self.assertStops(), "docs/TOKEN_INJECTION.md:7: images are not supported")
        status, out, err = self.run_main()
        self.assertEqual((status, out), (1, ""))
        self.assertEqual(err, "error: docs/TOKEN_INJECTION.md:7: images are not supported\n"
                              "error: nothing was written (1 problem(s) in the sources; --keep-going writes the pages "
                              "that can be built)\n")
        self.assertFalse(self.out.exists())

    def test_the_title_counts_among_the_headings_as_on_github(self):
        # GitHub numbers the title too: a section of the same name is "-1" (renderer-8), and
        # the title's own anchor leads to the top of the page, where the shell's <h1> has it.
        self.write("docs/MC_VAULT_ACCOUNT.md", "# A dedicated account\n\nSee [the section](#a-dedicated-account-1) and "
                   "[the top](#a-dedicated-account).\n\n## A dedicated account\n\nText.\n")
        self.assertEqual(self.check(), [])
        page = self.page("dedicated-account.html")
        self.assertIn('<h1 id="a-dedicated-account">A dedicated account</h1>', page)
        self.assertIn('<h2 id="a-dedicated-account-1">A dedicated account</h2>', page)
        self.assertEqual(page.count('id="a-dedicated-account"'), 1)
        # A shell that does not put the anchor on its title leaves such a link nothing to land
        # on, and the check says so instead of sending the reader to the section.
        self.edit("site/src/document.html", '<h1 id="{{TITLE_ID}}">', "<h1>")
        self.assertFails("dedicated-account.html: link '#a-dedicated-account': dedicated-account.html has no element with id "
                         "'a-dedicated-account'")

    def test_every_title_carries_its_anchor(self):
        for page, anchor in (("manual.html", "bastion-mc"), ("threat-model.html", "threat-model"),
                             ("token-injection.html", "token-path"), ("dedicated-account.html", "a-dedicated-account"),
                             ("emergency.html", "if-you-think-the-account-is-compromised")):
            with self.subTest(page=page):
                self.assertRegex(self.page(page), r'<h1 id="%s">[^<]' % anchor)

    def test_a_title_cannot_take_an_id_of_the_page_around_it(self):
        self.edit("docs/TOKEN_INJECTION.md", "# Token path", "# Site content")
        self.assertStops("docs/TOKEN_INJECTION.md:1: the title gets the anchor 'site-content' on GitHub, which the page "
                         "around the document uses already")

    def test_an_id_of_the_page_around_a_document_cannot_be_taken(self):
        # "## Site content" would be GitHub's #site-content, which the shell's <main> has.
        self.edit("docs/TOKEN_INJECTION.md", "## How it gets there", "## Site content\n\nText.\n\n## How it gets there")
        self.assertStops("docs/TOKEN_INJECTION.md:3: the heading 'Site content' gets the anchor 'site-content' on GitHub, "
                         "which the page around the document uses already")
        self.edit("docs/TOKEN_INJECTION.md", "## Site content", "## Site contents")
        self.assertEqual(self.check(), [])

    def test_a_heading_and_a_table_row_cannot_share_an_id(self):
        # Before, the second of the two was numbered silently (row-j1-1) or refused with the
        # wrong reason; GitHub numbers only headings, so neither can give way.
        self.edit("docs/THREAT_MODEL.md", "## 5. Not defended", "## Row J1\n\nText.\n\n## 5. Not defended")
        self.assertStops("docs/THREAT_MODEL.md:12: the heading 'Row J1' gets the anchor 'row-j1' on GitHub, which the "
                         "table row at line 9 has on this site")

    def test_a_document_that_cannot_be_read(self):
        self.remove("docs/MC_VAULT_ACCOUNT.md")
        self.assertStops("docs/MC_VAULT_ACCOUNT.md: ")
        (self.root / "docs/MC_VAULT_ACCOUNT.md").write_bytes(b"# Title\n\n\xff\xfe not UTF-8\n")
        self.assertStops("docs/MC_VAULT_ACCOUNT.md: ")


class TheEmergencyPage(SiteCase):
    def test_the_warning_comes_first_and_every_address_is_a_large_target(self):
        page = self.page("emergency.html")
        self.assertIn("<title>If you think the account is compromised | bastion-mc</title>", page)
        self.assertIn('<body class="page-emergency">', page)
        self.assertIn(
            '<main id="site-content">\n<h1 id="if-you-think-the-account-is-compromised">If you think the account is compromised</h1>\n'
            '<div class="callout danger">\n'
            '<h2 id="read-this-first-which-device-are-you-on">Read this first: which device are you on?</h2>\n'
            "<p>Do the account steps <strong>from a different device</strong> if this machine may be compromised.</p>\n"
            '</div>\n<article class="doc">\n<p>Two phases. The first is local.</p>\n'
            '<h2 id="phase-1--this-machine">Phase 1 — this machine</h2>\n'
            '<pre tabindex="0"><code class="language-sh">bastion-mc scram\n</code></pre>\n'
            '<h2 id="phase-2--the-account">Phase 2 — the account</h2>\n<ol>\n'
            "<li><strong>Change the password.</strong>\n"
            '<a class="tap" href="https://account.example.com/password" rel="noopener noreferrer">'
            '<span class="tap-what">Account: change the password</span> '
            '<span class="tap-where">https://account.example.com/password</span></a></li>\n'
            "<li><strong>Sign out everywhere</strong>: "
            '<a class="tap" href="https://account.example.com/sessions?all=1&amp;now=1" rel="noopener noreferrer">'
            '<span class="tap-what">Account: sign out &quot;everywhere&quot; &amp; more</span> '
            '<span class="tap-where">https://account.example.com/sessions?all=1&amp;now=1</span></a></li>\n'
            "</ol>\n</article>\n<p>Generated from <code>docs/SCRAM_RUNBOOK.md</code>; 2 addresses.</p>\n</main>",
            page,
        )

    def test_it_uses_the_footer_without_outbound_links(self):
        page = self.page("emergency.html")
        self.assertNotIn("repository.invalid", page)
        self.assertEqual(sorted(set(re.findall(r'<a [^>]*href="(https://[^"]*)"', page))),
                         [link.replace("&", "&amp;") for link in LINKS])

    def test_an_address_the_launcher_does_not_list_stops_the_build(self):
        self.add("docs/SCRAM_RUNBOOK.md", "3. Or ask: https://support.example.com/help\n")
        self.assertStops("docs/SCRAM_RUNBOOK.md: https://support.example.com/help is not in runbook::LINKS "
                         "(crates/proto/src/runbook.rs)")

    def test_an_address_written_as_a_link_is_held_to_the_list_by_the_check(self):
        # `[text](address)` is not drawn as a target, so the build does not stop; the check
        # compares every outbound link of the page with the list.
        self.add("docs/SCRAM_RUNBOOK.md", "3. Or [ask](https://support.example.com/help).\n")
        self.assertFails("emergency.html: its outbound links must be exactly runbook::LINKS; not in the list: "
                         "['https://support.example.com/help']; missing from the page: none")

    def test_the_runbook_must_carry_the_device_warning(self):
        self.edit("docs/SCRAM_RUNBOOK.md", "**from a different device**", "from another device")
        self.assertStops("docs/SCRAM_RUNBOOK.md: no section says 'from a different device'; the emergency page puts that "
                         "warning first")

    def test_its_headings_are_numbered_in_the_order_of_the_document(self):
        # The page shows the device warning first, but GitHub numbers the runbook's headings
        # in the order they stand in the file (renderer-8).
        self.edit("docs/SCRAM_RUNBOOK.md", "```sh\nbastion-mc scram\n```\n", "```sh\nbastion-mc scram\n```\n\n### Note\n\nOne.\n")
        self.edit("docs/SCRAM_RUNBOOK.md", "if this machine may be compromised.\n",
                  "if this machine may be compromised.\n\n### Note\n\nTwo.\n")
        page = self.page("emergency.html")
        self.assertIn('<h3 id="note-1">Note</h3>\n<p>Two.</p>\n</div>', page)
        self.assertIn('<h3 id="note">Note</h3>\n<p>One.</p>', page)
        self.assertLess(page.index('id="note-1"'), page.index('id="note"'))

    def test_the_runbook_needs_sections(self):
        self.write("docs/SCRAM_RUNBOOK.md", "# Runbook\n\nDo it from a different device.\n")
        self.assertStops("docs/SCRAM_RUNBOOK.md: the runbook has no '## ' sections")


class WholeBuild(SiteCase):
    def test_one_run_names_everything_that_is_wrong(self):
        self.remove("site/src/pages/security.html")
        self.remove("site/src/document.html")
        self.remove("site/src/robots.txt")
        self.edit("site/src/pages/index.html", "{{INCLUDE partials/aside.html}}", "{{INCLUDE partials/gone.html}}")
        with self.assertRaises(build.BuildErrors) as caught:
            self.site().build()
        self.assertEqual(caught.exception.problems, [
            "site/src/partials/gone.html does not exist",
            "site/src/pages/security.html does not exist",
            "site/src/document.html does not exist",  # (once, although it stops four pages)
            "site/src/robots.txt does not exist",
        ])
        status, out, err = self.run_main("--check")
        self.assertEqual((status, out), (1, ""))
        self.assertEqual(err.count("error: "), 5)
        self.assertTrue(err.endswith("error: nothing was written (4 problem(s) in the sources; --keep-going writes the "
                                     "pages that can be built)\n"), err)
        self.assertFalse(self.out.exists())

    def test_keep_going_writes_what_can_be_built_and_still_fails(self):
        self.remove("site/src/pages/security.html")
        self.remove("site/src/emergency.html")
        status, out, err = self.run_main("--keep-going")
        self.assertEqual((status, out), (1, ""))
        self.assertEqual(err, "error: site/src/pages/security.html does not exist\n"
                              "error: site/src/emergency.html does not exist\n"
                              "error: INCOMPLETE site in %s: 2 problem(s) in the sources (listed above as 'error:')\n" % self.out)
        self.assertTrue((self.out / "index.html").is_file())
        self.assertFalse((self.out / "security.html").exists())
        self.assertFalse((self.out / "emergency.html").exists())
        sitemap = (self.out / "sitemap.xml").read_text(encoding="utf-8")
        self.assertNotIn("security.html", sitemap)
        self.assertIn("manual.html", sitemap)

    def test_keep_going_with_check_reports_links_to_missing_pages_once_per_page(self):
        self.remove("site/src/pages/security.html")
        status, out, err = self.run_main("--keep-going", "--check")
        self.assertEqual((status, out), (1, ""))
        self.assertEqual(err, "error: site/src/pages/security.html does not exist\n"
                              "check failed: 12 link(s) to security.html could not be checked: that page was not built\n"
                              "error: INCOMPLETE site in %s: 1 problem(s) in the sources (listed above as 'error:')\n"
                              "error: 1 problem(s) in the generated site\n" % self.out)

    def test_a_site_without_the_emergency_page_fails_the_check(self):
        self.remove("site/src/emergency.html")
        status, out, err = self.run_main("--keep-going", "--check")
        self.assertEqual(status, 1)
        self.assertIn("check failed: emergency.html was not generated\n", err)

    def test_an_output_directory_is_only_emptied_if_the_generator_wrote_it(self):
        self.out.mkdir()
        (self.out / "precious.txt").write_text("not the generator's\n", encoding="utf-8")
        status, out, err = self.run_main()
        self.assertEqual((status, out), (1, ""))
        self.assertEqual(err, "error: %s is not empty and was not written by this generator; refusing to replace it\n" % self.out)
        self.assertEqual(sorted(path.name for path in self.out.iterdir()), ["precious.txt"])

    def test_its_own_output_is_replaced_completely(self):
        self.assertEqual(self.run_main()[0], 0)
        (self.out / "stale.html").write_text("left over from an earlier run\n", encoding="utf-8")
        (self.out / "assets" / "old.css").write_text("old\n", encoding="utf-8")
        self.assertEqual(self.run_main()[0], 0)
        self.assertFalse((self.out / "stale.html").exists())
        self.assertFalse((self.out / "assets" / "old.css").exists())
        self.assertTrue((self.out / "index.html").is_file())

    def test_an_empty_directory_is_used_and_a_file_or_a_link_is_not(self):
        self.out.mkdir()
        self.assertEqual(self.run_main()[0], 0)
        elsewhere = self.scratch / "file"
        elsewhere.write_text("a file\n", encoding="utf-8")
        with self.assertRaises(build.BuildError):
            build.write_output({"index.html": b""}, elsewhere)
        link = self.scratch / "link"
        os.symlink(str(self.out), str(link))
        with self.assertRaises(build.BuildError):
            build.write_output({"index.html": b""}, link)
        self.assertTrue((self.out / "index.html").stat().st_size > 0)

    def test_the_marker_file_is_hidden_so_that_it_is_not_published(self):
        # The deployment's upload step leaves out files whose names begin with a dot.
        self.assertTrue(build.MARKER.startswith("."))
        self.generate()
        self.assertTrue((self.out / build.MARKER).is_file())

    def test_only_style_sheets_vector_and_png_pictures_are_published(self):
        for name, message in (
            ("notes.txt", "site/src/assets/notes.txt: only .css, .svg and .png files are published"),
            ("script.js", "site/src/assets/script.js: only .css, .svg and .png files are published"),
            ("photo.jpg", "site/src/assets/photo.jpg: only .css, .svg and .png files are published"),
            (".DS_Store", "site/src/assets/.DS_Store: only plain files belong here"),
        ):
            with self.subTest(name=name):
                self.write("site/src/assets/" + name, "x\n")
                self.assertStops(message)
                self.remove("site/src/assets/" + name)
        (self.root / "site/src/assets/fonts").mkdir()
        self.assertStops("site/src/assets/fonts: only plain files belong here")


# --------------------------------------------------------------------------------------
# --check: every rule, broken once
# --------------------------------------------------------------------------------------


class CheckStructure(SiteCase):
    def test_doctype(self):
        self.edit("site/src/template.html", "<!doctype html>\n", "")
        self.assertFails("no <!doctype html>", pages=PAGES)

    def test_language(self):
        self.edit("site/src/template.html", '<html lang="en">', "<html>")
        self.assertFails('<html> needs lang="en"', pages=PAGES)

    def test_one_h1(self):
        self.edit("site/src/pages/security.html", "<h1>Security</h1>", "<h1>Security</h1>\n<h1>Again</h1>")
        self.assertFails("security.html: has 2 <h1> elements, a page has one")

    def test_no_h1(self):
        self.edit("site/src/pages/getting-started.html", "<h1>Get started</h1>\n", "")
        self.assertFails("getting-started.html: has 0 <h1> elements, a page has one",
                         "getting-started.html: the first heading is <h2>; a page begins with its <h1>")

    def test_heading_levels_are_not_skipped(self):
        self.edit("site/src/pages/security.html", "<h1>Security</h1>", "<h1>Security</h1>\n<h3>Too deep</h3>")
        self.assertFails("security.html: <h3> directly after <h1> skips a heading level")

    def test_going_back_up_any_number_of_levels_is_fine(self):
        self.add("site/src/pages/security.html", "<h2>Two</h2>\n<h3>Three</h3>\n<h4>Four</h4>\n<h2>Two again</h2>\n")
        self.assertEqual(self.check(), [])

    def test_title(self):
        self.edit("site/src/template.html", "<title>{{TITLE}}</title>", "<title> </title>")
        self.assertFails("empty <title>", pages=PAGES)

    def test_description(self):
        self.edit("site/src/template.html", '<meta name="description" content="{{DESCRIPTION}}">\n', "")
        self.assertFails("no description", pages=PAGES)

    def test_viewport(self):
        self.edit("site/src/template.html", '<meta name="viewport" content="width=device-width, initial-scale=1">\n', "")
        self.assertFails("no viewport meta element", pages=PAGES)

    def test_landmarks(self):
        self.edit("site/src/template.html", '<header class="site-header">', '<div class="site-header">')
        self.edit("site/src/template.html", "</header>", "</div>")
        self.assertFails("no <header> landmark", pages=PAGES)

    def test_navigation_landmark(self):
        self.edit("site/src/template.html", '<nav class="site-nav" aria-label="Site">', "<div>")
        self.edit("site/src/template.html", "</nav>", "</div>")
        # (the rendered documents have a second one, their table of contents)
        self.assertFails("no <nav> landmark", pages=("404.html", "emergency.html", "getting-started.html", "index.html", "security.html"))

    def test_main_landmark_and_the_skip_link(self):
        self.edit("site/src/template.html", '<main id="site-content">', '<div id="site-content">')
        self.edit("site/src/template.html", "</main>", "</div>")
        self.assertFails("no <main> landmark", "needs exactly one <main>, with an id for the skip link to point to", pages=PAGES)

    def test_main_needs_an_id(self):
        self.edit("site/src/template.html", '<main id="site-content">', "<main>")
        self.assertFails("needs exactly one <main>, with an id for the skip link to point to",
                         "link '#site-content': ", pages=PAGES)

    def test_the_first_link_is_the_skip_link(self):
        self.edit("site/src/template.html", '<a class="skip" href="#site-content">Skip to the content</a>\n', "")
        self.assertFails('the first link of the page must be the skip link to <main id="site-content">', pages=PAGES)

    def test_the_skip_link_points_to_main(self):
        self.edit("site/src/template.html", '<header class="site-header">', '<header class="site-header" id="top">')
        self.edit("site/src/template.html", 'href="#site-content"', 'href="#top"')
        self.assertFails('the first link of the page must be the skip link to <main id="site-content">', pages=PAGES)

    def test_duplicate_ids(self):
        self.add("site/src/pages/security.html", '<p id="twice">a</p>\n<p id="twice">b</p>\n<p id="site-content">c</p>\n')
        self.assertFails("security.html: duplicate id: site-content, twice")

    def test_an_included_heading_id_cannot_collide(self):
        # Two sections of documents on one page share one set of ids.
        self.add("site/src/pages/index.html", '{{PARAGRAPH README.md "Built and tested"}}\n')
        self.assertEqual(self.check(), [])

    def test_the_footer_carries_the_trademark_statement(self):
        self.edit("site/src/partials/footer.html", "Not an official Minecraft product. ", "")
        self.assertFails("the footer must carry the trademark statement (missing: 'not an official minecraft product')",
                         pages=[page for page in PAGES if page != "emergency.html"])

    def test_the_footer_names_mojang_and_microsoft(self):
        self.edit("site/src/partials/footer-minimal.html", "Not approved by or associated with Mojang or Microsoft.",
                  "Not approved by Mojang.")
        self.assertFails("emergency.html: the footer must carry the trademark statement (missing: 'not approved by or "
                         "associated with mojang or microsoft')")

    def test_the_statement_must_stand_in_the_footer_itself(self):
        self.edit("site/src/partials/footer.html", '<footer class="site-footer">', '<div class="site-footer">')
        self.edit("site/src/partials/footer.html", "</footer>", "</div>")
        self.assertFails("no <footer> landmark", "the footer must carry the trademark statement",
                         "the footer must say what the host of the site necessarily sees",
                         pages=[page for page in PAGES if page != "emergency.html"])

    def test_the_footer_says_what_the_host_sees(self):
        self.edit("site/src/partials/footer.html", "necessarily sees your IP address", "sees something")
        self.assertFails("the footer must say what the host of the site necessarily sees (missing: 'ip address')",
                         pages=[page for page in PAGES if page != "emergency.html"])

    def test_the_statements_may_be_wrapped_and_capitalised_differently(self):
        self.edit("site/src/partials/footer.html", "Not an official Minecraft product.", "NOT an official\n   Minecraft   product.")
        self.assertEqual(self.check(), [])

    def test_images_need_a_text_and_their_size(self):
        self.add("site/src/pages/security.html", '<p><img src="assets/mark.svg"></p>\n')
        self.assertFails("security.html: <img src='assets/mark.svg'> without alt", "<img src='assets/mark.svg'> without width",
                         "<img src='assets/mark.svg'> without height",
                         "<img src='assets/mark.svg'> needs its intrinsic width and height")

    def test_an_image_size_is_a_number_of_pixels(self):
        self.add("site/src/pages/security.html", '<p><img src="assets/mark.svg" alt="" width="100%" height="auto"></p>\n')
        self.assertFails("security.html: <img src='assets/mark.svg'> needs its intrinsic width and height")

    def test_an_empty_text_is_allowed_for_decoration(self):
        self.add("site/src/pages/security.html", '<p><img src="assets/mark.svg" alt="" width="8" height="8"></p>\n')
        self.assertEqual(self.check(), [])


    def test_an_image_keeps_the_proportions_of_its_picture(self):
        # (design request 4: the stacked drawing grew from 1422 to 1438 px and its two img
        # elements kept the old height, which nothing noticed)
        for markup in ('<img src="assets/shot-one.png" alt="x" width="4" height="3">',  # (its own size)
                       '<img src="assets/shot-one.png" alt="x" width="8" height="6">',  # (drawn larger)
                       '<img src="assets/shot-one.png" alt="x" width="3" height="2">',  # (rounded)
                       '<img src="assets/mark.svg" alt="" width="28" height="28">'):  # (an 8 x 8 drawing, smaller)
            with self.subTest(markup=markup):
                self.write("site/src/pages/security.html", SECURITY + "<p>%s</p>\n" % markup)
                self.assertEqual(self.check(), [])
        self.write("site/src/pages/security.html", SECURITY + '<p><img src="assets/shot-one.png" alt="x" width="4" height="4"></p>\n')
        self.assertFails("security.html: <img src='assets/shot-one.png' width='4' height='4'>: the picture is 4 x 3; an "
                         "img's width and height must have its proportions")
        self.write("site/src/pages/security.html", SECURITY + '<p><img src="assets/mark.svg" alt="" width="16" height="8"></p>\n')
        self.assertFails("security.html: <img src='assets/mark.svg' width='16' height='8'>: the picture is 8 x 8")

    def test_the_size_of_a_drawing_comes_from_its_view_box_when_it_gives_none_else(self):
        self.edit("site/src/assets/mark.svg", ' width="8" height="8"', "")
        self.write("site/src/pages/security.html", SECURITY + '<p><img src="assets/mark.svg" alt="" width="16" height="16"></p>\n')
        self.assertEqual(self.check(), [])
        self.edit("site/src/assets/mark.svg", ' viewBox="0 0 8 8"', "")
        self.assertFails("security.html: <img src='assets/mark.svg'>: the picture gives no size (width and height, or a "
                         "viewBox)")


class CheckMarkup(SiteCase):
    """What the reader of the pages itself refuses, wherever it stands."""

    CASES = (
        ("<!-- a note to myself -->", "HTML comment in the output"),
        ("<script>alert(1)</script>", "<script> is not allowed on this site"),
        ('<script src="assets/x.js"></script>', "<script> is not allowed on this site"),
        ("<style>p { color: red }</style>", "<style> is not allowed on this site"),
        ('<iframe src="index.html"></iframe>', "<iframe> is not allowed on this site"),
        ('<object data="assets/mark.svg"></object>', "<object> is not allowed on this site"),
        ('<embed src="assets/mark.svg">', "<embed> is not allowed on this site"),
        ('<video src="assets/x.webm"></video>', "<video> is not allowed on this site"),
        ('<audio src="assets/x.ogg"></audio>', "<audio> is not allowed on this site"),
        ('<form action="https://example.org/"><p>x</p></form>', "<form> is not allowed on this site"),
        ('<p><input name="q"></p>', "<input> is not allowed on this site"),
        ("<p><button>Go</button></p>", "<button> is not allowed on this site"),
        ('<base href="https://example.org/">', "<base> is not allowed on this site"),
        ("<noscript><p>x</p></noscript>", "<noscript> is not allowed on this site"),
        ("<template><p>x</p></template>", "<template> is not allowed on this site"),
        ("<canvas></canvas>", "<canvas> is not allowed on this site"),
        ('<p><image src="https://example.org/x.png"></p>', "<image> outside SVG (a browser reads it as <img>"),
        ('<map name="m"><area href="https://example.org/" alt="x"></map>', "<map> is not allowed on this site",
         "<area> is not allowed on this site"),
        ('<math><mi href="https://example.org/">x</mi></math>', "<math> is not allowed on this site"),
        ('<p onclick="alert(1)">x</p>', "event handler attribute onclick"),
        ('<p style="color: red">x</p>', "style attribute (the Content-Security-Policy forbids inline styles)"),
        # (web-2: a block that is wider than its column scrolls, and every one may be)
        ("<pre><code>x</code></pre>", '<pre> without tabindex="0" (a code block wider than its column scrolls'),
        ('<pre tabindex="-1"><code>x</code></pre>', '<pre> without tabindex="0"'),
        ('<p><img src="assets/mark.svg" srcset="https://example.org/x.png 2x" alt="" width="8" height="8"></p>',
         "attribute srcset is not allowed on this site"),
        ('<p><a href="index.html" ping="https://example.org/ping">x</a></p>', "attribute ping is not allowed on this site"),
        ('<p><a href="index.html" attributionsrc="https://example.org/a">x</a></p>',
         "attribute attributionsrc is not allowed on this site"),
        ('<table background="https://example.org/x.png"><tr><td>x</td></tr></table>',
         "attribute background is not allowed on this site"),
        ('<link rel="preload" href="assets/site.css" as="style">', '<link rel="preload"> is not allowed on this site'),
        ('<link rel="stylesheet preload" href="assets/site.css">', '<link rel="stylesheet preload"> is not allowed on this site'),
        ('<link rel="dns-prefetch" href="https://example.org/">', '<link rel="dns-prefetch"> is not allowed on this site'),
        ('<link href="https://example.org/x.css">', '<link rel=""> is not allowed on this site'),
        ('<p><a name="anchor">x</a></p>', "<a> without href"),
        ('<p><a href="index.html" target="_blank">x</a></p>', "<a target> (links open where the reader decides)"),
        ('<p><a href="index.html">x <a href="manual.html">y</a></a></p>', "<a> inside <a>"),
        ("<p>text <div>block</div></p>", "<div> inside <p> (a browser would end the paragraph there)"),
        ("<p>one<p>two</p></p>", "<p> inside <p> (a browser would end the paragraph there)"),
        ("<ul><li>never closed</ul>", "</ul> does not close <li>"),
        ("<p>stray</div></p>", "</div> does not close <p>"),
        ("<p>line<br></br>break</p>", "</br> for an element that has no end tag"),
        ('<p class="a" class="b">x</p>', "<p> repeats an attribute"),
        ('<svg viewBox="0 0 1 1"><use href="assets/mark.svg#x"/></svg>', "<use> inside inline SVG could load a resource",
         "href on <use> inside inline SVG"),
        ('<svg viewBox="0 0 1 1"><image href="https://example.org/x.png"/></svg>', "<image> inside inline SVG could load a resource",
         "href on <image> inside inline SVG"),
        ('<svg viewBox="0 0 1 1"><feImage xlink:href="https://example.org/x.png"/></svg>',
         "<feimage> inside inline SVG could load a resource", "xlink:href on <feimage> inside inline SVG"),
        ('<svg viewBox="0 0 1 1"><rect fill="url(https://example.org/p.svg#a)" width="1" height="1"/></svg>',
         "attribute fill on <rect> inside inline SVG: url(…) that names something outside the file"),
        ('<svg viewBox="0 0 1 1"><rect filter="\\75rl(x.svg#f)" width="1" height="1"/></svg>',
         "attribute filter on <rect> inside inline SVG: a backslash"),
        # An animation gives the link an address while the page is shown: the link checks
        # (and on the emergency page the allowlist) only ever see href="index.html".
        ('<svg viewBox="0 0 1 1"><a href="index.html"><set attributeName="href" to="https://example.org/"/><text>x</text>'
         '</a></svg>', "<set> inside inline SVG (an animation can set an address that no check reads"),
        ('<svg viewBox="0 0 1 1"><rect width="1" height="1"><animate attributeName="opacity" values="1;0;1" dur="2s" '
         'repeatCount="indefinite"/></rect></svg>', "<animate> inside inline SVG (an animation"),
    )

    def test_each_is_reported_on_a_page_and_in_the_shell(self):
        for case in self.CASES:
            markup, messages = case[0], case[1:]
            with self.subTest(markup=markup, where="page"):
                self.write("site/src/pages/security.html", SECURITY + markup + "\n")
                self.assertFails(*messages, pages=["security.html"])
            self.write("site/src/pages/security.html", SECURITY)
            with self.subTest(markup=markup, where="shell"):
                self.write("site/src/template.html", TEMPLATE.replace("{{CONTENT}}\n", "{{CONTENT}}\n" + markup + "\n"))
                self.assertFails(*messages, pages=PAGES)
            self.write("site/src/template.html", TEMPLATE)

    def test_the_lists_of_what_is_forbidden(self):
        # Written out once more here, so that a name cannot leave a list unnoticed.
        self.assertEqual(sorted(build.FORBIDDEN_ELEMENTS), (
            "applet area audio base button canvas embed fencedframe form frame frameset iframe input map math noscript "
            "object portal script select source style template textarea track video").split())
        self.assertEqual(sorted(build.FORBIDDEN_ATTRIBUTES),
                         "attributionsrc background dynsrc formaction imagesrcset lowsrc manifest ping poster srcset".split())
        self.assertEqual(sorted(build.SVG_REFERENCES), "feimage image use".split())
        self.assertEqual(sorted(build.SVG_ANIMATION),
                         "animate animatecolor animatemotion animatetransform discard mpath set".split())

    def test_every_svg_animation_element_is_reported(self):
        for element in sorted(build.SVG_ANIMATION):
            with self.subTest(element=element):
                markup = '<svg viewBox="0 0 1 1"><%s/></svg>' % element
                self.write("site/src/pages/security.html", SECURITY + markup + "\n")
                line = self.page("security.html").split("\n").index(markup) + 1
                self.assertEqual(self.check(), [
                    "security.html:%d: <%s> inside inline SVG (an animation can set an address that no check reads, "
                    "and it moves whatever the reader's reduced-motion setting)" % (line, element)])
                self.write("site/src/assets/mark.svg", MARK.replace("</svg>", "<%s/></svg>" % element))
                self.write("site/src/pages/security.html", SECURITY)
                self.assertEqual(self.check(), [
                    "assets/mark.svg: <%s> is not allowed in a published SVG (an animation can set an address that no "
                    "check reads, and it moves whatever the reader's reduced-motion setting)" % element])
                self.write("site/src/assets/mark.svg", MARK)

    def test_every_forbidden_element_is_reported(self):
        for element in sorted(build.FORBIDDEN_ELEMENTS):
            with self.subTest(element=element):
                markup = "<%s>" % element if element in build.VOID else "<%s></%s>" % (element, element)
                self.write("site/src/pages/security.html", SECURITY + markup + "\n")
                line = self.page("security.html").split("\n").index(markup) + 1
                self.assertEqual(self.check(), ["security.html:%d: <%s> is not allowed on this site" % (line, element)])

    def test_every_forbidden_attribute_is_reported(self):
        for attribute in sorted(build.FORBIDDEN_ATTRIBUTES):
            with self.subTest(attribute=attribute):
                markup = '<p %s="index.html">x</p>' % attribute
                self.write("site/src/pages/security.html", SECURITY + markup + "\n")
                line = self.page("security.html").split("\n").index(markup) + 1
                self.assertEqual(self.check(), ["security.html:%d: attribute %s is not allowed on this site" % (line, attribute)])

    def test_an_unclosed_element_is_reported(self):
        self.add("site/src/pages/security.html", "<div><p>never closed\n")
        self.assertFails("security.html:", pages=["security.html"])
        self.assertTrue(any("does not close <p>" in problem for problem in self.check()))

    def test_a_page_that_ends_inside_its_elements(self):
        self.edit("site/src/template.html", "</body>\n</html>\n", "")
        self.assertFails("unclosed <html>, <body>", pages=PAGES)
        self.assertEqual(self.check()[0], "404.html: unclosed <html>, <body>")

    def test_inline_svg_that_refers_only_to_itself_is_fine(self):
        self.add("site/src/pages/security.html",
                 '<svg viewBox="0 0 4 4" role="img" aria-label="A square"><defs><linearGradient id="g"><stop offset="0" '
                 'stop-color="#fff"/></linearGradient><clipPath id="c"><rect width="4" height="4"/></clipPath></defs>'
                 '<rect width="4" height="4" fill="url(#g)" clip-path="url(#c)"/><a href="index.html"><text>x</text></a></svg>\n')
        self.assertEqual(self.check(), [])

    def test_a_problem_names_the_page_and_the_line(self):
        self.add("site/src/pages/security.html", "<p>fine</p>\n<p onclick=\"x()\">not fine</p>\n")
        problems = self.check()
        self.assertEqual(len(problems), 1)
        line = self.page("security.html").split("\n").index('<p onclick="x()">not fine</p>') + 1
        self.assertEqual(problems[0], "security.html:%d: event handler attribute onclick" % line)


class CheckPolicy(SiteCase):
    POLICY = '<meta http-equiv="Content-Security-Policy" content="{{CSP}}">\n'
    MESSAGE = ("must carry exactly this Content-Security-Policy: default-src 'none'; style-src 'self'; img-src 'self'; "
               "base-uri 'none'; form-action 'none'")

    def test_the_policy_itself(self):
        self.assertEqual(build.CSP, "default-src 'none'; style-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'")
        self.assertIn('<meta http-equiv="Content-Security-Policy" content="default-src &#x27;none&#x27;; style-src &#x27;self&#x27;; '
                      'img-src &#x27;self&#x27;; base-uri &#x27;none&#x27;; form-action &#x27;none&#x27;">', self.page("index.html"))

    def test_a_page_without_the_policy(self):
        self.edit("site/src/template.html", self.POLICY, "")
        self.assertFails(self.MESSAGE, pages=PAGES)

    def test_a_weaker_policy(self):
        for weaker in ("default-src 'self'", "default-src 'none'; style-src 'self' 'unsafe-inline'; img-src 'self'; "
                       "base-uri 'none'; form-action 'none'", "default-src 'none'; script-src 'self'; style-src 'self'; "
                       "img-src 'self'; base-uri 'none'; form-action 'none'", ""):
            with self.subTest(policy=weaker):
                self.write("site/src/template.html", TEMPLATE.replace('content="{{CSP}}"', 'content="%s"' % weaker))
                self.assertFails(self.MESSAGE, pages=PAGES)

    def test_two_policies(self):
        self.edit("site/src/template.html", self.POLICY, self.POLICY + self.POLICY)
        self.assertFails(self.MESSAGE, pages=PAGES)

    def test_a_policy_outside_head_is_ignored_by_a_browser(self):
        self.edit("site/src/template.html", self.POLICY, "")
        self.edit("site/src/template.html", '<body class="{{BODY_CLASS}}">\n', '<body class="{{BODY_CLASS}}">\n' + self.POLICY)
        self.assertFails("the Content-Security-Policy must stand in <head> (a browser ignores it anywhere else)", pages=PAGES)

    def test_the_policy_comes_before_anything_is_loaded(self):
        self.edit("site/src/template.html", self.POLICY, "")
        self.edit("site/src/template.html", "</head>", self.POLICY + "</head>")
        self.assertFails("the Content-Security-Policy must come before the first thing the page loads", pages=PAGES)

    def test_no_other_http_equiv(self):
        self.edit("site/src/template.html", "</head>", '<meta http-equiv="refresh" content="0; url=https://example.org/">\n</head>')
        self.assertFails('<meta http-equiv="refresh"> is not allowed', pages=PAGES)

    def test_the_referrer_policy(self):
        referrer = '<meta name="referrer" content="no-referrer">\n'
        message = 'must carry exactly one <meta name="referrer" content="no-referrer">, in <head>'
        for replacement in ("", '<meta name="referrer" content="origin">\n', referrer + referrer,
                            '<meta name="referrer" content="no-referrer, strict-origin">\n'):
            with self.subTest(replacement=replacement):
                self.write("site/src/template.html", TEMPLATE.replace(referrer, replacement))
                self.assertFails(message, pages=PAGES)
        self.write("site/src/template.html", TEMPLATE.replace(referrer, "").replace("<body", referrer + "<body").replace(
            referrer + "<body", "<body") .replace('<a class="skip"', referrer + '<a class="skip"'))
        self.assertFails(message, pages=PAGES)


class CheckResources(SiteCase):
    def test_nothing_is_loaded_from_another_origin(self):
        for markup, message in (
            ('<link rel="stylesheet" href="https://cdn.example.org/site.css">',
             "stylesheet 'https://cdn.example.org/site.css' would be loaded from another origin"),
            ('<link rel="stylesheet" href="//cdn.example.org/site.css">',
             "stylesheet '//cdn.example.org/site.css' would be loaded from another origin"),
            ('<link rel="icon" href="http://example.org/favicon.ico">',
             "icon 'http://example.org/favicon.ico' would be loaded from another origin"),
            ('<link rel="stylesheet" href="/site.css">', "stylesheet '/site.css' would be loaded from another origin"),
            ('<link rel="icon" href="data:image/svg+xml,%3Csvg/%3E">', "would be loaded from another origin"),
        ):
            with self.subTest(markup=markup):
                self.write("site/src/template.html", TEMPLATE.replace("</head>", markup + "\n</head>"))
                self.assertFails(message, pages=PAGES)

    def test_no_image_from_another_origin(self):
        for source in ("https://example.org/pixel.png", "//example.org/pixel.png", "http://example.org/pixel.png",
                       "data:image/png;base64,AAAA", "/assets/mark.svg", "\\\\example.org\\pixel.png"):
            with self.subTest(source=source):
                self.write("site/src/pages/security.html",
                           SECURITY + '<p><img src="%s" alt="" width="1" height="1"></p>\n' % source)
                self.assertFails("security.html: image %r would be loaded from another origin" % source)

    def test_what_a_page_loads_must_exist(self):
        self.add("site/src/pages/security.html", '<p><img src="assets/missing.png" alt="" width="1" height="1"></p>\n')
        self.assertFails("security.html: image 'assets/missing.png' does not exist in the output")

    def test_the_style_sheet_must_exist(self):
        self.edit("site/src/template.html", "assets/site.css", "assets/style.css")
        self.assertFails("stylesheet 'assets/style.css' does not exist in the output",
                         "assets/site.css is published but no page uses it")

    def test_an_asset_nobody_uses_is_not_published(self):
        self.write("site/src/assets/shot-unused.png", png())
        self.write("site/src/assets/old.svg", MARK)
        self.assertFails("assets/shot-unused.png is published but no page uses it",
                         "assets/old.svg is published but no page uses it")

    def test_the_canonical_address_is_under_the_site_address(self):
        self.edit("site/src/template.html", "{{HEAD}}", '<link rel="canonical" href="https://elsewhere.example.org/">')
        self.assertFails("canonical address 'https://elsewhere.example.org/' is not under the site address", pages=PAGES)

    def test_the_style_sheet_loads_nothing(self):
        for css, found in (
            ("body { background: url(pixel.png); }", "'url('"),
            ("body { background: URL( 'https://example.org/x.png' ); }", "'URL('"),
            ('@import "other.css";', "'@import'"),
            ("@IMPORT 'other.css';", "'@IMPORT'"),
            ("/* see https://example.org/ */ body { margin: 0; }", "'https:'"),
            ("body { background: image-set('//example.org/x.png' 1x); }", "'//e'"),
            ("a { width: expression(alert(1)); }", "'expression('"),
            ("@font-face { font-family: x; src: url(font.woff2); }", "'url('"),
        ):
            with self.subTest(css=css):
                self.write("site/src/assets/site.css", css + "\n")
                self.assertFails("assets/site.css: %s could make the style sheet load something" % found)

    def test_the_style_sheet_has_no_escapes_that_could_hide_a_load(self):
        self.write("site/src/assets/site.css", "body { background: \\75rl(pixel.png); }\n")
        self.assertFails("assets/site.css: a backslash (a CSS escape can spell url( where this check does not see it; "
                         "write the character itself)")

    def test_the_style_sheet_is_utf_8(self):
        (self.root / "site/src/assets/site.css").write_bytes(b"body { margin: 0; } /* \xff */\n")
        self.assertFails("assets/site.css: not UTF-8")

    def test_a_scrollbar_thumb_needs_three_to_one_against_its_track(self):
        # (design request 3, after web-1: the thumbs had 2.12:1 and 2.06:1, and the pair was
        # in nobody's list)
        for css in ("pre { scrollbar-color: #33485a #05080a; }",
                    ":root { --a: #33485a; --b: #05080a; }\npre { scrollbar-color: var(--a) var(--b); }",
                    ":root { --a: #33485a; --b: #05080a; }\npre { scrollbar-color:\n  var(--a)  var(--b) }"):
            with self.subTest(css=css):
                self.write("site/src/assets/site.css", css + "\n")
                self.assertFails("assets/site.css: scrollbar-color ", "the thumb #33485a has 2.12:1 against its track #05080a")
        for css in ("pre { scrollbar-color: #556c7e #05080a; }",  # (3.67:1)
                    ":root { --edge: #556c7e; --well: #05080a; }\npre { scrollbar-color: var(--edge) var(--well); }",
                    "pre { scrollbar-color: #fff #000; scrollbar-width: thin; }",  # (#rgb)
                    "pre { scrollbar-color: auto; }",
                    "/* pre { scrollbar-color: #33485a #05080a; } */ body { margin: 0; }",
                    ":root { --a: #556c7e; --b: #05080a; }\n@media print { :root { --a: #050809; } }\n"
                    "pre { scrollbar-color: var(--a) var(--b); }"):  # (a print rule: nothing scrolls on paper)
            with self.subTest(css=css):
                self.write("site/src/assets/site.css", css + "\n")
                self.assertEqual(self.check(), [])
        for value in ("var(--missing) #05080a", "red blue", "#556c7e", "rgb(0 0 0) #fff"):
            with self.subTest(value=value):
                self.write("site/src/assets/site.css", "pre { scrollbar-color: %s; }\n" % value)
                self.assertFails("assets/site.css: scrollbar-color %r: write two colours as #rrggbb" % value)

    def test_contrast_ratio(self):
        self.assertAlmostEqual(build.contrast_ratio("#000000", "#ffffff"), 21.0)
        self.assertAlmostEqual(build.contrast_ratio("#ffffff", "#ffffff"), 1.0)
        self.assertEqual(round(build.contrast_ratio("#556c7e", "#05080a"), 2), 3.67)
        self.assertEqual(round(build.contrast_ratio("#0a0d10", "#556c7e"), 2), 3.56)

    def test_a_published_svg_can_do_nothing_when_it_is_opened_by_itself(self):
        head = '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 8 8" width="8" height="8">'
        for body, messages in (
            ("<script>alert(1)</script>", ["assets/mark.svg: <script> is not allowed in a published SVG"]),
            ("<style>path { fill: red }</style>", ["assets/mark.svg: <style> is not allowed in a published SVG"]),
            ('<image href="https://example.org/x.png" width="8" height="8"/>',
             ["assets/mark.svg: <image> is not allowed in a published SVG", "assets/mark.svg: attribute href is not allowed in a published SVG"]),
            ('<use xlink:href="other.svg#shape"/>',
             ["assets/mark.svg: <use> is not allowed in a published SVG", "assets/mark.svg: attribute href is not allowed in a published SVG"]),
            ('<foreignObject width="8" height="8"><p xmlns="http://www.w3.org/1999/xhtml">x</p></foreignObject>',
             ["assets/mark.svg: <foreignobject> is not allowed in a published SVG"]),
            ('<a><path d="M0 0h8v8H0z"/></a>', ["assets/mark.svg: <a> is not allowed in a published SVG"]),
            ('<path d="M0 0h8v8H0z" onclick="alert(1)"/>', ["assets/mark.svg: attribute onclick is not allowed in a published SVG"]),
            ('<path d="M0 0h8v8H0z" style="fill: red"/>', ["assets/mark.svg: attribute style is not allowed in a published SVG"]),
            ('<path d="M0 0h8v8H0z" fill="url(https://example.org/paint.svg#p)"/>',
             ["assets/mark.svg: attribute fill: url(…) that names something outside the file"]),
            ('<path d="M0 0h8v8H0z" filter="url(other.svg#f)"/>',
             ["assets/mark.svg: attribute filter: url(…) that names something outside the file"]),
            ('<path d="M0 0h8v8H0z" mask="url( \'//example.org/m.svg#m\' )"/>',
             ["assets/mark.svg: attribute mask: url(…) that names something outside the file"]),
            ('<path d="M0 0h8v8H0z" fill="\\75rl(x.svg#p)"/>', ["assets/mark.svg: attribute fill: a backslash"]),
            # No href attribute anywhere, and still a fetch when the file is opened by itself:
            # the animation gives the filter's picture its address at run time.
            ('<filter id="f"><feImage width="8" height="8"><set attributeName="href" to="https://example.org/x.png"/>'
             '</feImage></filter><path d="M0 0h8v8H0z" filter="url(#f)"/>',
             ["assets/mark.svg: <feimage> is not allowed in a published SVG",
              "assets/mark.svg: <set> is not allowed in a published SVG (an animation can set an address"]),
            ('<path d="M0 0h8v8H0z"><animateTransform attributeName="transform" type="rotate" from="0" to="360" dur="4s" '
             'repeatCount="indefinite"/></path>',
             ["assets/mark.svg: <animatetransform> is not allowed in a published SVG (an animation"]),
        ):
            with self.subTest(body=body):
                self.write("site/src/assets/mark.svg", head + body + "</svg>\n")
                self.assertFails(*messages)

    def test_a_published_svg_has_no_instructions_and_no_document_type(self):
        self.write("site/src/assets/mark.svg", '<?xml version="1.0"?>\n<?xml-stylesheet href="https://example.org/x.css"?>\n' + MARK)
        self.assertFails("assets/mark.svg: a processing instruction is not allowed in a published SVG "
                         "(xml-stylesheet would load a style sheet)")
        # An entity can hold an element that is nowhere in the file as an element. Whether
        # the XML parser of this Python expands it is the parser's business: the
        # declaration alone is refused.
        declaration = ("assets/mark.svg: a document type declaration is not allowed in a published SVG (it can define "
                       "entities that this check does not read)")
        self.write("site/src/assets/mark.svg",
                   '<!DOCTYPE svg [<!ENTITY hidden "<script xmlns=\'http://www.w3.org/2000/svg\'>alert(1)</script>">]>\n'
                   + MARK.replace("<title>mark</title>", "<title>mark</title>&hidden;"))
        problems = self.check()
        self.assertIn(declaration, problems)
        self.assertLessEqual(set(problems), {declaration, "assets/mark.svg: <script> is not allowed in a published SVG"})
        # (the line that drawing programs put at the top of what they export)
        self.write("site/src/assets/mark.svg", '<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" '
                                               '"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">\n' + MARK)
        self.assertEqual(self.check(), [declaration])

    def test_a_published_svg_is_well_formed(self):
        self.write("site/src/assets/mark.svg", MARK.replace("</svg>", ""))
        self.assertFails("assets/mark.svg: not well-formed XML: ")

    def test_an_svg_with_a_declaration_and_references_to_itself_is_fine(self):
        self.write("site/src/assets/mark.svg",
                   '<?xml version="1.0" encoding="UTF-8"?>\n<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8" width="8" height="8">'
                   '<defs><linearGradient id="g"><stop offset="0" stop-color="#3fd9e6"/></linearGradient>'
                   '<clipPath id="c"><path d="M0 0h8v8H0z"/></clipPath></defs>'
                   '<path d="M0 0h8v8H0z" fill="url(#g)" clip-path="url( #c )" stroke="URL(\'#g\')"/></svg>\n')
        self.assertEqual(self.check(), [])

    def test_a_picture_is_only_its_image_data(self):
        for name, data, message in (
            ("a text chunk", png(extra=[(b"tEXt", b"Software\x00made on some machine")]),
             "assets/shot-one.png: the PNG file carries a tEXt chunk (text, a time or camera data about how it was made)"),
            ("international text", png(extra=[(b"iTXt", b"Author\x00\x00\x00\x00\x00somebody")]), "carries a iTXt chunk"),
            ("compressed text", png(extra=[(b"zTXt", b"Comment\x00\x00" + zlib.compress(b"x"))]), "carries a zTXt chunk"),
            ("a time", png(extra=[(b"tIME", struct.pack(">HBBBBB", 2026, 11, 13, 21, 0, 0))]), "carries a tIME chunk"),
            ("camera data", png(extra=[(b"eXIf", b"MM\x00\x2a\x00\x00\x00\x08\x00\x00")]), "carries a eXIf chunk"),
            ("bytes after the end", png(after=b"hidden"), "assets/shot-one.png: 6 byte(s) after the end of the PNG picture"),
            ("cut off", png()[:-5], "assets/shot-one.png: the PNG file is cut off"),
            ("cut off inside a chunk", png()[:50], "assets/shot-one.png: the PNG file is cut off inside its IDAT chunk"),
            ("a wrong checksum", png()[:-1] + b"\x00", "assets/shot-one.png: the IEND chunk of the PNG file has a wrong checksum"),
            ("an end chunk with something in it", png(end=b"hidden"), "assets/shot-one.png: the IEND chunk of the PNG file is not empty"),
            ("a chunk without a name", png(extra=[(b"\x00\x00\x00\x00", b"")]),
             "assets/shot-one.png: the PNG file has a chunk whose name is not four letters (bytes 00000000)"),
            ("a chunk with a name that is not ASCII", png(extra=[("täx".encode("latin-1") + b"t", b"")]),
             "assets/shot-one.png: the PNG file has a chunk whose name is not four letters (bytes 74e47874)"),
        ):
            with self.subTest(case=name):
                self.write("site/src/assets/shot-one.png", data)
                problems = self.assertFails(message)
                self.assertTrue(all(character.isprintable() for problem in problems for character in problem), problems)

    def test_a_picture_has_a_size(self):
        # (found by the deploy role: the size goes into the page, and a browser shows nothing)
        for width, height in ((0, 3), (4, 0), (0, 0)):
            with self.subTest(size=(width, height)):
                self.write("site/src/assets/shot-one.png", png(width, height))
                self.assertStops("shot-one.png: a PNG picture without a size (%d x %d)" % (width, height))
                self.assertEqual(build.png_problem(png(width, height)),
                                 "the header of the PNG file gives no size (IHDR is 13 bytes, width and height above zero)")
        longer = png(header=struct.pack(">IIBBBBBB", 4, 3, 8, 2, 0, 0, 0, 0))
        self.write("site/src/assets/shot-one.png", longer)
        self.assertStops("shot-one.png is not a PNG file")

    def test_a_picture_that_a_page_shows_by_hand_is_checked_like_the_others(self):
        # {{SHOT}} reads the size itself and stops the build; an <img> written by hand
        # says its own size, so only the check sees the file.
        self.add("site/src/pages/security.html", '<p><img src="assets/drawing.png" alt="A drawing." width="4" height="3"></p>\n')
        self.write("site/src/assets/drawing.png", png())
        self.assertEqual(self.check(), [])
        for data, message in (
            (png(header=struct.pack(">IIBBBBBB", 4, 3, 8, 2, 0, 0, 0, 0)),
             "the header of the PNG file gives no size (IHDR is 13 bytes, width and height above zero)"),
            (png(0, 0), "the header of the PNG file gives no size (IHDR is 13 bytes, width and height above zero)"),
            (b"GIF89a" + b"\x00" * 30, "not a PNG file"),
            (png(extra=[(b"tEXt", b"Author\x00somebody")]),
             "the PNG file carries a tEXt chunk (text, a time or camera data about how it was made)"),
        ):
            with self.subTest(message=message):
                self.write("site/src/assets/drawing.png", data)
                self.assertEqual(self.check(), ["assets/drawing.png: " + message])

    def test_chunks_that_describe_the_picture_are_fine(self):
        self.write("site/src/assets/shot-one.png", png(extra=[(b"pHYs", struct.pack(">IIB", 3780, 3780, 1)), (b"sRGB", b"\x00")]))
        self.assertEqual(self.check(), [])

    def test_png_problem(self):
        self.assertIsNone(build.png_problem(png()))
        self.assertEqual(build.png_problem(b""), "not a PNG file")
        self.assertEqual(build.png_problem(b"<svg/>"), "not a PNG file")
        self.assertEqual(build.png_problem(b"\x89PNG\r\n\x1a\n"), "not a complete PNG file (IHDR first, image data, IEND last)")
        without_data = png().replace(b"IDAT", b"iDAT")
        self.assertIn("checksum", build.png_problem(without_data))


class CheckLinks(SiteCase):
    def link(self, markup):
        self.write("site/src/pages/security.html", SECURITY + "<p>%s</p>\n" % markup)

    def test_outbound_links_need_rel(self):
        for rel in ("", ' rel="noopener"', ' rel="noreferrer"', ' rel="nofollow"', ' rel="noopener noreferrer nofollow"'):
            with self.subTest(rel=rel):
                self.link('<a href="https://example.org/"%s>x</a>' % rel)
                self.assertFails("security.html: outbound link 'https://example.org/' needs rel=\"noopener noreferrer\"")

    def test_the_order_of_the_two_words_does_not_matter(self):
        self.link('<a href="https://example.org/" rel="noreferrer  noopener">x</a>')
        self.assertEqual(self.check(), [])

    def test_outbound_links_are_https(self):
        for href, reason in (
            ("http://example.org/", "only https:// addresses are allowed"),
            ("javascript:alert(1)", "only https:// addresses are allowed"),
            ("mailto:someone@example.org", "only https:// addresses are allowed"),
            ("ftp://example.org/file", "only https:// addresses are allowed"),
            ("data:text/html,x", "only https:// addresses are allowed"),
            ("//example.org/", "only https:// addresses are allowed"),
            ("/absolute/path", "only https:// addresses are allowed"),
            ("https://user@example.org/", "not a plain https address"),
            ("https://example.org:8443/", "not a plain https address"),
            ("HTTPS://EXAMPLE.ORG/", "only https:// addresses are allowed"),
        ):
            with self.subTest(href=href):
                self.link('<a href="%s" rel="noopener noreferrer">x</a>' % href)
                self.assertFails("security.html: outbound link %r: %s" % (href, reason))

    def test_a_script_address_in_disguise_is_never_a_link_inside_the_site(self):
        # A browser drops white space and control characters in front of and inside the
        # scheme. Whatever is not an existing file of the site is reported.
        for href, message in (
            (" javascript:alert(1)", "link ' javascript:alert(1)':  javascript:alert(1) does not exist in the output"),
            ("java&#9;script:alert(1)", "link 'java\\tscript:alert(1)': java\tscript:alert(1) does not exist in the output"),
            ("java&#10;script:alert(1)", "does not exist in the output"),
            ("&#1;javascript:alert(1)", "outbound link 'javascript:alert(1)': only https:// addresses are allowed"),
            ("JaVaScRiPt:alert(1)", "outbound link 'JaVaScRiPt:alert(1)': only https:// addresses are allowed"),
        ):
            with self.subTest(href=href):
                self.link('<a href="%s" rel="noopener noreferrer">x</a>' % href)
                problems = self.check()
                self.assertTrue(any(message in problem for problem in problems), problems)

    def test_links_inside_the_site_carry_no_rel(self):
        self.link('<a href="manual.html" rel="noopener noreferrer">x</a>')
        self.assertFails("security.html: internal link 'manual.html' carries rel='noopener noreferrer'")

    def test_a_link_to_a_page_that_does_not_exist(self):
        for href, target in (("manuel.html", "manuel.html"), ("docs/manual.html", "docs/manual.html"),
                             ("../index.html", "../index.html"), ("manual.html/", "manual.html"),
                             ("Manual.html", "Manual.html"), ("manual", "manual")):
            with self.subTest(href=href):
                self.link('<a href="%s">x</a>' % href)
                if href == "manual.html/":
                    self.assertEqual(self.check(), [])  # (normalised to the page; a server would refuse it)
                    continue
                self.assertFails("security.html: link %r: %s does not exist in the output" % (href, target))

    def test_a_fragment_that_does_not_resolve(self):
        for href, message in (
            ("manual.html#no-such-section", "link 'manual.html#no-such-section': manual.html has no element with id 'no-such-section'"),
            ("#nowhere", "link '#nowhere': security.html has no element with id 'nowhere'"),
            ("threat-model.html#row-j9", "link 'threat-model.html#row-j9': threat-model.html has no element with id 'row-j9'"),
            ("manual.html#Status", "link 'manual.html#Status': manual.html has no element with id 'Status'"),
            ("assets/site.css#top", "link 'assets/site.css#top': a fragment on something that is not a page"),
        ):
            with self.subTest(href=href):
                self.link('<a href="%s">x</a>' % href)
                self.assertFails("security.html: " + message)

    def test_fragments_that_resolve(self):
        self.add("README.md", "\n## Über uns\n\nText.\n")
        self.link('<a href="manual.html#status">a</a> <a href="#site-content">b</a> <a href="manual.html#%C3%BCber-uns">c</a> '
                  '<a href="manual.html#über-uns">d</a> <a href="index.html?x=1#status">e</a> <a href="">f</a> '
                  '<a href="assets/shot-one.png">g</a>')
        self.assertEqual(self.check(), [])

    def test_a_fragment_in_a_document_that_does_not_resolve(self):
        self.edit("README.md", "(#status)", "(#statuss)")
        self.assertFails("manual.html: link '#statuss': manual.html has no element with id 'statuss'")

    def test_a_fragment_into_another_document_that_does_not_resolve(self):
        self.edit("README.md", "(docs/THREAT_MODEL.md#5-not-defended)", "(docs/THREAT_MODEL.md#6-not-defended)")
        self.assertFails("manual.html: link 'threat-model.html#6-not-defended': threat-model.html has no element with id "
                         "'6-not-defended'")

    def test_the_error_page_has_no_relative_link_when_deployed(self):
        # GitHub Pages serves 404.html for a missing address at any depth: there,
        # "index.html" means /project/a/b/index.html. (A preview is relative throughout.)
        self.edit("site/src/pages/404.html", '<a href="{{ROOT}}index.html">', '<a href="index.html">')
        self.assertEqual(self.check(), [])
        self.assertFails("404.html: link 'index.html' is relative, but the error page is served at any depth (write "
                         "{{ROOT}} before it)", deploy=True, pages=["404.html"])

    def test_the_error_page_loads_nothing_by_a_relative_address_when_deployed(self):
        self.edit("site/src/template.html", 'href="{{ROOT}}assets/site.css"', 'href="assets/site.css"')
        self.assertEqual(self.check(), [])
        self.assertFails("404.html: stylesheet 'assets/site.css' is relative, but the error page is served at any depth",
                         deploy=True, pages=["404.html"])
        self.write("site/src/template.html", TEMPLATE)
        self.add("site/src/pages/404.html", '<p><img src="assets/mark.svg" alt="" width="8" height="8"></p>\n')
        self.assertFails("404.html: image 'assets/mark.svg' is relative, but the error page is served at any depth",
                         deploy=True, pages=["404.html"])

    def test_the_error_pages_absolute_links_are_checked_like_the_others(self):
        self.edit("site/src/pages/404.html", "{{ROOT}}index.html", "{{ROOT}}indexx.html")
        self.assertFails("404.html: link '%sindexx.html': indexx.html does not exist in the output" % SITE, deploy=True)
        self.assertFails("404.html: link 'indexx.html': indexx.html does not exist in the output")

    def test_an_attribute_without_a_value_is_an_empty_one(self):
        # As a browser reads it. (The check used to stop with a traceback on these.)
        self.link('<a href="https://example.org/" rel>x</a> <a href>this page</a>')
        self.assertEqual(self.check(), ["security.html: outbound link 'https://example.org/' needs rel=\"noopener noreferrer\""])
        self.link('<img src alt="" width="8" height="8"> <img src=" " alt="" width="8" height="8">')
        self.assertEqual(self.check(), ["security.html: image with an empty address"] * 2)
        self.write("site/src/pages/security.html", SECURITY)
        self.edit("site/src/template.html", '<meta name="referrer" content="no-referrer">',
                  '<meta name="referrer" content="no-referrer">\n<meta http-equiv>')
        self.assertFails('<meta http-equiv=""> is not allowed', pages=PAGES)

    def test_a_link_to_the_sites_own_address_is_not_a_link_inside_the_site(self):
        # Inside the site every link is relative, so that it also works from a file system.
        # Written as an outbound link (with rel), it used to pass.
        message = ("security.html: link '%smanual.html' names this site by its address; inside the site every link is "
                   "relative, so that the site also works from a file system")
        for markup in ('<a href="{{SITE_URL}}manual.html">x</a>', '<a href="{{SITE_URL}}manual.html" rel="noopener noreferrer">x</a>'):
            with self.subTest(markup=markup):
                self.link(markup)
                self.assertEqual(self.check(deploy=True), [message % SITE])
                self.assertEqual(self.check(), [message % build.PREVIEW_SITE_URL])
        self.link('<a href="{{SITE_URL}}" rel="noopener noreferrer">the overview</a>')
        self.assertEqual(self.check(deploy=True), [(message % SITE).replace("manual.html", "")])

    def test_the_error_page_names_the_site_by_root_not_by_its_address(self):
        # {{ROOT}} is empty in a preview, where the site address does not resolve.
        self.edit("site/src/pages/404.html", "{{ROOT}}index.html", "{{SITE_URL}}index.html")
        self.assertEqual(self.check(deploy=True), [])
        self.assertFails("404.html: link '%sindex.html' names this site by its address" % build.PREVIEW_SITE_URL,
                         pages=["404.html"])


class CheckEmergency(SiteCase):
    MESSAGE = "emergency.html: its outbound links must be exactly runbook::LINKS; "

    def test_an_extra_link_in_the_shell(self):
        self.add("site/src/emergency.html", '<p><a href="https://example.org/help" rel="noopener noreferrer">More help</a></p>\n')
        self.assertFails(self.MESSAGE + "not in the list: ['https://example.org/help']; missing from the page: none")

    def test_an_extra_link_in_the_footer(self):
        self.write("site/src/partials/footer-minimal.html", FOOTER)
        self.assertFails(self.MESSAGE + "not in the list: ['https://repository.invalid']; missing from the page: none")

    def test_a_link_of_the_launcher_that_the_page_does_not_have(self):
        self.edit("crates/proto/src/runbook.rs", "];\n\npub fn",
                  '    ("https://account.example.com/activity", "Account: recent activity"),\n];\n\npub fn')
        self.assertFails(self.MESSAGE + "not in the list: none; missing from the page: ['https://account.example.com/activity']")

    def test_an_address_that_differs_in_one_character(self):
        self.edit("crates/proto/src/runbook.rs", "https://account.example.com/password", "https://account.example.com/Password")
        self.assertStops("docs/SCRAM_RUNBOOK.md: https://account.example.com/password is not in runbook::LINKS")

    def test_the_warning_must_be_on_the_page(self):
        self.edit("site/src/emergency.html", "{{WARNING}}\n", "")
        self.assertFails("emergency.html: the warning 'from a different device' is missing")

    def test_no_address_before_the_warning(self):
        self.edit("site/src/emergency.html", '<div class="callout danger">\n{{WARNING}}\n</div>\n', "")
        self.edit("site/src/emergency.html", "{{BODY}}\n", "{{BODY}}\n{{WARNING}}\n")
        self.assertFails("emergency.html: an outbound link comes before the warning to use another device")

    def test_the_emergency_page_must_exist(self):
        self.generate()
        (self.out / "emergency.html").unlink()
        site = self.site()
        site.build()
        problems, _ = build.check_output(self.out, site, explicit_urls=False)
        self.assertIn("emergency.html was not generated", problems)


class CheckCommands(SiteCase):
    def command(self, text, page="security.html", source=SECURITY):
        self.write("site/src/pages/" + page, source + '<pre tabindex="0"><code>%s</code></pre>\n' % text)

    def test_commands_on_a_page_exist_in_the_help_text(self):
        for text, reason in (
            ("bastion-mc frobnicate", "`bastion-mc frobnicate`: command 'frobnicate' is not in the launcher's help text"),
            ("bastion-mc doctor check", "`bastion-mc doctor check`: 'check' is not a form of `doctor` in the launcher's help text"),
            ("bastion-mc skin wear cape", "`bastion-mc skin wear cape`: 'wear' is not a form of `skin` in the launcher's help text"),
            ("bastion-mc status --demo",
             "`bastion-mc status --demo`: option --demo is not an option of `bastion-mc status` in the launcher's help text"),
            ("bastion-mc skin show --slim",
             "`bastion-mc skin show --slim`: option --slim is not an option of `bastion-mc skin show` in the launcher's help text"),
            ("bastion-mc launch &lt;id&gt; --burner",
             "`bastion-mc launch <id> --burner`: option --burner is not an option of `bastion-mc launch` in the launcher's help text"),
            ("bastion-mc launch --events &lt;id&gt;",
             "`bastion-mc launch --events <id>`: option --events is not an option of `bastion-mc launch` in the launcher's help text"),
            ("bastion-mc -v status", "`bastion-mc -v status`: option -v in front of the command is not in the launcher's help text"),
            ("target/release/bastion-mc docter", "`bastion-mc docter`: command 'docter' is not in the launcher's help text"),
            ("sudo -u vault /usr/bin/bastion-mc lanch main", "`bastion-mc lanch main`: command 'lanch' is not in the launcher's help text"),
            ("bastion-mc doctor &amp;&amp; bastion-mc purge", "`bastion-mc purge`: command 'purge' is not in the launcher's help text"),
        ):
            with self.subTest(command=text):
                self.command(text)
                self.assertEqual(self.check(), ["security.html: " + reason])

    def test_an_option_on_a_continued_line_is_checked_too(self):
        self.command("bastion-mc launch main \\\n    --account work \\\n    --burner")
        self.assertEqual(self.check(), ["security.html: `bastion-mc launch main --account work --burner`: option --burner is "
                                        "not an option of `bastion-mc launch` in the launcher's help text"])

    def test_in_running_text_too(self):
        self.write("site/src/pages/security.html", SECURITY + "<p>Run <code>bastion-mc selfdestruct</code> now.</p>\n")
        self.assertFails("security.html: `bastion-mc selfdestruct`: command 'selfdestruct' is not in the launcher's help text")

    def test_commands_that_agree_with_the_help_text(self):
        self.command("\n".join((
            "bastion-mc",
            "bastion-mc --help",
            "bastion-mc --version",
            "bastion-mc &lt;command&gt;",
            "bastion-mc doctor",
            "bastion-mc status --account NAME",
            "bastion-mc --events status",
            "bastion-mc config path",
            "bastion-mc config example &gt; ~/.config/bastion-mc/config.toml",
            "bastion-mc account default",
            "bastion-mc account default main",
            "bastion-mc skin set skin.png --slim --account main",
            "bastion-mc cape set &lt;alias|none&gt;",
            "bastion-mc cape set none",
            "bastion-mc scram --yes",
            "bastion-mc wiki scram --links",
            "bastion-mc instance new vanilla --minecraft 1.21.4",
            "bastion-mc instance list",
            "bastion-mc launch vanilla --join mc.example.org:25565 --demo --quiet",
            "target/release/bastion-mc launch &lt;id&gt; --account NAME",
            "sudo -u vault /usr/bin/bastion-mc launch main   # as the other account",
            "cargo build   # builds bastion-mc and bastion-mc-gui",
            "bastion-mc-gui",
            "ls ~/.config/bastion-mc/ /etc/bastion-mc/trusted-keys.d",
            "bastion-mc profile lock &lt;id&gt;",
        )))
        self.assertEqual(self.check(), [])

    def test_a_command_in_a_rendered_document_is_reported_but_does_not_fail(self):
        # The documents are the launcher's own; this site does not get to fail on them.
        self.edit("README.md", "`bastion-mc launch <id> --demo`", "`bastion-mc launch <id> --daemon`")
        self.assertEqual(self.check(), [])
        self.assertEqual(self.notices, ["manual.html: `bastion-mc launch <id> --daemon`: option --daemon is not an option of "
                                        "`bastion-mc launch` in the launcher's help text (in a rendered document: not an error here)"])

    def test_a_command_in_a_section_included_on_a_page_fails_there(self):
        self.edit("README.md", "target/release/bastion-mc doctor", "target/release/bastion-mc doktor")
        self.add("site/src/pages/security.html", '{{SECTION README.md "Requirements"}}\n')
        self.assertFails("security.html: `bastion-mc doktor`: command 'doktor' is not in the launcher's help text")
        self.assertEqual(len(self.notices), 1)


class CheckMachineReadableFiles(SiteCase):
    def checked(self, name, text):
        """Check the output after one of its files was replaced."""
        site = self.generate()
        (self.out / name).write_text(text, encoding="utf-8")
        problems, _ = build.check_output(self.out, site, explicit_urls=False)
        return problems

    def sitemap(self, *locations):
        return ('<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
                + "".join("  <url><loc>%s</loc></url>\n" % location for location in locations) + "</urlset>\n")

    def test_the_sitemap_lists_only_pages_of_this_site(self):
        for location in ("https://elsewhere.example.org/", "https://site.invalid/nothing.html", "https://site.invalid/404.html",
                         "https://site.invalid/assets/site.css", "manual.html", "https://site.invalid.example.org/"):
            with self.subTest(location=location):
                self.assertEqual(self.checked("sitemap.xml", self.sitemap("https://site.invalid/", location)),
                                 ["sitemap.xml: %r is not a page of this site" % location])

    def test_the_sitemap_lists_something_and_is_xml(self):
        self.assertEqual(self.checked("sitemap.xml", self.sitemap()), ["sitemap.xml lists nothing"])
        problems = self.checked("sitemap.xml", "<urlset>")
        self.assertEqual(len(problems), 1)
        self.assertTrue(problems[0].startswith("sitemap.xml: "), problems)

    def test_robots_names_the_sitemap(self):
        for text in ("User-agent: *\nAllow: /\n", "Sitemap: https://elsewhere.example.org/sitemap.xml\n",
                     "Sitemap: sitemap.xml\n", "Sitemap: {{SITE_URL}}sitemap.xml"):
            with self.subTest(robots=text):
                self.write("site/src/robots.txt", text)
                self.assertFails("robots.txt does not name the sitemap under the site address")

    def test_robots_is_part_of_the_site(self):
        self.remove("site/src/robots.txt")
        self.assertStops("site/src/robots.txt does not exist")


class CheckTokens(SiteCase):
    def test_a_token_in_a_file_that_is_not_expanded(self):
        self.write("site/src/assets/site.css", "body { margin: 0; } /* {{ROOT}} */\n")
        self.assertFails("assets/site.css: unsubstituted {{ROOT}}")

    def test_a_token_in_an_svg(self):
        self.edit("site/src/assets/mark.svg", "<title>mark</title>", "<title>{{TITLE}}</title>")
        self.assertFails("assets/mark.svg: unsubstituted {{TITLE}}")

    def test_a_token_in_a_file_that_is_included(self):
        self.write("site/src/partials/aside.html", ASIDE)
        self.edit("site/src/partials/aside.html", "An included note.", "See {{REPO_URL}}.")
        self.assertStops("site/src/partials/aside.html: an included file cannot use directives itself")

    def test_a_token_that_a_document_brings_along_is_text(self):
        # In every place a document's text reaches a page: running text, a code span, a code
        # block, a heading (and its anchor's label), a table's header and cells, the title
        # (also in <title>), and an included section of a hand-written page.
        self.write("docs/TOKEN_INJECTION.md", "# Token path {{SITE_URL}}\n\n## How {{REPO_URL}} gets there\n\n"
                   "Never on a command line. {{SITE_URL}} and `{{{ROOT}}}`.\n\n```\n{{CSP}}\n```\n\n"
                   "| {{TITLE}} | b |\n|---|---|\n| {{BODY}} | 2 |\n")
        self.assertEqual(self.check(), [])
        page = self.page("token-injection.html")
        self.assertNotIn("{{", page)
        for text in ("<title>Token path &#123;{SITE_URL}} | bastion-mc</title>", "<h1 id=\"token-path-site_url\">Token path &#123;{SITE_URL}}</h1>",
                     ">How &#123;{REPO_URL}} gets there</h2>", 'aria-label="Permalink: How &#123;{REPO_URL}} gets there"',
                     "Never on a command line. &#123;{SITE_URL}} and <code>&#123;&#123;{ROOT}}}</code>.",
                     "<code>&#123;{CSP}}\n</code>", 'data-label="&#123;{TITLE}}"', ">&#123;{TITLE}}</th>", ">&#123;{BODY}}</td>"):
            with self.subTest(text=text):
                self.assertIn(text, page)
        self.add("docs/THREAT_MODEL.md", "\n## 6. More\n\nA {{TITLE}} in a section.\n")
        self.edit("site/src/pages/index.html", '{{PARAGRAPH docs/THREAT_MODEL.md "Out of scope:"}}',
                  '{{PARAGRAPH docs/THREAT_MODEL.md "Out of scope:"}}\n{{SECTION docs/THREAT_MODEL.md "6. More"}}')
        self.assertEqual(self.check(), [])
        self.assertIn("<p>A &#123;{TITLE}} in a section.</p>", self.page("index.html"))

    def test_a_token_in_a_page_header_stops_the_build(self):
        # (the header is written as it is: the check would have caught it, the build names it)
        self.edit("site/src/pages/security.html", "title: Security | bastion-mc", "title: Security of {{SITE_URL}}")
        self.assertStops("site/src/pages/security.html: header line 'title: Security of {{SITE_URL}}': the header "
                         "takes no directives")

    def test_a_deployable_build_has_no_preview_address_left(self):
        self.add("site/src/pages/security.html",
                 '<p><a href="https://repository.invalid/issues" rel="noopener noreferrer">written by hand</a></p>\n')
        self.assertEqual(self.check(), [])  # (in a preview it is one more outbound link)
        self.assertFails("security.html: still contains the preview address https://repository.invalid", deploy=True)

    def test_neither_preview_address(self):
        self.add("site/src/robots.txt", "# see https://site.invalid/\n")
        self.assertFails("robots.txt: still contains the preview address https://site.invalid/", deploy=True)

    def test_the_command_line_applies_this_to_a_deployable_build_only(self):
        self.add("site/src/robots.txt", "# see https://site.invalid/\n")
        self.assertEqual(self.run_main("--check")[0], 0)
        self.assertEqual(self.run_main("--check", "--site-url", SITE)[0], 0)  # (one address: still a preview)
        self.assertEqual(self.run_main("--check", "--repo-url", REPO, "--site-url", SITE),
                         (1, "", "check failed: robots.txt: still contains the preview address https://site.invalid/\n"
                                 "error: 1 problem(s) in the generated site\n"))


class CheckDeterminism(SiteCase):
    def arguments(self):
        return ["--root", str(self.root)]

    def test_two_runs_of_the_small_site_are_identical(self):
        self.generate()
        self.assertEqual(build.check_deterministic(self.out, self.arguments()), [])

    def test_a_file_that_differs_is_named(self):
        self.generate()
        with open(str(self.out / "index.html"), "a", encoding="utf-8") as handle:
            handle.write("<!-- built at 21:40:07 -->\n")
        (self.out / "assets" / "site.css").write_bytes(b"body { margin: 1px; }\n")
        self.assertEqual(build.check_deterministic(self.out, self.arguments()),
                         ["assets/site.css differs between two runs", "index.html differs between two runs"])

    def test_a_file_that_only_one_run_wrote_is_named(self):
        self.generate()
        (self.out / "extra.html").write_text("x\n", encoding="utf-8")
        (self.out / "robots.txt").unlink()
        self.assertEqual(build.check_deterministic(self.out, self.arguments()),
                         ["extra.html exists in only one of two runs", "robots.txt exists in only one of two runs"])

    def test_a_second_run_that_fails_is_reported(self):
        self.generate()
        self.remove("site/src/template.html")
        problems = build.check_deterministic(self.out, self.arguments())
        self.assertEqual(len(problems), 1)
        self.assertTrue(problems[0].startswith("the second run failed: error: site/src/template.html does not exist"), problems)

    def test_the_second_run_gets_the_same_addresses(self):
        self.generate(deploy=True)
        self.assertEqual(build.check_deterministic(self.out, self.arguments() + ["--repo-url", REPO, "--site-url", SITE]), [])
        different = build.check_deterministic(self.out, self.arguments())
        self.assertIn("404.html differs between two runs", different)
        self.assertIn("sitemap.xml differs between two runs", different)

    def test_the_command_line_fails_on_output_that_is_not_reproducible(self):
        # A generator that wrote the time of day into a page: here it is the first run
        # that does it, and the fresh second run, which does not, shows the difference.
        original = build.write_output

        def stamped(files, out):
            files = dict(files)
            files["index.html"] += b"\n"
            original(files, out)

        build.write_output = stamped
        try:
            status, out, err = self.run_main("--check")
        finally:
            build.write_output = original
        self.assertEqual((status, out), (1, ""))
        self.assertEqual(err, "check failed: index.html differs between two runs\nerror: 1 problem(s) in the generated site\n")

    def test_the_output_does_not_depend_on_the_hash_seed(self):
        import subprocess

        outputs = []
        for seed in ("0", "1", "4242"):
            out = self.scratch / ("seed-" + seed)
            environment = dict(os.environ, PYTHONHASHSEED=seed)
            result = subprocess.run([sys.executable, "-B", str(HERE / "build.py"), "--root", str(self.root), "--out", str(out),
                                     "--quiet"], env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            self.assertEqual((result.returncode, result.stdout), (0, b""))
            outputs.append({path.relative_to(out).as_posix(): path.read_bytes() for path in sorted(out.rglob("*")) if path.is_file()})
        self.assertEqual(outputs[0], outputs[1])
        self.assertEqual(outputs[0], outputs[2])


# --------------------------------------------------------------------------------------
# What the generator reads from the launcher's code
# --------------------------------------------------------------------------------------


class Commands(unittest.TestCase):
    def test_finding_invocations_in_code_text(self):
        for text, expected in (
            ("bastion-mc", [[]]),
            ("bastion-mc doctor", [["doctor"]]),
            ("$ bastion-mc launch <id> --demo", [["launch", "<id>", "--demo"]]),
            ("bastion-mc doctor\nbastion-mc status --account NAME\n", [["doctor"], ["status", "--account", "NAME"]]),
            ("bastion-mc doctor && bastion-mc wiki scram --links", [["doctor"], ["wiki", "scram", "--links"]]),
            ("bastion-mc doctor; bastion-mc scram", [["doctor"], ["scram"]]),
            ("(bastion-mc status)", [["status"]]),
            ("target/release/bastion-mc doctor", [["doctor"]]),
            ("./target/debug/bastion-mc doctor", [["doctor"]]),
            ("/usr/bin/bastion-mc launch main", [["launch", "main"]]),
            ("./bastion-mc status", [["status"]]),
            ("sudo -u vault bastion-mc launch main", [["launch", "main"]]),
            ("bastion-mc config example > ~/.config/bastion-mc/config.toml", [["config", "example"]]),
            ("bastion-mc doctor 2>&1 | less", [["doctor"]]),
            ("bastion-mc wiki scram >runbook.txt", [["wiki", "scram"]]),
            ("bastion-mc config path | example", [["config", "path"]]),
            ("bastion-mc cape set <alias|none>", [["cape", "set"]]),
            # A line that ends in a backslash goes on in the next, as in the shell.
            ("bastion-mc launch main \\\n    --demo", [["launch", "main", "--demo"]]),
            ("bastion-mc launch main \\\n    --account work \\\n    --quiet\nbastion-mc doctor",
             [["launch", "main", "--account", "work", "--quiet"], ["doctor"]]),
            ("echo x \\\n    > bastion-mc.txt\nbastion-mc doctor", [["doctor"]]),
            ("# a note that ends in a backslash \\\nbastion-mc doctor", [["doctor"]]),
            ("bastion-mc launch main   # a note \\\n--demo", [["launch", "main"]]),
            ("bastion-mc launch main \\", [["launch", "main"]]),
            ("cargo build --release   # builds bastion-mc and its front end", []),
            ("# bastion-mc doctor", []),
            ("bastion-mc doctor   # checks the setup with bastion-mc itself", [["doctor"]]),
            ("bastion-mc-gui", []),
            ("bastion-mc-gui --help", []),
            ("bastion-mc.toml", []),
            ("~/.config/bastion-mc/config.toml", []),
            ("/etc/bastion-mc/trusted-keys.d", []),
            ("ls target/release/bastion-mc", [[]]),
            ("the bastion-mc (launcher) process", []),
            ("bastion-mc_events", []),
            ("xbastion-mc doctor", []),
            ("pkgname=bastion-mc", [[]]),
        ):
            with self.subTest(text=text):
                self.assertEqual(build.commands_in(text), expected)


class HelpTexts(SiteCase):
    def test_what_is_read_from_the_small_help_text(self):
        forms = build.HelpText(self.root).forms
        self.assertEqual(sorted(forms), ["account", "cape", "config", "doctor", "instance", "launch", "profile", "scram",
                                         "skin", "status", "wiki"])
        self.assertEqual(forms["status"], [(None, False, {"--account"})])
        self.assertEqual(forms["doctor"], [(None, False, set())])
        self.assertEqual(forms["config"], [(["path", "example"], False, set())])
        self.assertEqual(forms["account"], [(["default"], True, set())])
        self.assertEqual(forms["skin"], [(["show"], False, {"--account"}), (["set"], True, {"--slim", "--account"})])
        self.assertEqual(forms["cape"], [(["set"], True, {"--account"})])
        self.assertEqual(forms["instance"], [(["new"], True, {"--minecraft"}), (["list"], False, set())])
        self.assertEqual(forms["launch"], [(None, True, {"--account", "--join", "--demo", "--quiet"})])
        self.assertEqual(forms["profile"], [(["lock"], True, set())])

    def test_the_first_word_of_a_description_is_not_a_sub_command(self):
        help_text = build.HelpText(self.root)
        self.assertIsNotNone(help_text.problem_with(["doctor", "check"]))
        self.assertIsNotNone(help_text.problem_with(["scram", "EMERGENCY"]))
        self.assertIsNotNone(help_text.problem_with(["status", "sign-in"]))

    def test_a_help_text_the_generator_cannot_read_stops_the_build(self):
        for old, new, message in (
            ("const HELP: &str", "const USAGE: &str", "src/cli.rs: cannot find the HELP constant"),
            ("bastion-mc — a launcher under test", "bastion-mc \\u{2014} a launcher under test",
             "src/cli.rs: escape \\u in a string this generator has to read"),
        ):
            with self.subTest(change=new):
                self.write("src/cli.rs", CLI_RS.replace(old, new))
                with self.assertRaises(build.BuildError) as caught:
                    self.site()
                self.assertEqual(str(caught.exception), message)
        for text, message in (
            ('const HELP: &str = "no commands here";\n', "src/cli.rs: the HELP constant lists no commands"),
            # (the command check gives --events a place of its own; if the launcher drops the
            # option, that rule has to go too)
            (CLI_RS.replace("[--events] ", "").replace(
                "  --events    (first argument) report progress as JSON lines on standard output\n", ""),
             "src/cli.rs: the HELP constant no longer mentions --events"),
        ):
            with self.subTest(message=message):
                self.assertNotIn("--events", text)
                self.write("src/cli.rs", text)
                with self.assertRaises(build.BuildError) as caught:
                    self.site()
                self.assertEqual(str(caught.exception), message)


class RunbookLinks(SiteCase):
    def test_what_is_read_from_the_small_list(self):
        self.assertEqual(build.runbook_links(self.root), [
            ("https://account.example.com/password", "Account: change the password"),
            ("https://account.example.com/sessions?all=1&now=1", 'Account: sign out "everywhere" & more'),
        ])

    def test_a_list_the_generator_cannot_read_stops_the_build(self):
        entry = '    ("https://account.example.com/password", "Account: change the password"),\n'
        for old, new, message in (
            ("pub const LINKS:", "pub const ADDRESSES:", "crates/proto/src/runbook.rs: cannot find the LINKS list"),
            (entry, '    ("https://account.example.com/password"),\n',
             "crates/proto/src/runbook.rs: LINKS is not a list of (address, description) pairs"),
            (entry, '    ("http://account.example.com/password", "Account: change the password"),\n',
             "crates/proto/src/runbook.rs: unexpected LINKS entry 'http://account.example.com/password'"),
            (entry, '    ("Account: change the password", "https://account.example.com/password"),\n',
             "crates/proto/src/runbook.rs: unexpected LINKS entry 'Account: change the password'"),
            (entry, entry + entry, "crates/proto/src/runbook.rs: LINKS lists an address twice"),
            (entry, '    ("https://account.example.com/password", "Account: change\\tthe password"),\n',
             "crates/proto/src/runbook.rs: escape \\t in a string this generator has to read"),
        ):
            with self.subTest(change=new):
                self.assertEqual(RUNBOOK_RS.count(old), 1)
                self.write("crates/proto/src/runbook.rs", RUNBOOK_RS.replace(old, new))
                with self.assertRaises(build.BuildError) as caught:
                    build.runbook_links(self.root)
                self.assertEqual(str(caught.exception), message)

    def test_an_empty_list_is_refused(self):
        self.write("crates/proto/src/runbook.rs", "pub const LINKS: &[(&str, &str)] = &[\n];\n")
        with self.assertRaises(build.BuildError):
            build.runbook_links(self.root)


# --------------------------------------------------------------------------------------
# The stated repairs of defects in documents the site does not own
# --------------------------------------------------------------------------------------


def numbered(text):
    return [(number, line) for number, line in enumerate(text.split("\n"), 1)]


class Repairs(unittest.TestCase):
    HEADER = "| # | Attack | Defence | Residual |"
    DEFECT = "\n".join((
        "intro",
        "",
        HEADER,
        "|---|---|---|---|",
        "| D1 | a | b | tested by t1 | r1 |",
        "| D2 | a \\| c | b | tested by t2 | r2 |",
        "| D3 | a | b | r3 |",
        "",
        "after",
    ))

    def test_the_missing_column_is_added_where_rows_already_have_it(self):
        repair = build.repair_missing_column(self.HEADER, "Verified by", 3)
        lines, done = repair(numbered(self.DEFECT))
        self.assertEqual(done, "gave the table under '| # | Attack | Defence | Residual |' the column 'Verified by' that 2 of "
                               "its rows already have")
        self.assertEqual([text for _, text in lines], [
            "intro",
            "",
            "| # | Attack | Defence | Verified by | Residual |",
            "| --- | --- | --- | --- | --- |",
            "| D1 | a | b | tested by t1 | r1 |",
            "| D2 | a \\| c | b | tested by t2 | r2 |",
            "| D3 | a | b |  | r3 |",
            "",
            "after",
        ])
        self.assertEqual([number for number, _ in lines], list(range(1, 10)))  # (errors keep the file's line numbers)
        html_text = md.render(md.parse(self.DEFECT, repair=lambda lines: repair(lines)[0]))
        self.assertIn('<td role="cell" data-label="Verified by">tested by t1</td><td role="cell" data-label="Residual">r1</td>',
                      html_text)
        self.assertIn('<td role="cell" data-label="Attack">a | c</td>', html_text)
        self.assertIn('<td role="cell" data-label="Verified by"></td><td role="cell" data-label="Residual">r3</td>', html_text)

    def test_it_is_inert_once_the_document_is_fixed(self):
        repair = build.repair_missing_column(self.HEADER, "Verified by", 3)
        for fixed in (
            # the column was added by the owner of the document
            self.DEFECT.replace(self.HEADER, "| # | Attack | Defence | Verified by | Residual |").replace(
                "|---|---|---|---|", "|---|---|---|---|---|").replace("| D3 | a | b | r3 |", "| D3 | a | b | t3 | r3 |"),
            # or the two rows were cut down to the header
            self.DEFECT.replace(" tested by t1 |", "").replace(" tested by t2 |", ""),
            # or the table is gone
            "intro\n\nafter",
        ):
            with self.subTest(fixed=fixed):
                lines = numbered(fixed)
                self.assertEqual(repair(lines), (lines, None))

    def test_cells_are_counted_as_the_converter_counts_them(self):
        # A pipe after a backslash never ends a cell, whatever stands before the backslash
        # (renderer-4): the repair must count the cells the converter will see.
        self.assertEqual(build._raw_cells("| D2 | a \\\\| b | c |"), ["D2", "a \\\\| b", "c"])
        self.assertEqual(build._raw_cells("| D2 | a \\| b | c |"), ["D2", "a \\| b", "c"])
        self.assertEqual(build._raw_cells("| a |  |"), ["a", ""])
        self.assertIsNone(build._raw_cells("a | b"))
        defect = self.DEFECT.replace("| D2 | a \\| c |", "| D2 | a \\\\| c |")
        lines, done = build.repair_missing_column(self.HEADER, "Verified by", 3)(numbered(defect))
        self.assertIsNotNone(done)
        self.assertIn((6, "| D2 | a \\\\| c | b | tested by t2 | r2 |"), lines)

    def test_it_does_not_touch_a_table_it_does_not_understand(self):
        repair = build.repair_missing_column(self.HEADER, "Verified by", 3)
        for other in (
            self.DEFECT.replace("| D3 | a | b | r3 |", "| D3 | a | b |"),  # a row that is too short
            self.DEFECT.replace("| D1 | a | b | tested by t1 | r1 |", "| D1 | a | b | t | r | one more |"),  # two cells too many
            self.DEFECT.replace("|---|---|---|---|\n", ""),  # no row of dashes
        ):
            with self.subTest(table=other):
                lines = numbered(other)
                self.assertEqual(repair(lines), (lines, None))

    SPLIT = "| ID | What |\n|---|---|\n| E10 | ten |\n\n\n| E11 | eleven |\n| E12 | twelve |\n\nafter"

    def test_a_table_split_by_blank_lines_is_joined(self):
        repair = build.repair_split_table("| E11 |")
        lines, done = repair(numbered(self.SPLIT))
        self.assertEqual(done, "joined the table that a blank line splits above the row '| E11 |'")
        self.assertEqual(lines, [(1, "| ID | What |"), (2, "|---|---|"), (3, "| E10 | ten |"), (6, "| E11 | eleven |"),
                                 (7, "| E12 | twelve |"), (8, ""), (9, "after")])
        with self.assertRaises(md.MarkdownError) as caught:
            md.parse(self.SPLIT)
        self.assertEqual((caught.exception.line, caught.exception.message),
                         (6, "table row outside a table (is there a blank line inside the table?)"))
        html_text = md.render(md.parse(self.SPLIT, repair=lambda lines: repair(lines)[0]), md.Renderer(row_ids=True))
        self.assertIn('<tr role="row" id="row-e10">', html_text)
        self.assertIn('<tr role="row" id="row-e12">', html_text)

    def test_joining_is_inert_once_the_document_is_fixed(self):
        repair = build.repair_split_table("| E11 |")
        for fixed in (
            self.SPLIT.replace("| E10 | ten |\n\n\n", "| E10 | ten |\n"),  # the blank lines are gone
            self.SPLIT.replace("| E11 | eleven |\n", ""),  # the row is gone
            "Text.\n\n| E11 | eleven |",  # what stands above is not a table
            "| E11 | first line of the file |",
        ):
            with self.subTest(fixed=fixed):
                lines = numbered(fixed)
                self.assertEqual(repair(lines), (lines, None))

    def test_a_repair_that_applies_is_announced_with_every_build(self):
        case = SiteCase("setUp")
        case.setUp()
        self.addCleanup(case.doCleanups)
        case.write("docs/THREAT_MODEL.md", THREAT_MODEL + "\n## 7. Evidence\n\n| ID | What |\n|---|---|\n| E10 | ten |\n\n| E11 | eleven |\n")
        original = build.REPAIRS
        build.REPAIRS = {"docs/THREAT_MODEL.md": (build.repair_split_table("| E11 |"),)}
        try:
            self.assertEqual(case.check(), [])
            self.assertEqual(case.notices, ["docs/THREAT_MODEL.md: joined the table that a blank line splits above the row "
                                            "'| E11 |' (the source should be fixed; see docs/notes/website.md)"])
            self.assertIn('<tr role="row" id="row-e11">', (case.out / "threat-model.html").read_text(encoding="utf-8"))
            build.REPAIRS = {}
            self.assertIn("docs/THREAT_MODEL.md:25: table row outside a table", case.assertStops())
        finally:
            build.REPAIRS = original


# --------------------------------------------------------------------------------------
# The launcher's own code and documents
# --------------------------------------------------------------------------------------


class TheWorkflow(unittest.TestCase):
    """What the deployment workflow must keep for the published site to follow the sources
    (content request 2, after web-4)."""

    WORKFLOW = REAL_ROOT / ".github" / "workflows" / "pages.yml"

    def trigger(self):
        """The lines of the workflow's `on:` block (the file is ours; plain text is enough)."""
        lines = self.WORKFLOW.read_text(encoding="utf-8").split("\n")
        self.assertIn("on:", lines, "%s: no `on:` line that this test can read" % self.WORKFLOW)
        block = []
        for line in lines[lines.index("on:") + 1 :]:
            if line and not line.startswith((" ", "#")):
                break
            block.append(line)
        return block

    def test_every_push_to_main_builds_the_site(self):
        trigger = self.trigger()
        self.assertTrue(any(re.match(r"^  push:\s*$", line) for line in trigger), trigger)
        self.assertTrue(any(re.match(r"^    branches:\s*\[\s*main\s*\]\s*$", line) for line in trigger), trigger)
        for line in trigger:
            with self.subTest(line=line):
                self.assertIsNone(
                    re.match(r"^\s+(paths|paths-ignore)\s*:", line),
                    "the workflow's trigger must not filter by path: the generator checks every file a document "
                    "links to, so a push that renames or deletes any file of the repository can change the site "
                    "(docs/notes/website.md, section 4)",
                )


class TheRealSources(unittest.TestCase):
    """What the generator reads outside site/: the real help text, the real list of
    emergency links and the real documents. (The real pages are other people's and are
    checked by `python3 site/build.py --check`.)"""

    @classmethod
    def setUpClass(cls):
        cls.help = build.HelpText(REAL_ROOT)
        cls.links = build.runbook_links(REAL_ROOT)

    def test_the_help_text_is_read_completely(self):
        # Every line of the usage that describes a command is found, whatever it is.
        usage = [line for line in self.help.text.split("\n") if re.match(r"^  [a-z]", line)]
        self.assertGreater(len(usage), 30)
        self.assertEqual(sum(len(forms) for forms in self.help.forms.values()), len(usage))
        for line in usage:
            with self.subTest(line=line):
                self.assertIn(line.split()[0], self.help.commands)

    def test_commands_the_documents_rely_on(self):
        for words in (["doctor"], ["launch", "<id>", "--demo"], ["scram"], ["scram", "--yes"], ["wiki", "scram"],
                      ["wiki", "scram", "--links"], ["status"], ["login", "--device"], ["config", "example"],
                      ["--events", "launch", "<id>"], ["--help"], []):
            with self.subTest(command=words):
                self.assertIsNone(self.help.problem_with(words))

    def test_commands_that_do_not_exist(self):
        for words in (["frobnicate"], ["doctor", "check"], ["status", "--demo"], ["launch", "--events", "<id>"],
                      ["wiki", "scram", "--yes"], ["scram", "--links"], ["skin", "wear"], ["-h"]):
            with self.subTest(command=words):
                self.assertIsNotNone(self.help.problem_with(words))

    def test_every_command_in_the_documents_is_in_the_help_text(self):
        found = 0
        for source, _, _, _, _ in build.DOCUMENTS:
            text = (REAL_ROOT / source).read_text(encoding="utf-8")
            code = re.findall(r"```[a-z]*\n(.*?)```", text, re.S) + re.findall(r"`([^`\n]+)`", text)
            for piece in code:
                for words in build.commands_in(piece):
                    found += 1
                    with self.subTest(document=source, command=" ".join(words)):
                        self.assertIsNone(self.help.problem_with(words))
        self.assertGreater(found, 20)

    def test_the_emergency_links_are_read_completely(self):
        source = (REAL_ROOT / "crates/proto/src/runbook.rs").read_text(encoding="utf-8")
        listed = source[source.index("pub const LINKS") : source.index("];")]
        self.assertEqual(len(self.links), listed.count("https://"))
        self.assertGreater(len(self.links), 5)
        for address, description in self.links:
            with self.subTest(address=address):
                self.assertIsNone(md.check_https_url(address))
                self.assertIn('"%s"' % address, listed)
                self.assertTrue(description.strip())

    def test_the_runbook_names_exactly_those_addresses(self):
        # (the launcher's own test says the same of the compiled list; tests/site.rs
        # compares the generated page with it)
        text = (REAL_ROOT / build.RUNBOOK).read_text(encoding="utf-8")
        renderer = md.Renderer(source=build.RUNBOOK)
        renderer.blocks(md.parse(text, build.RUNBOOK))
        self.assertEqual(sorted(set(renderer.outbound)), sorted(address for address, _ in self.links))
        self.assertIn(build.DEVICE_WARNING, text)

    def test_every_document_converts_as_it_is(self):
        notices = []
        site = build.Site(REAL_ROOT, build.PREVIEW_REPO_URL, build.PREVIEW_SITE_URL, absolute_error_page=False, notices=notices)
        for source, page, _, _, _ in build.DOCUMENTS:
            with self.subTest(document=source):
                document = site.document(source)
                self.assertTrue(md.plain_text(document.title.inlines).strip())
                rendered = site.renderer(source, page, heading_anchors=True, row_ids=True).blocks(document.body)
                self.assertNotIn("<script", rendered)
        # No repair is stated today (the two table defects of the threat model were fixed in
        # the document on 2026-11-14), so nothing may be announced. A repair added for a new
        # defect announces itself at every build until the document is fixed.
        self.assertEqual(build.REPAIRS, {})
        self.assertEqual(notices, [])

    def test_a_tab_after_any_closing_fence_changes_nothing(self):
        # renderer-1: one tab after one closing fence turned whole sections into code without
        # a word. Each closing fence of each document, with a tab after it, renders as before.
        renderer = lambda source: md.Renderer(source=source, heading_anchors=True, row_ids=True)
        tried = 0
        for source, _, _, _, _ in build.DOCUMENTS:
            repairs = build.REPAIRS.get(source, ())

            def repair(lines, repairs=repairs):
                for step in repairs:
                    lines, _ = step(lines)
                return lines

            text = (REAL_ROOT / source).read_text(encoding="utf-8")
            expected = renderer(source).blocks(md.parse(text, source, repair=repair))
            lines, fence = text.split("\n"), None
            for number, line in enumerate(lines):
                opening = re.match(r"^ *(`{3,}|~{3,})", line)
                if fence is None and opening:
                    fence = opening.group(1)
                elif fence is not None and re.match(r"^ {0,3}%s{%d,} *$" % (re.escape(fence[0]), len(fence)), line):
                    fence = None
                    changed = "\n".join(lines[:number] + [line + "\t"] + lines[number + 1 :])
                    with self.subTest(document=source, line=number + 1):
                        self.assertEqual(renderer(source).blocks(md.parse(changed, source, repair=repair)), expected)
                    tried += 1
        self.assertGreater(tried, 20)

    def test_the_sections_the_threat_model_is_known_by(self):
        site = build.Site(REAL_ROOT, build.PREVIEW_REPO_URL, build.PREVIEW_SITE_URL, absolute_error_page=False)
        document = site.document("docs/THREAT_MODEL.md")
        rendered = site.renderer("docs/THREAT_MODEL.md", "threat-model.html", heading_anchors=True, row_ids=True).blocks(document.body)
        # Rows D1, D2 (section 4.3) and E11 to E15 (section 7) are the ones GitHub lost while
        # the document had its two table defects; they are rows of their tables now.
        for row in ("d1", "d2", "e10", "e11", "e15"):
            with self.subTest(row=row):
                self.assertIn('<tr role="row" id="row-%s">' % row, rendered)


if __name__ == "__main__":
    unittest.main()
