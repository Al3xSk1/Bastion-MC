#!/usr/bin/env python3
"""Builds the project website from the repository's own documents.

    python3 site/build.py [--out DIR] [--repo-url URL] [--site-url URL] [--check]

The generator is deterministic, uses only the Python standard library and never touches
the network. It reads

  * README.md and docs/*.md, the single source of truth: each is rendered as a page (with a
    table of contents and GitHub-compatible heading anchors) by the strict converter in
    md.py, which stops with file and line on any Markdown it does not implement;
  * site/src/: the page shell, the hand-written pages, the style sheet, the mark and the
    screenshots;
  * src/cli.rs (the launcher's help text) and crates/proto/src/runbook.rs (the emergency
    links), to check what the pages say against the code.

Two addresses are not known until the site is deployed, so no committed file contains
them: sources write {{REPO_URL}} and {{SITE_URL}}, and the two options fill them in (the
deployment workflow takes them from its own context). Without the options the result is a
local preview whose outbound repository links point to a reserved, unresolvable name.

--check then verifies the written output: every internal link and fragment resolves, no
page loads anything from another origin, every page carries the Content-Security-Policy,
every outbound link is https with rel="noopener noreferrer", the deployed error page names
the site by absolute addresses only (it is served at any depth), the emergency page links to
exactly the launcher's allowlist, every `bastion-mc …` command on a hand-written page is
in the launcher's help text, the style sheet and the published pictures can fetch nothing
and say nothing about the machine they were made on, every scrollbar the style sheet colours
has a thumb of 3:1 against its track, every img has its picture's proportions, every code
block (<pre>) can take keyboard focus, and a second run in a fresh process gives the same
bytes.

What a source may contain (everything else in `{{…}}` stops the build):

  site/src/pages/NAME.html   a header (`title:`, `description:`, and if wanted `nav:`,
                             `class:`, `footer:`; written as it is, so it takes no
                             directive), a line `---`, then HTML with
      {{ROOT}} {{REPO_URL}} {{SITE_URL}}    addresses; ROOT is empty except on 404.html
      {{INCLUDE file}}                      a file below site/src/, copied as it is
      {{SECTION source "heading"}}          the body of one section of a document
      {{PARAGRAPH source "first words"}}    one paragraph of a document
      {{REQUIRE source "phrase"}}           nothing; stops the build when the document
                                            no longer says the phrase
      {{SHOT name "what it shows"}}         assets/shot-NAME.png, linked, with its size
  site/src/template.html     {{CSP}} {{TITLE}} {{DESCRIPTION}} {{HEAD}} {{BODY_CLASS}}
                             {{NAVIGATION}} {{CONTENT}} {{FOOTER}} and the addresses
  site/src/partials/*.html   the addresses (a footer); nothing (an included file)
  site/src/document.html     {{LABEL}} {{TITLE}} {{TITLE_ID}} {{PATH}} {{SOURCE}} {{HISTORY}} {{TOC}} {{BODY}}
  site/src/emergency.html    {{TITLE}} {{TITLE_ID}} {{WARNING}} {{LEAD}} {{BODY}} {{PATH}} {{COUNT}}
  site/src/robots.txt        {{SITE_URL}}

TITLE_ID is the anchor GitHub gives a document's title (its first heading): on the element
that shows the title, a link to it lands at the top of the page, as on GitHub.
"""

from __future__ import annotations

import argparse
import html
import os
import posixpath
import re
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import urllib.parse
import xml.etree.ElementTree as ElementTree
import zlib
from html.parser import HTMLParser
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import md  # noqa: E402  (the converter lives next to this file)

# --------------------------------------------------------------------------------------
# What the site consists of
# --------------------------------------------------------------------------------------

#: The policy every page carries. `style-src 'self'` and `img-src 'self'` are the only
#: things allowed at all: one style sheet, the mark and the screenshots. No script source
#: exists, so no script can run; inline styles are refused by the same rule.
CSP = "default-src 'none'; style-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'"

#: The branch that links into the repository name (the repository's default branch).
BRANCH = "main"

#: Used when --repo-url / --site-url are not given: `.invalid` never resolves (RFC 6761).
PREVIEW_REPO_URL = "https://repository.invalid"
PREVIEW_SITE_URL = "https://site.invalid/"

#: Left in an output directory so that a later run knows it may empty it.
MARKER = ".bastion-site-output"

#: Documents rendered as pages: source, page, label, navigation key, description.
DOCUMENTS = (
    ("README.md", "manual.html", "Manual", "manual",
     "The bastion-mc manual: a demo launch without an account, each layer of protection with its limits, the "
     "graphical front end, mods, the companion mod and the in-game phone, Realms, settings sync, Minecraft: Bedrock "
     "Edition, accounts, the emergency steps, the configuration and the tests."),
    ("docs/THREAT_MODEL.md", "threat-model.html", "Threat model", "threat-model",
     "What bastion-mc defends, against whom, with which mechanism and how each claim is checked, and what it does "
     "not defend."),
    ("docs/TOKEN_INJECTION.md", "token-injection.html", "Token path", "",
     "How the access token, or by default a placeholder for it, gets from the launcher into the game without "
     "appearing on a command line, in the environment or in a file, and where that guarantee ends."),
    ("docs/MC_VAULT_ACCOUNT.md", "dedicated-account.html", "Dedicated account", "",
     "Keeping other programs that run as you away from the game and the launcher: ptrace_scope and a dedicated UNIX "
     "account."),
    ("docs/MODS.md", "mods.html", "Mods", "",
     "Mods in bastion-mc: signed profiles, fetching mods from Modrinth with verified hashes (profile add, profile "
     "check), what Modrinth learns, the aliases, ViaFabric, and Simple Voice Chat with the server rule that lets its "
     "UDP through."),
    ("docs/COMPANION.md", "companion.html", "Companion mod", "",
     "The launcher's own Fabric mod on Minecraft 1.20.1 to 26.3: what it adds, its settings, when it is added, its "
     "channel to the launcher, how it is built and what was verified."),
    ("docs/PHONE.md", "phone.html", "In-game phone", "",
     "The in-game phone and its services: Spotify, Discord and the browser, clips, Realms approvals and settings sync; "
     "what each app does, its settings and limits, what leaves the machine and what the game can see."),
    ("docs/BEDROCK.md", "bedrock.html", "Bedrock Edition", "",
     "Minecraft: Bedrock Edition under bastion-mc: the Android build run by your installed mcpelauncher-client in the "
     "sandbox, what it is and is not, the licence substitution, importing a version, signing in every session, "
     "servers, Realms and event servers, capes, and what bastion-mc cannot protect: the game holds your Xbox sign-in."),
    ("docs/SCRAM_RUNBOOK.md", "emergency.html", "Emergency runbook", "emergency",
     "What to do when you think the account is compromised: remove the credentials from this machine, then secure "
     "the Microsoft account, with the exact addresses."),
)

#: The site navigation: key, page, label.
NAVIGATION = (
    ("start", "getting-started.html", "Get started"),
    ("security", "security.html", "Security"),
    ("manual", "manual.html", "Manual"),
    ("threat-model", "threat-model.html", "Threat model"),
    ("emergency", "emergency.html", "Emergency"),
)

#: Hand-written pages (site/src/pages/NAME) in the order of the site map.
PAGES = ("index.html", "getting-started.html", "security.html", "404.html")

#: The page that must link to the launcher's allowlist and to nothing else.
EMERGENCY_PAGE = "emergency.html"
RUNBOOK = "docs/SCRAM_RUNBOOK.md"
#: The phrase the launcher's own test uses to find the "use another device" warning.
DEVICE_WARNING = "from a different device"

TEXT_SUFFIXES = (".html", ".css", ".svg", ".xml", ".txt")

#: What every page's footer says, compared in lower case with white space collapsed: that
#: this is not Mojang's or Microsoft's product, and what the host of the site learns.
TRADEMARK_STATEMENT = ("not an official minecraft product", "not approved by or associated with mojang or microsoft")
HOST_STATEMENT = "ip address"


class BuildError(Exception):
    """Something in the sources is wrong; the message says where."""


class BuildErrors(BuildError):
    """Several things in the sources are wrong; `problems` has one message for each."""

    def __init__(self, problems):
        BuildError.__init__(self, "\n".join(problems))
        self.problems = list(problems)


# --------------------------------------------------------------------------------------
# Known defects in documents this site renders but does not own
# --------------------------------------------------------------------------------------
#
# GitHub hides two kinds of table mistakes instead of reporting them: it ends a table at a
# blank line (the rows after it become a paragraph of raw pipes), and it drops the cells a
# row has beyond its header without a word. The converter refuses both. An entry of REPAIRS
# states the correction for a place where a document has such a mistake and its owner has
# not fixed it yet. Each prints a notice when it applies and is silently inert once the
# document is fixed, so fixing the document needs no change here; the entry should then be
# deleted. None is needed today: the two that docs/THREAT_MODEL.md needed (section 4.3, a
# missing column "Verified by"; section 7, a blank line after row E10) were fixed in the
# document, and their entries were deleted on 2026-11-14. The two functions stay for the
# next one.


def _raw_cells(text):
    """The cells of a table row as written (escapes untouched), or None if not a row. They
    are split exactly as the converter splits a row (md.split_row: a pipe after a backslash
    never ends a cell), so that a repair counts the cells the converter will see."""
    body = text.strip(" \t")
    if not body.startswith("|") or not body.endswith("|") or len(body) < 2:
        return None
    return md.split_row(body, keep_escapes=True)


def repair_split_table(row_prefix):
    """Remove the blank lines directly above the row that starts with `row_prefix`, when
    the line above them is a table row too (a table split in two by accident)."""

    def apply(lines):
        for position, (_, text) in enumerate(lines):
            if not text.startswith(row_prefix):
                continue
            above = position - 1
            while above >= 0 and lines[above][1].strip() == "":
                above -= 1
            if above == position - 1 or above < 0 or not lines[above][1].startswith("|"):
                return lines, None
            return lines[: above + 1] + lines[position:], "joined the table that a blank line splits above the row %r" % row_prefix
        return lines, None

    return apply


def repair_missing_column(header, title, position):
    """The table under the exact header line `header` has rows with one cell more than the
    header. Give the header the column `title` at `position`, and an empty cell there to
    the rows that have the header's original width."""

    def apply(lines):
        for start, (_, text) in enumerate(lines):
            if text.strip() != header:
                continue
            width = len(_raw_cells(header))
            dashes = _raw_cells(lines[start + 1][1]) if start + 1 < len(lines) else None
            if dashes is None or len(dashes) != width or not all(re.match(r":?-+:?\Z", cell) for cell in dashes):
                return lines, None  # (not a table: the line below the header is not its row of dashes)
            end = start + 2
            rows = []
            while end < len(lines) and lines[end][1].startswith("|"):
                rows.append(_raw_cells(lines[end][1]))
                end += 1
            if not rows or any(row is None or len(row) not in (width, width + 1) for row in rows):
                return lines, None
            if not any(len(row) == width + 1 for row in rows):
                return lines, None
            repaired = list(lines)

            def widen(cells, extra):
                return "| " + " | ".join(cells[:position] + [extra] + cells[position:]) + " |"

            repaired[start] = (lines[start][0], widen(_raw_cells(header), title))
            repaired[start + 1] = (lines[start + 1][0], widen(dashes, "---"))
            for offset, row in enumerate(rows):
                if len(row) == width:
                    repaired[start + 2 + offset] = (lines[start + 2 + offset][0], widen(row, ""))
            return repaired, "gave the table under %r the column %r that %d of its rows already have" % (
                header, title, sum(len(row) == width + 1 for row in rows))
        return lines, None

    return apply


#: source -> the repairs applied to it, in order, e.g.
#: {"docs/THREAT_MODEL.md": (repair_split_table("| E11 |"),)}
REPAIRS = {}


# --------------------------------------------------------------------------------------
# Small helpers
# --------------------------------------------------------------------------------------


def escape(text):
    """Escape a value for HTML. A value is never a directive: md.escape also writes the
    first of two braces in a row as `&#123;`, so that a document that shows `{{SITE_URL}}`
    is published as it reads, and the check for a placeholder that was left unsubstituted
    sees only placeholders."""
    return md.escape(text, quote=True)


def png_size(path):
    """Width and height from a PNG file's header."""
    with open(path, "rb") as handle:
        head = handle.read(24)
    if len(head) < 24 or head[:8] != b"\x89PNG\r\n\x1a\n" or head[8:16] != b"\x00\x00\x00\rIHDR":
        raise BuildError("%s is not a PNG file" % path)
    width, height = struct.unpack(">II", head[16:24])
    if not width or not height:
        raise BuildError("%s: a PNG picture without a size (%d x %d)" % (path, width, height))
    return width, height


_DIRECTIVE = re.compile(r"\{\{([^{}]*)\}\}")
#: An id written in a page shell or a footer (not data-id and the like).
_ID_ATTRIBUTE = re.compile(r'(?<![\w-])id="([^"]*)"')
#: A file below site/src/ as a page may name it: plain segments, so no `..`, no hidden
#: file and no absolute path.
_SOURCE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*(?:/[A-Za-z0-9][A-Za-z0-9._-]*)*\Z")
#: The NAME of site/src/assets/shot-NAME.png.
_SHOT_NAME = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*\Z")


def expand(text, handlers, where):
    """Replace every {{NAME arguments}} in one pass. What a handler returns is not scanned
    again, so text that comes from a document can never be taken for a directive.

    Every directive of the text is tried even when an earlier one failed, so that one run
    names everything that is wrong in a file (each phrase of a {{REQUIRE …}} that is gone,
    not only the first)."""
    problems = []

    def failed(message):
        problems.append(message)
        return ""

    def replace(match):
        try:
            words = shlex.split(match.group(1))
        except ValueError as problem:
            return failed("%s: cannot read {{%s}}: %s" % (where, match.group(1), problem))
        if not words or words[0] not in handlers:
            return failed("%s: unknown directive {{%s}}" % (where, match.group(1)))
        try:
            return handlers[words[0]](*words[1:])
        except TypeError as problem:
            return failed("%s: wrong arguments in {{%s}}: %s" % (where, match.group(1), problem))
        except BuildErrors as several:
            for message in several.problems:
                failed(message)
            return ""
        except (BuildError, md.MarkdownError) as problem:
            return failed(str(problem))

    # A "{{" or "}}" outside a whole directive is one that lost a brace ({{ROOT}assets/…);
    # it would reach the reader as it is, and the check of the output only knows whole ones.
    position = 0
    for match in list(_DIRECTIVE.finditer(text)) + [None]:
        end = match.start() if match else len(text)
        for mark in ("{{", "}}"):
            found = text.find(mark, position, end)
            if found != -1:
                problems.append("%s: %r that is not part of a whole directive, at %r" % (where, mark, text[found : found + 40]))
        position = match.end() if match else end

    expanded = _DIRECTIVE.sub(replace, text)
    if len(problems) == 1:
        raise BuildError(problems[0])
    if problems:
        raise BuildErrors(problems)
    return expanded


def normalise_site_url(url):
    problem = md.check_https_url(url)
    if problem:
        raise BuildError("--site-url %r: %s" % (url, problem))
    if "?" in url or "#" in url:
        raise BuildError("--site-url %r must not have a query or a fragment" % url)
    return url if url.endswith("/") else url + "/"


def normalise_repo_url(url):
    problem = md.check_https_url(url)
    if problem:
        raise BuildError("--repo-url %r: %s" % (url, problem))
    if "?" in url or "#" in url:
        raise BuildError("--repo-url %r must not have a query or a fragment" % url)
    return url.rstrip("/")


# --------------------------------------------------------------------------------------
# What the code says: the emergency links and the help text
# --------------------------------------------------------------------------------------

_RUST_STRING = re.compile(r'"((?:[^"\\]|\\.)*)"', re.S)


def _rust_unescape(literal, where):
    out, index = [], 0
    while index < len(literal):
        char = literal[index]
        if char != "\\":
            out.append(char)
            index += 1
            continue
        following = literal[index + 1]
        if following == "\n":  # a line continuation swallows the white space that follows
            index += 2
            while index < len(literal) and literal[index] in " \t\n":
                index += 1
        elif following in '"\\\'':
            out.append(following)
            index += 2
        elif following == "n":
            out.append("\n")
            index += 2
        else:
            raise BuildError("%s: escape \\%s in a string this generator has to read" % (where, following))
    return "".join(out)


def runbook_links(root):
    """The addresses of `runbook::LINKS`, read from the Rust source (tests/site.rs compares
    the generated page with the compiled constant as well)."""
    where = "crates/proto/src/runbook.rs"
    source = (root / where).read_text(encoding="utf-8")
    match = re.search(r"pub const LINKS:[^=]*=\s*&\[(.*?)\n\];", source, re.S)
    if not match:
        raise BuildError("%s: cannot find the LINKS list" % where)
    strings = [_rust_unescape(literal, where) for literal in _RUST_STRING.findall(match.group(1))]
    if not strings or len(strings) % 2:
        raise BuildError("%s: LINKS is not a list of (address, description) pairs" % where)
    links = []
    for address, description in zip(strings[0::2], strings[1::2]):
        problem = md.check_https_url(address)
        if problem or description.startswith("https://"):
            raise BuildError("%s: unexpected LINKS entry %r" % (where, address))
        links.append((address, description))
    if len({address for address, _ in links}) != len(links):
        raise BuildError("%s: LINKS lists an address twice" % where)
    return links


class HelpText:
    """The launcher's usage text (`const HELP` in src/cli.rs), read well enough to tell
    whether a command shown on a page exists.

    Every line of the text that begins with two spaces and a lower-case letter is one form
    of a command: the usage, then (after two or more spaces) what it does. From the usage
    this keeps the command, its sub-command where it has one (`skin set`, `config path |
    example`), whether it takes an argument of its own (`launch <id>`) and its options."""

    #: Accepted in front of the command, as the launcher's argument parser does.
    BEFORE_THE_COMMAND = ("--help", "--version")

    def __init__(self, root):
        where = "src/cli.rs"
        source = (root / where).read_text(encoding="utf-8")
        match = re.search(r'const HELP: &str = "((?:[^"\\]|\\.)*)";', source, re.S)
        if not match:
            raise BuildError("%s: cannot find the HELP constant" % where)
        self.text = _rust_unescape(match.group(1), where)
        self.forms = {}  # command -> [(sub-commands or None, takes an argument, options)]
        for line in self.text.split("\n"):
            if not re.match(r"^  [a-z]", line):
                continue
            # The usage ends where the description begins: after a gap of two or more
            # spaces, at something that is not the next piece of usage.
            pieces = re.split(r" {2,}", line.strip())
            usage = [pieces[0]]
            for piece in pieces[1:]:
                if piece[0] not in "[<(-|":
                    break
                usage.append(piece)
            tokens = " ".join(usage).split()
            subcommands, index = [], 1
            while index < len(tokens) and re.match(r"^[a-z][a-z0-9-]*$", tokens[index]):
                subcommands.append(tokens[index])
                if index + 2 < len(tokens) and tokens[index + 1] == "|":
                    index += 2
                else:
                    index += 1
                    break
            takes_argument = index < len(tokens) and (
                tokens[index].startswith("<") or (tokens[index].startswith("[") and not tokens[index].startswith("[-"))
            )
            options = set(re.findall(r"--[a-z][a-z-]*", " ".join(tokens)))
            self.forms.setdefault(tokens[0], []).append((subcommands or None, takes_argument, options))
        if not self.forms:
            raise BuildError("%s: the HELP constant lists no commands" % where)
        self.commands = set(self.forms)
        if "--events" not in self.text:
            raise BuildError("%s: the HELP constant no longer mentions --events" % where)

    def problem_with(self, words):
        """None if `bastion-mc WORDS…` agrees with the help text, else what is wrong."""
        words = list(words)
        if words and words[0] == "--events":  # "(first argument)", says the help text
            del words[0]
        if not words or words[0] in self.BEFORE_THE_COMMAND:
            return None
        command = words[0]
        if command.startswith("<") or command in ("…", "..."):
            return None  # a placeholder such as <command>
        if command.startswith("-"):
            return "option %s in front of the command is not in the launcher's help text" % command
        if command not in self.commands:
            return "command %r is not in the launcher's help text" % command
        forms = self.forms[command]
        second = words[1] if len(words) > 1 and not words[1].startswith("-") else None
        if second is not None and not second.startswith("<") and second not in ("…", "..."):
            named = [form for form in forms if form[0] and second in form[0]]
            if named:
                forms = named
            else:
                forms = [form for form in forms if not form[0] and form[1]]
                if not forms:
                    return "%r is not a form of `%s` in the launcher's help text" % (second, command)
        allowed = set().union(*(form[2] for form in forms))
        for word in words[1:]:
            if not word.startswith("--"):
                continue
            option = re.match(r"--[A-Za-z0-9-]*", word).group(0)
            if option not in allowed:
                shown = "bastion-mc %s%s" % (command, " " + second if second and forms[0][0] else "")
                return "option %s is not an option of `%s` in the launcher's help text" % (option, shown)
        return None


#: `bastion-mc`, alone or as the last part of a path to a program (`/usr/bin/bastion-mc`,
#: `target/release/bastion-mc`, `./bastion-mc`), and the words after it up to the end of
#: the command. Not `bastion-mc-gui`, `bastion-mc.toml` or `~/.config/bastion-mc/…`.
_COMMAND = re.compile(
    r"(?<![\w/.~-])(?:(?:[\w.~-]*/)*(?:bin|release|debug|\.)/)?bastion-mc(?![\w/.-])((?:[ \t]+[^\s#|;&)]+)*)"
)
_SHELL_COMMENT = re.compile(r"(?:^|\s)#.*$")
_REDIRECTION = re.compile(r"^[0-9]*[<>]")
_PLACEHOLDER = re.compile(r"^<[^<>]+>$")


def commands_in(text):
    """Every `bastion-mc …` invocation in a piece of code text, as lists of words.

    A shell comment is not part of a command, and neither is a redirection or what follows
    it. A line that ends in a backslash goes on in the next, as in the shell (a backslash
    inside a comment does not). Where the name is followed by something that cannot be a
    command or an option (`bastion-mc (launcher)` in a drawing), it is not an invocation
    at all."""
    found, lines, continued = [], [], ""
    for line in text.split("\n"):
        line = _SHELL_COMMENT.sub("", line)
        if line.endswith("\\"):
            continued += line[:-1] + " "
            continue
        lines.append(continued + line)
        continued = ""
    if continued:
        lines.append(continued)
    for line in lines:
        for match in _COMMAND.finditer(line):
            words = []
            for word in match.group(1).split():
                if word == "\\" or (_REDIRECTION.match(word) and not _PLACEHOLDER.match(word)):
                    break
                words.append(word)
            if words and not re.match(r"^(?:[A-Za-z0-9]|-|<[^<>]+>$|…$|\.\.\.$)", words[0]):
                continue
            found.append(words)
    return found


# --------------------------------------------------------------------------------------
# The site
# --------------------------------------------------------------------------------------


class Document:
    """A parsed Markdown document, split into its title and its sections."""

    def __init__(self, source, blocks):
        self.source = source
        if not blocks or blocks[0].kind != "heading" or blocks[0].level != 1:
            raise BuildError("%s: the document must begin with its one '# ' heading" % source)
        for block in blocks[1:]:
            if block.kind == "heading" and block.level == 1:
                raise BuildError("%s:%d: a second '# ' heading (a page has one)" % (source, block.line))
        self.title = blocks[0]
        self.body = blocks[1:]

    def section(self, prefix):
        """The blocks under the heading whose text starts with `prefix`, without the
        heading itself and up to the next heading of the same or a higher level."""
        matches = [
            index
            for index, block in enumerate(self.body)
            if block.kind == "heading" and md.plain_text(block.inlines).startswith(prefix)
        ]
        if len(matches) != 1:
            raise BuildError(
                "%s: expected exactly one heading that starts with %r, found %d (the site includes that section)"
                % (self.source, prefix, len(matches))
            )
        start = matches[0]
        level = self.body[start].level
        end = start + 1
        while end < len(self.body) and not (self.body[end].kind == "heading" and self.body[end].level <= level):
            end += 1
        return self.body[start], self.body[start + 1 : end]

    def paragraph(self, prefix):
        """The one top-level paragraph whose text starts with `prefix`."""
        matches = [
            block for block in self.body if block.kind == "paragraph" and md.plain_text(block.inlines).startswith(prefix)
        ]
        if len(matches) != 1:
            raise BuildError(
                "%s: expected exactly one paragraph that starts with %r, found %d (the site includes it)"
                % (self.source, prefix, len(matches))
            )
        return matches[0]


class Site:
    def __init__(self, root, repo_url, site_url, absolute_error_page, notices=None):
        self.root = Path(root)
        self.src = self.root / "site" / "src"
        self.repo_url = repo_url
        self.site_url = site_url
        #: The error page is served for every missing address, at any depth, so relative
        #: references would break there. With a real site address it uses absolute ones.
        self.error_root = site_url if absolute_error_page else ""
        self.notices = notices if notices is not None else []
        self.page_of = {source: page for source, page, _, _, _ in DOCUMENTS}
        self._documents = {}
        self._texts = {}  # source -> the document's text without marks, white space folded
        self.links = runbook_links(self.root)
        self.help = HelpText(self.root)
        self.files = {}
        self.errors = []
        self.unbuilt = set()
        self.handwritten = []

    # -- reading ------------------------------------------------------------------------

    def read(self, relative):
        if not _SOURCE_NAME.match(relative):
            raise BuildError("site/src/%s: not a plain path below site/src/" % relative)
        path = self.src / relative
        try:
            return path.read_text(encoding="utf-8")
        except FileNotFoundError:
            raise BuildError("site/src/%s does not exist" % relative)
        except (OSError, UnicodeDecodeError) as problem:
            raise BuildError("site/src/%s: %s" % (relative, problem))

    def document(self, source):
        if source in self._documents:
            return self._documents[source]
        if source not in self.page_of:
            raise BuildError("%s is not one of the documents the site renders" % source)
        try:
            text = (self.root / source).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as problem:
            raise BuildError("%s: %s" % (source, problem))
        repairs = REPAIRS.get(source, ())

        def repair(lines):
            for step in repairs:
                lines, done = step(lines)
                if done:
                    self.notices.append("%s: %s (the source should be fixed; see docs/notes/website.md)" % (source, done))
            return lines

        document = Document(source, md.parse(text, source=source, repair=repair if repairs else None))
        self._documents[source] = document
        return document

    # -- links --------------------------------------------------------------------------

    def resolver(self, source, page):
        """Where a link written in `source` goes when it is shown on `page`."""
        base = posixpath.dirname(source)
        own_page = self.page_of[source]

        def resolve(target, kind):
            if kind[0] == "https":
                return target, True
            if kind[0] == "fragment":
                return ("#" + kind[1] if page == own_page else "%s#%s" % (own_page, kind[1])), False
            path, fragment = kind[1], kind[2]
            resolved = posixpath.normpath(posixpath.join(base, path))
            if resolved == ".." or resolved.startswith("../") or resolved.startswith("/"):
                raise ValueError("link target %r leaves the repository" % target)
            suffix = "#" + fragment if fragment else ""
            if resolved in self.page_of:
                rendered = self.page_of[resolved]
                if rendered == page and fragment:
                    return suffix, False
                return rendered + suffix, False
            on_disk = self.root / resolved
            if on_disk.is_file():
                return "%s/blob/%s/%s%s" % (self.repo_url, BRANCH, resolved, suffix), True
            if on_disk.is_dir():
                return "%s/tree/%s/%s%s" % (self.repo_url, BRANCH, resolved, suffix), True
            raise ValueError("link target %r does not exist in the repository" % target)

        return resolve

    def renderer(self, source, page, **options):
        return md.Renderer(resolve=self.resolver(source, page), source=source, **options)

    def shell_ids(self, shell, footer):
        """The ids that the page around a rendered document uses (the page shell, `shell`
        and the footer): a heading or a table row of the document cannot have one of them."""
        ids = set()
        for relative in ("template.html", shell, "partials/%s.html" % footer):
            # (an id that a directive fills in, such as {{TITLE_ID}}, is the document's own)
            ids.update(value for value in _ID_ATTRIBUTE.findall(self.read(relative)) if "{{" not in value)
        return ids

    # -- the page shell -----------------------------------------------------------------

    def navigation(self, current, root):
        items = []
        for key, page, label in NAVIGATION:
            attributes = ' aria-current="page"' if key == current else ""
            if key == "emergency":
                attributes += ' class="nav-emergency"'
            items.append('<li><a href="%s%s"%s>%s</a></li>' % (escape(root), page, attributes, escape(label)))
        return "<ul>\n%s\n</ul>" % "\n".join(items)

    def shell(self, page, title, description, content, nav="", body_class="", footer="footer", index=True):
        root = self.error_root if page == "404.html" else ""
        if index:
            canonical = self.site_url if page == "index.html" else self.site_url + page
            head = '<link rel="canonical" href="%s">' % escape(canonical)
        else:
            head = '<meta name="robots" content="noindex">'
        common = {
            "REPO_URL": lambda: escape(self.repo_url),
            "SITE_URL": lambda: escape(self.site_url),
            "ROOT": lambda: escape(root),
        }
        footer_html = expand(self.read("partials/%s.html" % footer), common, "site/src/partials/%s.html" % footer)
        handlers = dict(common)
        handlers.update(
            {
                "CSP": lambda: escape(CSP),
                "TITLE": lambda: escape(title),
                "DESCRIPTION": lambda: escape(description),
                "HEAD": lambda: head,
                "BODY_CLASS": lambda: escape(body_class),
                "NAVIGATION": lambda: self.navigation(nav, root),
                "CONTENT": lambda: content,
                "FOOTER": lambda: footer_html,
            }
        )
        return expand(self.read("template.html"), handlers, "site/src/template.html")

    # -- pieces of documents inside hand-written pages ----------------------------------

    def include_section(self, page, used_ids, source, prefix):
        document = self.document(source)
        _, blocks = document.section(prefix)
        for block in blocks:
            if block.kind == "heading":
                raise BuildError("%s:%d: the section included in %s has a sub-heading; the site cannot place it"
                                 % (source, block.line, page))
        return self.renderer(source, page, used_ids=used_ids).blocks(blocks)

    def include_paragraph(self, page, used_ids, source, prefix):
        block = self.document(source).paragraph(prefix)
        return self.renderer(source, page, used_ids=used_ids).blocks([block])

    def require(self, relative, source, phrase):
        """{{REQUIRE source "phrase"}} writes nothing. It ties a hand-written statement to
        the document that backs it: when the document no longer says `phrase`, the build
        stops and names the page, so that the statement is read against the document again.

        The phrase is compared with the document's text as a reader sees it: without the
        Markdown marks, white space folded, capitals and punctuation exactly as written.
        That text is every block in the order of the document (the title, headings,
        paragraphs, list items, the cells of a table row by row after its header, quotes,
        code blocks), joined by single spaces. So a phrase may run from one block into the
        next, which also holds the order of two list items, or the label of a table row
        together with its value. Pages rely on this: it is part of the interface."""
        wanted = " ".join(phrase.split())
        if not wanted:
            raise BuildError("site/src/%s: {{REQUIRE %s \"\"}} names no phrase" % (relative, source))
        if source not in self._texts:
            document = self.document(source)
            text = " ".join(_block_text(block) for block in [document.title] + document.body)
            self._texts[source] = " ".join(text.split())
        if wanted not in self._texts[source]:
            raise BuildError(
                "%s no longer says %r, which a statement in site/src/%s relies on ({{REQUIRE …}}): "
                "read that statement against the document again" % (source, wanted, relative)
            )
        return ""

    def screenshot(self, root, name, alt):
        if not _SHOT_NAME.match(name):
            raise BuildError("screenshot name %r: lower-case letters, digits and single hyphens only" % name)
        if not alt.strip():
            raise BuildError("screenshot %r needs a text that says what it shows" % name)
        relative = "assets/shot-%s.png" % name
        path = self.src / relative
        if not path.is_file():
            raise BuildError("site/src/%s is missing (run site/update-screenshots.sh)" % relative)
        width, height = png_size(path)
        return (
            '<a class="shot" href="%(root)s%(file)s"><img src="%(root)s%(file)s" width="%(w)d" height="%(h)d" '
            'alt="%(alt)s" loading="lazy" decoding="async"></a>'
            % {"root": escape(root), "file": relative, "w": width, "h": height, "alt": escape(alt)}
        )

    def partial(self, relative):
        text = self.read(relative)
        if "{{" in text:
            raise BuildError("site/src/%s: an included file cannot use directives itself" % relative)
        return text.rstrip("\n")

    # -- pages --------------------------------------------------------------------------

    def front_matter(self, relative):
        text = self.read(relative)
        head, separator, body = text.partition("\n---\n")
        if not separator:
            raise BuildError("site/src/%s: missing the '---' line that ends the header" % relative)
        fields = {}
        for line in head.split("\n"):
            key, colon, value = line.partition(":")
            if not colon or not key.strip() or not value.strip():
                raise BuildError("site/src/%s: header line %r is not 'key: value'" % (relative, line))
            if "{{" in value or "}}" in value:
                # (the header is written as it is; a directive there would reach the reader)
                raise BuildError("site/src/%s: header line %r: the header takes no directives" % (relative, line))
            fields[key.strip()] = value.strip()
        unknown = set(fields) - {"title", "description", "nav", "class", "footer"}
        if unknown or "title" not in fields or "description" not in fields:
            raise BuildError("site/src/%s: the header needs title and description, and knows nav, class and footer"
                             % relative)
        return fields, body

    def handwritten_page(self, name):
        relative = "pages/" + name
        fields, body = self.front_matter(relative)
        nav = fields.get("nav", "")
        if nav and nav not in [key for key, _, _ in NAVIGATION]:
            # (a misspelt key would only lose the mark on the current entry, without a word)
            raise BuildError("site/src/%s: 'nav: %s' is not an entry of the navigation (%s)"
                             % (relative, nav, ", ".join(key for key, _, _ in NAVIGATION)))
        root = self.error_root if name == "404.html" else ""
        used_ids = set()
        handlers = {
            "REPO_URL": lambda: escape(self.repo_url),
            "SITE_URL": lambda: escape(self.site_url),
            "ROOT": lambda: escape(root),
            "INCLUDE": self.partial,
            "SECTION": lambda source, prefix: self.include_section(name, used_ids, source, prefix),
            "PARAGRAPH": lambda source, prefix: self.include_paragraph(name, used_ids, source, prefix),
            "REQUIRE": lambda source, phrase: self.require(relative, source, phrase),
            "SHOT": lambda shot, alt: self.screenshot(root, shot, alt),
        }
        content = expand(body.strip("\n"), handlers, "site/src/" + relative)
        self.handwritten.append(name)
        return self.shell(name, fields["title"], fields["description"], content, nav=nav,
                          body_class=fields.get("class", ""), footer=fields.get("footer", "footer"),
                          index=name != "404.html")

    def table_of_contents(self, headings):
        """A nested list of the second- and third-level headings."""
        out, open_sub = [], False
        entries = [(level, identifier, body) for level, identifier, body, _ in headings if level in (2, 3)]
        if not entries:
            return ""
        out.append("<ol>")
        first = True
        for level, identifier, body in entries:
            link = '<a href="#%s">%s</a>' % (escape(identifier), body)
            if level == 2:
                if open_sub:
                    out.append("</ol>")
                    open_sub = False
                if not first:
                    out.append("</li>")
                out.append("<li>" + link)
                first = False
            else:
                if first:
                    raise BuildError("a document has a '###' heading before its first '##' heading")
                if not open_sub:
                    out.append("<ol>")
                    open_sub = True
                out.append("<li>%s</li>" % link)
        if open_sub:
            out.append("</ol>")
        out.append("</li>")
        out.append("</ol>")
        return "\n".join(out)

    def document_page(self, source, page, label, nav, description):
        document = self.document(source)
        renderer = self.renderer(source, page, heading_anchors=True, row_ids=True,
                                 reserved_ids=self.shell_ids("document.html", "footer"))
        # GitHub counts the title among the headings: a section of the same name is "-1",
        # and a link to the title's own anchor leads to the top of the page.
        title_id = renderer.reserve_title(document.title)
        title_html = renderer.inlines(document.title.inlines)
        title_text = md.plain_text(document.title.inlines)
        body = renderer.blocks(document.body)
        handlers = {
            "LABEL": lambda: escape(label),
            "PATH": lambda: escape(source),
            "TITLE": lambda: title_html,
            "TITLE_ID": lambda: escape(title_id),
            "TOC": lambda: self.table_of_contents(renderer.headings),
            "BODY": lambda: body,
            "SOURCE": lambda: escape("%s/blob/%s/%s" % (self.repo_url, BRANCH, source)),
            "HISTORY": lambda: escape("%s/commits/%s/%s" % (self.repo_url, BRANCH, source)),
        }
        content = expand(self.read("document.html").strip("\n"), handlers, "site/src/document.html")
        title = "%s | bastion-mc" % label if title_text == "bastion-mc" else "%s | bastion-mc" % title_text
        return self.shell(page, title, description, content, nav=nav, body_class="page-document")

    def emergency_page(self, source, page, label, nav, description):
        """The runbook, with its "which device are you on" section moved to the top and
        every address as a large target that shows what it is and where it goes."""
        document = self.document(source)
        described = dict(self.links)
        body = document.body
        starts = [index for index, block in enumerate(body) if block.kind == "heading" and block.level == 2]
        if not starts:
            raise BuildError("%s: the runbook has no '## ' sections" % source)
        warning = None
        for number, start in enumerate(starts):
            end = starts[number + 1] if number + 1 < len(starts) else len(body)
            text = " ".join(" ".join(_block_text(block) for block in body[start:end]).split())
            if DEVICE_WARNING in text:
                warning = (start, end)
                break
        if warning is None:
            raise BuildError("%s: no section says %r; the emergency page puts that warning first"
                             % (source, DEVICE_WARNING))
        lead = body[: starts[0]]

        def target(url):
            if url not in described:
                raise BuildError("%s: %s is not in runbook::LINKS (crates/proto/src/runbook.rs)" % (source, url))
            return (
                '<a class="tap" href="%(url)s" rel="%(rel)s"><span class="tap-what">%(what)s</span> '
                '<span class="tap-where">%(url)s</span></a>'
                % {"url": escape(url), "rel": md.Renderer.OUTBOUND_REL, "what": escape(described[url])}
            )

        renderer = md.Renderer(resolve=self.resolver(source, page), autolink=target, source=source,
                               reserved_ids=self.shell_ids("emergency.html", "footer-minimal"))
        pieces = (("LEAD", lead), ("BEFORE", body[starts[0] : warning[0]]), ("WARNING", body[warning[0] : warning[1]]),
                  ("AFTER", body[warning[1] :]))
        rendered = []

        def piece(name):
            """Every piece is rendered once, in the order of the document, so that headings
            are numbered as GitHub numbers them, whatever order the shell shows them in. (A
            failure is raised again by each directive that asks; the build names it once.)"""
            if not rendered:
                try:
                    done = {"TITLE_ID": renderer.reserve_title(document.title),
                            "TITLE": renderer.inlines(document.title.inlines)}
                    for key, blocks in pieces:
                        done[key] = renderer.blocks(blocks)
                    rendered.append(done)
                except (BuildError, md.MarkdownError) as problem:
                    rendered.append(problem)
            if isinstance(rendered[0], Exception):
                raise rendered[0]
            return rendered[0][name]

        handlers = {
            "TITLE": lambda: piece("TITLE"),
            "TITLE_ID": lambda: escape(piece("TITLE_ID")),
            "WARNING": lambda: piece("WARNING"),
            "LEAD": lambda: piece("LEAD"),
            "BODY": lambda: "\n".join(part for part in (piece("BEFORE"), piece("AFTER")) if part),
            "PATH": lambda: escape(source),
            "COUNT": lambda: str(len(self.links)),
        }
        content = expand(self.read("emergency.html").strip("\n"), handlers, "site/src/emergency.html")
        title = "%s | bastion-mc" % md.plain_text(document.title.inlines)
        return self.shell(page, title, description, content, nav=nav, body_class="page-emergency", footer="footer-minimal")

    # -- everything ---------------------------------------------------------------------

    def build(self, keep_going=False):
        """Render every file of the site: {name: bytes}.

        Each page is attempted even when an earlier one failed, so that one run names
        everything that is wrong. Then the build stops with all of it (BuildErrors) unless
        `keep_going` is set: in that case the files that could be made are returned and
        `self.errors` says what is missing."""
        files = {}
        self.errors = []

        def attempt(name, make):
            try:
                files[name] = make()
            except (BuildError, md.MarkdownError) as problem:
                # One cause (the shell, a document that does not parse) stops many pages,
                # and one page can have several things wrong with it.
                for message in getattr(problem, "problems", None) or [str(problem)]:
                    if message not in self.errors:
                        self.errors.append(message)

        for name in PAGES:
            attempt(name, lambda name=name: self.handwritten_page(name))
        for source, page, label, nav, description in DOCUMENTS:
            make = self.emergency_page if page == EMERGENCY_PAGE else self.document_page
            attempt(page, lambda make=make, entry=(source, page, label, nav, description): make(*entry))

        listed = ["index.html", "getting-started.html", "security.html"] + [page for _, page, _, _, _ in DOCUMENTS]
        entries = "".join(
            "  <url><loc>%s</loc></url>\n" % escape(self.site_url if page == "index.html" else self.site_url + page)
            for page in listed
            if page in files  # (all of them, except in a --keep-going build)
        )
        files["sitemap.xml"] = (
            '<?xml version="1.0" encoding="UTF-8"?>\n'
            '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n%s</urlset>\n' % entries
        )
        attempt("robots.txt", lambda: expand(self.read("robots.txt"), {"SITE_URL": lambda: self.site_url},
                                             "site/src/robots.txt"))

        encoded = {name: text.encode("utf-8") for name, text in files.items()}
        assets = self.src / "assets"
        if not assets.is_dir():
            self.errors.append("site/src/assets does not exist (the style sheet, the mark and the screenshots)")
        else:
            for path in sorted(assets.iterdir()):
                if not path.is_file() or path.name.startswith("."):
                    self.errors.append("site/src/assets/%s: only plain files belong here" % path.name)
                elif path.suffix not in (".css", ".svg", ".png"):
                    self.errors.append("site/src/assets/%s: only .css, .svg and .png files are published" % path.name)
                else:
                    encoded["assets/" + path.name] = path.read_bytes()
        encoded[MARKER] = b"Written by site/build.py; a later run may empty this directory.\n"
        self.files = encoded
        self.unbuilt = (set(PAGES) | {page for _, page, _, _, _ in DOCUMENTS}) - set(files)
        if self.errors and not keep_going:
            raise BuildErrors(self.errors)
        return encoded


def _block_text(block):
    """All text below a block, in reading order: its own, then that of the blocks, items
    and cells inside it (a table's header first, then row by row), joined by spaces.
    White space is not folded here; whoever searches the text for a phrase does that."""
    parts = []
    if block.inlines:
        parts.append(md.plain_text(block.inlines))
    if block.text:
        parts.append(block.text)
    for item in block.items or ():
        parts.extend(_block_text(child) for child in item)
    for child in block.blocks or ():
        parts.append(_block_text(child))
    for row in [block.header or []] + list(block.rows or []):
        parts.extend(md.plain_text(cell) for cell in row)
    return " ".join(parts)


# --------------------------------------------------------------------------------------
# Writing
# --------------------------------------------------------------------------------------


def write_output(files, out):
    """Make `out` contain exactly `files`. An existing directory is emptied only if an
    earlier run of this generator left its marker in it."""
    out = Path(out)
    if out.is_symlink() or (out.exists() and not out.is_dir()):
        raise BuildError("%s is not a directory" % out)
    if out.is_dir() and any(out.iterdir()):
        if not (out / MARKER).is_file():
            raise BuildError("%s is not empty and was not written by this generator; refusing to replace it" % out)
        shutil.rmtree(str(out))
    out.mkdir(parents=True, exist_ok=True)
    for name in sorted(files):
        path = out / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(files[name])


# --------------------------------------------------------------------------------------
# Checking what was written
# --------------------------------------------------------------------------------------

VOID = frozenset("area base br col embed hr img input link meta source track wbr".split())
#: Elements that run something, load something, send something, or are a link or a
#: resource that the checks below would not see (`area` is a link, `math` may carry one).
FORBIDDEN_ELEMENTS = frozenset(
    "script style iframe frame frameset fencedframe object embed applet form base audio video source track portal "
    "input button select textarea canvas noscript template area map math".split()
)
#: Attributes that make a browser fetch or send something the checks below would not see.
FORBIDDEN_ATTRIBUTES = frozenset(
    "srcset imagesrcset ping attributionsrc formaction background poster manifest lowsrc dynsrc".split()
)
BLOCK_ELEMENTS = frozenset(
    "address article aside blockquote details div dl dd dt fieldset figcaption figure footer h1 h2 h3 h4 h5 h6 "
    "header hr li main nav ol p pre section summary table tbody thead tr td th ul".split()
)
SVG_REFERENCES = frozenset("image use feimage".split())
#: SVG animation (SMIL). It sets attributes while a drawing is shown, an address among them
#: (`<set attributeName="href" to="…">`), where no check here reads it; and nothing on this
#: site can stop it for a reader who asked for reduced motion (no style sheet reaches it).
SVG_ANIMATION = frozenset("animate animatecolor animatemotion animatetransform discard mpath set".split())


class PageScan(HTMLParser):
    """Reads one generated page and records what the checks need."""

    def __init__(self, name):
        HTMLParser.__init__(self, convert_charrefs=True)
        self.name = name
        self.problems = []
        self.stack = []
        self.ids = []
        self.links = []  # (href, rel, order)
        self.resources = []  # (what, url)
        self.images = []
        self.metas = []  # (attributes, order)
        self.h1 = 0
        self.landmarks = set()
        self.lang = None
        self.title = ""
        self.code = []  # text of <code> and <pre> elements
        self.text = []  # (order, text)
        self.order = 0
        self.first_load = None
        self.has_doctype = False
        self.mains = []  # the id of every <main> (None where it has none)
        self.headings = []  # the level of every h1…h6, in order
        self.footer = []  # the text inside <footer>
        self._code_depth = 0
        self._in_title = False

    def problem(self, message):
        line, _ = self.getpos()
        self.problems.append("%s:%d: %s" % (self.name, line, message))

    def handle_decl(self, decl):
        if decl.lower() == "doctype html":
            self.has_doctype = True

    def handle_comment(self, data):
        self.problem("HTML comment in the output")

    def handle_startendtag(self, tag, attrs):
        self.element(tag, attrs, closed=True)

    def handle_starttag(self, tag, attrs):
        self.element(tag, attrs, closed=False)

    def element(self, tag, attrs, closed):
        self.order += 1
        # An attribute written without a value (`<a href>`) has the empty string as its
        # value, as a browser reads it; the parser gives None.
        attrs = [(name, "" if value is None else value) for name, value in attrs]
        names = [name for name, _ in attrs]
        if len(names) != len(set(names)):
            self.problem("<%s> repeats an attribute" % tag)
        attributes = dict(attrs)
        in_svg = "svg" in self.stack or tag == "svg"
        if tag in FORBIDDEN_ELEMENTS:
            self.problem("<%s> is not allowed on this site" % tag)
        if tag == "image" and not in_svg:
            self.problem("<image> outside SVG (a browser reads it as <img>, and the checks of <img> would not see it)")
        if tag in BLOCK_ELEMENTS and "p" in self.stack and not in_svg:
            self.problem("<%s> inside <p> (a browser would end the paragraph there)" % tag)
        if tag == "a" and "a" in self.stack:
            self.problem("<a> inside <a>")
        for name, value in attrs:
            if name.startswith("on"):
                self.problem("event handler attribute %s" % name)
            if name == "style":
                self.problem("style attribute (the Content-Security-Policy forbids inline styles)")
            if name in FORBIDDEN_ATTRIBUTES:
                self.problem("attribute %s is not allowed on this site" % name)
            if name == "id":
                self.ids.append(value)
            if in_svg and value and _outside_reference(value):
                self.problem("attribute %s on <%s> inside inline SVG: %s" % (name, tag, _outside_reference(value)))
        if tag == "html":
            self.lang = attributes.get("lang")
        elif tag == "meta":
            self.metas.append((attributes, self.order, bool(self.stack) and self.stack[-1] == "head"))
        elif tag == "link":
            rel = attributes.get("rel", "")
            if rel not in ("stylesheet", "icon", "canonical"):
                self.problem('<link rel="%s"> is not allowed on this site' % rel)
            elif rel == "canonical":
                self.resources.append(("canonical", attributes.get("href", "")))
            else:
                self.load(rel, attributes.get("href", ""))
        elif tag == "img":
            self.load("image", attributes.get("src", ""))
            self.images.append(attributes)
        elif tag == "a":
            if "href" not in attributes:
                self.problem("<a> without href")
            else:
                self.links.append((attributes["href"], attributes.get("rel", ""), self.order))
            if "target" in attributes:
                self.problem("<a target> (links open where the reader decides)")
        elif in_svg and tag in SVG_REFERENCES:
            self.problem("<%s> inside inline SVG could load a resource" % tag)
        elif in_svg and tag in SVG_ANIMATION:
            self.problem("<%s> inside inline SVG (an animation can set an address that no check reads, and it moves "
                         "whatever the reader's reduced-motion setting)" % tag)
        if in_svg:
            for name, value in attrs:
                if name in ("href", "xlink:href") and tag != "a":
                    self.problem("%s on <%s> inside inline SVG" % (name, tag))
        if tag == "h1":
            self.h1 += 1
        if re.match(r"^h[1-6]$", tag) and not in_svg:
            self.headings.append(int(tag[1]))
        if tag in ("header", "nav", "main", "footer"):
            self.landmarks.add(tag)
        if tag == "main":
            self.mains.append(attributes.get("id"))
        if tag == "title" and not in_svg:
            self._in_title = True
        if tag == "pre" and not in_svg and attributes.get("tabindex") != "0":
            # (web-2: whether a block scrolls depends on the window and the reader's font
            # size, which no page knows, so every one takes focus)
            self.problem('<pre> without tabindex="0" (a code block wider than its column scrolls inside its own '
                         "box, and a reader on the keyboard has to be able to reach what is hidden)")
        if tag in ("code", "pre"):
            self._code_depth += 1
            if self._code_depth == 1:
                self.code.append("")
        if not closed and tag not in VOID and not (tag == "image" and not in_svg):  # (<image> is <img>: no end tag)
            self.stack.append(tag)

    def handle_endtag(self, tag):
        if tag in VOID:
            self.problem("</%s> for an element that has no end tag" % tag)
            return
        if not self.stack or self.stack[-1] != tag:
            self.problem("</%s> does not close <%s>" % (tag, self.stack[-1] if self.stack else "nothing"))
            if tag in self.stack:
                while self.stack and self.stack[-1] != tag:
                    self.stack.pop()
            else:
                return
        self.stack.pop()
        if tag == "title":
            self._in_title = False
        if tag in ("code", "pre"):
            self._code_depth -= 1

    def handle_data(self, data):
        if self._in_title:
            self.title += data
        if self._code_depth:
            self.code[-1] += data
        if data.strip():
            self.order += 1
            self.text.append((self.order, data))
            if "footer" in self.stack:
                self.footer.append(data)

    def load(self, what, url):
        if self.first_load is None:
            self.first_load = self.order
        self.resources.append((what, url))

    def finish(self):
        self.close()
        if self.stack:
            self.problems.append("%s: unclosed <%s>" % (self.name, ">, <".join(self.stack)))


_CSS_URL = re.compile(r"url\(|@import|expression\(|javascript:|https?:|//[A-Za-z]", re.I)
#: `url(…)` in the value of an SVG attribute, with what it names.
_URL_FUNCTION = re.compile(r"url\(\s*['\"]?\s*([^'\")\s]*)", re.I)
#: Chunks that carry text, a time or camera data: none of it is the picture, and all of it
#: can say where and by whom a file was made.
_PNG_ABOUT_ITS_MAKING = (b"tEXt", b"zTXt", b"iTXt", b"eXIf", b"tIME")


def _outside_reference(value):
    """Why this SVG attribute value could make a browser fetch something, or None.

    `url(#id)` (a gradient, a clip path) stays inside the file. Any other `url(…)` names
    something outside it, and a backslash could spell one where this check does not look
    (the values of presentation attributes are read as CSS, escapes included)."""
    if "\\" in value:
        return "a backslash (a CSS escape can spell url(…) unseen)"
    for match in _URL_FUNCTION.finditer(value):
        if not match.group(1).startswith("#"):
            return "url(…) that names something outside the file"
    return None


def _relative_luminance(colour):
    """WCAG 2's relative luminance of a #rrggbb colour."""

    def channel(value):
        value /= 255
        return value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4

    red, green, blue = (int(colour[index : index + 2], 16) for index in (1, 3, 5))
    return 0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)


def contrast_ratio(first, second):
    """WCAG 2 contrast of two #rrggbb colours."""
    lighter, darker = sorted((_relative_luminance(first), _relative_luminance(second)), reverse=True)
    return (lighter + 0.05) / (darker + 0.05)


_CSS_COMMENT = re.compile(r"/\*.*?\*/", re.S)
_CSS_COLOUR = re.compile(r"#(?:[0-9a-fA-F]{6}|[0-9a-fA-F]{3})\Z")


def scrollbar_problems(css):
    """What is wrong with the scrollbar colours a style sheet sets. A scrollbar whose colours
    a page sets is a control of that page, not the browser's own (WCAG 1.4.11 no longer
    exempts it): its thumb needs 3:1 against its track. A colour is #rrggbb, #rgb, or
    var() of a custom property set to one of them in the first :root rule (the screen's; a
    print rule may set its own, and nothing scrolls on paper). Anything else is reported as
    unreadable, so that the check cannot pass by not understanding."""
    css = _CSS_COMMENT.sub("", css)
    root = re.search(r":root\s*\{([^}]*)\}", css)
    tokens = dict(re.findall(r"(--[\w-]+)\s*:\s*(#[0-9a-fA-F]{3,6})\s*;", root.group(1))) if root else {}
    problems = []
    for value in re.findall(r"scrollbar-color\s*:\s*([^;}]+)", css):
        value = " ".join(value.split())
        if value in ("auto", "inherit", "initial", "unset", "revert", "revert-layer"):
            continue
        colours = []
        for part in value.split():
            reference = re.match(r"var\((--[\w-]+)\)\Z", part)
            colour = tokens.get(reference.group(1), "") if reference else part
            if not _CSS_COLOUR.match(colour):
                break
            if len(colour) == 4:
                colour = "#" + "".join(digit * 2 for digit in colour[1:])
            colours.append(colour.lower())
        if len(colours) != 2 or len(value.split()) != 2:
            problems.append("scrollbar-color %r: write two colours as #rrggbb, or var() of custom properties set to "
                            "them in :root (this check reads nothing else)" % value)
            continue
        ratio = contrast_ratio(*colours)
        if ratio < 3:
            problems.append("scrollbar-color %s: the thumb %s has %.2f:1 against its track %s; a scrollbar that the "
                            "page colours needs 3:1 (WCAG 1.4.11)" % (value, colours[0], ratio, colours[1]))
    return problems


def picture_size(data, name):
    """The intrinsic width and height of a published picture, or a string that says why
    there is none: a PNG's IHDR; an SVG's width and height (plain numbers or px), else its
    viewBox."""
    if name.endswith(".png"):
        if len(data) < 24 or data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
            return "not a PNG picture"
        return struct.unpack(">II", data[16:24])
    try:
        root = ElementTree.fromstring(data)
    except ElementTree.ParseError as problem:
        return "not well-formed XML: %s" % problem
    sizes = [re.match(r"\s*([0-9]+(?:\.[0-9]+)?)(?:px)?\s*\Z", root.get(side, "")) for side in ("width", "height")]
    if all(sizes):
        return tuple(float(size.group(1)) for size in sizes)
    box = root.get("viewBox", "").replace(",", " ").split()
    if len(box) == 4:
        try:
            return float(box[2]), float(box[3])
        except ValueError:
            pass
    return _NO_SIZE


_NO_SIZE = "the picture gives no size (width and height, or a viewBox)"


def png_problem(data):
    """What is wrong with these bytes as a published PNG picture, or None.

    A picture on this site is exactly its image data: complete (every chunk whole and with
    the right checksum, IHDR first, nothing after IEND) and without chunks that describe
    how it was made."""
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return "not a PNG file"
    position, kinds = 8, []
    while position < len(data):
        if position + 12 > len(data):
            return "the PNG file is cut off"
        length, kind = struct.unpack(">I4s", data[position : position + 8])
        if not (kind.isalpha() and kind.isascii()):  # (never written to a terminal as it is)
            return "the PNG file has a chunk whose name is not four letters (bytes %s)" % kind.hex()
        name = kind.decode("ascii")
        end = position + 12 + length
        if end > len(data):
            return "the PNG file is cut off inside its %s chunk" % name
        (checksum,) = struct.unpack(">I", data[end - 4 : end])
        if zlib.crc32(data[position + 4 : end - 4]) != checksum:
            return "the %s chunk of the PNG file has a wrong checksum" % name
        if kind == b"IHDR" and (length != 13 or 0 in struct.unpack(">II", data[position + 8 : position + 16])):
            return "the header of the PNG file gives no size (IHDR is 13 bytes, width and height above zero)"
        if kind == b"IEND" and length:
            return "the IEND chunk of the PNG file is not empty"
        kinds.append(kind)
        position = end
        if kind == b"IEND":
            break
    if not kinds or kinds[0] != b"IHDR" or kinds[-1] != b"IEND" or b"IDAT" not in kinds:
        return "not a complete PNG file (IHDR first, image data, IEND last)"
    if position != len(data):
        return "%d byte(s) after the end of the PNG picture" % (len(data) - position)
    for kind in kinds:
        if kind in _PNG_ABOUT_ITS_MAKING:
            return "the PNG file carries a %s chunk (text, a time or camera data about how it was made)" % kind.decode("ascii")
    return None


def check_output(out, site, explicit_urls):
    """Verify the written site. Returns (problems, facts for the summary)."""
    out = Path(out)
    problems = []
    pages = {}
    files = set()
    for path in sorted(out.rglob("*")):
        if path.is_file():
            files.add(path.relative_to(out).as_posix())
    for name in sorted(files):
        if name.endswith(".html"):
            scan = PageScan(name)
            scan.feed((out / name).read_text(encoding="utf-8"))
            scan.finish()
            pages[name] = scan

    site_url = site.site_url
    referenced = set()
    unchecked = {}  # a page that could not be built (--keep-going) -> links to it
    internal_links = outbound_links = 0

    def local_target(page, url):
        """The output file a same-site URL names, with its fragment; None if it leaves the site."""
        if page == "404.html" and site.error_root and url.startswith(site.error_root):
            url = url[len(site.error_root) :]
        if re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", url) or url.startswith("//") or url.startswith("/") or "\\" in url:
            return None
        path, _, fragment = url.partition("#")
        path = path.partition("?")[0]
        target = posixpath.normpath(posixpath.join(posixpath.dirname(page), path)) if path else page
        return target, fragment

    for name, scan in sorted(pages.items()):
        problems.extend(scan.problems)

        def bad(message, name=name):
            problems.append("%s: %s" % (name, message))

        # Structure a person or a screen reader relies on.
        if not scan.has_doctype:
            bad("no <!doctype html>")
        if scan.lang != "en":
            bad('<html> needs lang="en"')
        if scan.h1 != 1:
            bad("has %d <h1> elements, a page has one" % scan.h1)
        if not scan.title.strip():
            bad("empty <title>")
        for landmark in ("header", "nav", "main", "footer"):
            if landmark not in scan.landmarks:
                bad("no <%s> landmark" % landmark)
        if scan.headings and scan.headings[0] != 1:
            bad("the first heading is <h%d>; a page begins with its <h1>" % scan.headings[0])
        for before, after in zip(scan.headings, scan.headings[1:]):
            if after > before + 1:
                bad("<h%d> directly after <h%d> skips a heading level" % (after, before))
                break
        duplicates = sorted({identifier for identifier in scan.ids if scan.ids.count(identifier) > 1})
        if duplicates:
            bad("duplicate id: %s" % ", ".join(duplicates))
        footer = " ".join(" ".join(scan.footer).split()).lower()
        for phrase in TRADEMARK_STATEMENT:
            if phrase not in footer:
                bad("the footer must carry the trademark statement (missing: %r)" % phrase)
        if HOST_STATEMENT not in footer:
            bad("the footer must say what the host of the site necessarily sees (missing: %r)" % HOST_STATEMENT)
        for attributes in scan.images:
            for needed in ("alt", "width", "height"):
                if needed not in attributes:
                    bad("<img src=%r> without %s" % (attributes.get("src"), needed))
            if not (attributes.get("width", "").isdigit() and attributes.get("height", "").isdigit()):
                bad("<img src=%r> needs its intrinsic width and height" % attributes.get("src"))
                continue
            # The size must have the picture's proportions (it may be drawn smaller, like the
            # mark): with a stale size the browser keeps a box of the wrong shape until the
            # picture has loaded, and nobody notices a drawing that grew.
            source = attributes.get("src", "")
            target = local_target(name, source) if source.strip() else None
            if target is None or target[0] not in files or not target[0].endswith((".png", ".svg")):
                continue  # (an empty, foreign or missing address is reported with what the page loads)
            size = picture_size((out / target[0]).read_bytes(), target[0])
            width, height = int(attributes["width"]), int(attributes["height"])
            if isinstance(size, str) or 0 in size:
                if size == _NO_SIZE:  # (a broken file is reported with the published files)
                    bad("<img src=%r>: %s" % (source, size))
            elif not (width and height and (height == round(width * size[1] / size[0])
                                             or width == round(height * size[0] / size[1]))):
                bad("<img src=%r width=%r height=%r>: the picture is %s x %s; an img's width and height must have its "
                    "proportions" % (source, attributes["width"], attributes["height"],
                                     "%g" % size[0], "%g" % size[1]))

        # The page's own policy, before anything is loaded.
        policies = [meta for meta in scan.metas if meta[0].get("http-equiv", "").lower() == "content-security-policy"]
        if len(policies) != 1 or policies[0][0].get("content") != CSP:
            bad("must carry exactly this Content-Security-Policy: %s" % CSP)
        elif not policies[0][2]:
            bad("the Content-Security-Policy must stand in <head> (a browser ignores it anywhere else)")
        elif scan.first_load is not None and policies[0][1] > scan.first_load:
            bad("the Content-Security-Policy must come before the first thing the page loads")
        for attributes, _, _ in scan.metas:
            if "http-equiv" in attributes and attributes["http-equiv"].lower() != "content-security-policy":
                bad('<meta http-equiv="%s"> is not allowed' % attributes["http-equiv"])
        referrers = [meta for meta in scan.metas if meta[0].get("name", "").lower() == "referrer"]
        if len(referrers) != 1 or referrers[0][0].get("content") != "no-referrer" or not referrers[0][2]:
            bad('must carry exactly one <meta name="referrer" content="no-referrer">, in <head>')
        if not any(a.get("name") == "viewport" for a, _, _ in scan.metas):
            bad("no viewport meta element")
        if not any(a.get("name") == "description" and a.get("content", "").strip() for a, _, _ in scan.metas):
            bad("no description")

        # Nothing is loaded from anywhere else.
        for what, url in scan.resources:
            if what == "canonical":
                if not url.startswith(site_url):
                    bad("canonical address %r is not under the site address" % url)
                continue
            if not url.strip():
                bad("%s with an empty address" % what)  # (it would name the page itself)
                continue
            target = local_target(name, url)
            if target is None:
                bad("%s %r would be loaded from another origin" % (what, url))
            elif target[0] not in files:
                bad("%s %r does not exist in the output" % (what, url))
            else:
                referenced.add(target[0])

        # Links.
        if len(scan.mains) != 1 or not scan.mains[0]:
            bad("needs exactly one <main>, with an id for the skip link to point to")
        elif not scan.links or scan.links[0][0] != "#" + scan.mains[0]:
            bad('the first link of the page must be the skip link to <main id="%s">' % scan.mains[0])
        for href, rel, _ in scan.links:
            target = local_target(name, href)
            if target is None and href.startswith(site_url):
                # (in a preview that address does not even resolve; deployed, it leaves a
                # copy of the site that was opened from a disk)
                bad("link %r names this site by its address; inside the site every link is relative, so that the "
                    "site also works from a file system" % href)
                continue
            if target is None:
                outbound_links += 1
                reason = md.check_https_url(href)
                if reason:
                    bad("outbound link %r: %s" % (href, reason))
                if sorted(rel.split()) != ["noopener", "noreferrer"]:
                    bad('outbound link %r needs rel="noopener noreferrer"' % href)
                continue
            internal_links += 1
            if rel:
                bad("internal link %r carries rel=%r" % (href, rel))
            path, fragment = target
            if path in site.unbuilt:
                unchecked[path] = unchecked.get(path, 0) + 1
                continue
            if path not in files:
                bad("link %r: %s does not exist in the output" % (href, path))
                continue
            referenced.add(path)
            if fragment:
                # (a browser decodes %XX before it looks for the id)
                if path not in pages:
                    bad("link %r: a fragment on something that is not a page" % href)
                elif fragment not in pages[path].ids and urllib.parse.unquote(fragment) not in pages[path].ids:
                    bad("link %r: %s has no element with id %r" % (href, path, fragment))

        # The error page is served for every missing address, at any depth, where a relative
        # reference points somewhere else than it does here: deployed, everything on it that
        # stays on the site is absolute, except a fragment of the page itself.
        if name == "404.html" and site.error_root:
            references = [("link", href) for href, _, _ in scan.links]
            references += [(what, url) for what, url in scan.resources if what != "canonical"]
            for what, url in references:
                if url.startswith(("#", site.error_root)) or local_target(name, url) is None:
                    continue
                bad("%s %r is relative, but the error page is served at any depth (write {{ROOT}} before it)"
                    % (what, url))

    for path in sorted(unchecked):
        problems.append("%d link(s) to %s could not be checked: that page was not built" % (unchecked[path], path))

    # The emergency page: the launcher's allowlist, exactly, and the warning before any of it.
    allowed = [address for address, _ in site.links]
    if EMERGENCY_PAGE not in pages:
        problems.append("%s was not generated" % EMERGENCY_PAGE)
    else:
        scan = pages[EMERGENCY_PAGE]
        outbound = [(href, order) for href, _, order in scan.links if local_target(EMERGENCY_PAGE, href) is None]
        found = sorted({href for href, _ in outbound})
        if found != sorted(allowed):
            extra = sorted(set(found) - set(allowed))
            missing = sorted(set(allowed) - set(found))
            problems.append(
                "%s: its outbound links must be exactly runbook::LINKS; not in the list: %s; missing from the page: %s"
                % (EMERGENCY_PAGE, extra or "none", missing or "none")
            )
        warned = [order for order, text in scan.text if DEVICE_WARNING in " ".join(text.split())]
        if not warned:
            problems.append("%s: the warning %r is missing" % (EMERGENCY_PAGE, DEVICE_WARNING))
        elif outbound and min(order for _, order in outbound) < warned[0]:
            problems.append("%s: an outbound link comes before the warning to use another device" % EMERGENCY_PAGE)

    # Commands shown on hand-written pages exist. Rendered documents are only reported.
    for name, scan in sorted(pages.items()):
        for block in scan.code:
            for words in commands_in(block):
                reason = site.help.problem_with(words)
                if not reason:
                    continue
                message = "%s: `bastion-mc %s`: %s" % (name, " ".join(words), reason)
                if name in site.handwritten:
                    problems.append(message)
                else:
                    site.notices.append(message + " (in a rendered document: not an error here)")

    # Style sheets and images.
    for name in sorted(files):
        if name.endswith(".css"):
            try:
                text = (out / name).read_text(encoding="utf-8")
            except UnicodeDecodeError as problem:
                problems.append("%s: not UTF-8: %s" % (name, problem))
                continue
            match = _CSS_URL.search(text)
            if match:
                problems.append("%s: %r could make the style sheet load something" % (name, match.group(0)))
            if "\\" in text:
                problems.append("%s: a backslash (a CSS escape can spell url( where this check does not see it; "
                                "write the character itself)" % name)
            for problem in scrollbar_problems(text):
                problems.append("%s: %s" % (name, problem))
        elif name.endswith(".svg"):
            # A published SVG can also be opened by itself, as a document of this site with
            # no policy of its own: it must not be able to run or fetch anything then.
            try:
                tree = ElementTree.parse(str(out / name))
                text = (out / name).read_text(encoding="utf-8")
            except ElementTree.ParseError as problem:
                problems.append("%s: not well-formed XML: %s" % (name, problem))
                continue
            except UnicodeDecodeError:
                continue  # (reported below: not UTF-8)
            if re.search(r"<!DOCTYPE|<!ENTITY", text, re.I):
                problems.append("%s: a document type declaration is not allowed in a published SVG "
                                "(it can define entities that this check does not read)" % name)
            if re.search(r"<\?(?!xml[\s?])", text):
                problems.append("%s: a processing instruction is not allowed in a published SVG "
                                "(xml-stylesheet would load a style sheet)" % name)
            for element in tree.iter():
                tag = element.tag.rpartition("}")[2].lower()
                if tag in ("script", "style", "image", "use", "feimage", "foreignobject", "a"):
                    problems.append("%s: <%s> is not allowed in a published SVG" % (name, tag))
                elif tag in SVG_ANIMATION:
                    problems.append("%s: <%s> is not allowed in a published SVG (an animation can set an address "
                                    "that no check reads, and it moves whatever the reader's reduced-motion setting)"
                                    % (name, tag))
                for attribute, value in element.attrib.items():
                    plain = attribute.rpartition("}")[2].lower()
                    if plain.startswith("on") or plain in ("href", "style"):
                        problems.append("%s: attribute %s is not allowed in a published SVG" % (name, plain))
                    reason = _outside_reference(value)
                    if reason:
                        problems.append("%s: attribute %s: %s" % (name, plain, reason))
        elif name.endswith(".png"):
            reason = png_problem((out / name).read_bytes())
            if reason:
                problems.append("%s: %s" % (name, reason))
    for name in sorted(files):
        if name.startswith("assets/") and name not in referenced:
            problems.append("%s is published but no page uses it" % name)

    # The two machine-readable files.
    try:
        sitemap = ElementTree.parse(str(out / "sitemap.xml"))
        locations = [element.text or "" for element in sitemap.iter() if element.tag.endswith("}loc")]
        if not locations:
            problems.append("sitemap.xml lists nothing")
        for location in locations:
            page = location[len(site_url) :] if location.startswith(site_url) else None
            if page is None or (page or "index.html") not in pages or page == "404.html":
                problems.append("sitemap.xml: %r is not a page of this site" % location)
    except (OSError, ElementTree.ParseError) as problem:
        problems.append("sitemap.xml: %s" % problem)
    robots = (out / "robots.txt").read_text(encoding="utf-8") if "robots.txt" in files else ""
    if "Sitemap: %ssitemap.xml\n" % site_url not in robots:
        problems.append("robots.txt does not name the sitemap under the site address")

    # No token left behind, and the addresses that were asked for are the ones written.
    for name in sorted(files):
        if not name.endswith(TEXT_SUFFIXES):
            continue
        try:
            text = (out / name).read_text(encoding="utf-8")
        except UnicodeDecodeError:
            if not name.endswith(".css"):  # (a style sheet was reported above)
                problems.append("%s: not UTF-8" % name)
            continue
        leftover = re.search(r"\{\{[A-Z][^{}]*\}\}", text)
        if leftover:
            problems.append("%s: unsubstituted %s" % (name, leftover.group(0)))
        if explicit_urls:
            for preview in (PREVIEW_REPO_URL, PREVIEW_SITE_URL):
                if preview in text:
                    problems.append("%s: still contains the preview address %s" % (name, preview))

    facts = {"pages": len(pages), "internal": internal_links, "outbound": outbound_links, "files": len(files)}
    return problems, facts


def check_deterministic(out, arguments):
    """Build again in a fresh interpreter with another hash seed; compare every byte."""
    out = Path(out)
    with tempfile.TemporaryDirectory(prefix="bastion-site-check-") as scratch:
        second = Path(scratch) / "again"
        environment = dict(os.environ)
        environment["PYTHONHASHSEED"] = "1" if os.environ.get("PYTHONHASHSEED") != "1" else "2"
        command = [sys.executable, "-B", str(Path(__file__).resolve()), "--out", str(second), "--quiet"] + arguments
        result = subprocess.run(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if result.returncode != 0:
            return ["the second run failed: %s" % result.stdout.decode("utf-8", "replace").strip()]
        first_files = {path.relative_to(out).as_posix() for path in out.rglob("*") if path.is_file()}
        second_files = {path.relative_to(second).as_posix() for path in second.rglob("*") if path.is_file()}
        problems = []
        for name in sorted(first_files ^ second_files):
            problems.append("%s exists in only one of two runs" % name)
        for name in sorted(first_files & second_files):
            if (out / name).read_bytes() != (second / name).read_bytes():
                problems.append("%s differs between two runs" % name)
        return problems


# --------------------------------------------------------------------------------------
# Command line
# --------------------------------------------------------------------------------------


def main(argv=None):
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(
        prog="site/build.py",
        description="Build the bastion-mc website from README.md, docs/ and site/src/.",
    )
    parser.add_argument("--out", metavar="DIR", default=None, help="output directory (default: site/dist)")
    parser.add_argument("--repo-url", metavar="URL", default=None,
                        help="address of the repository, for links to files that are not pages")
    parser.add_argument("--site-url", metavar="URL", default=None,
                        help="address the site is served from, for the sitemap, canonical links and the error page")
    parser.add_argument("--check", action="store_true", help="verify the output (links, policy, allowlist, determinism)")
    parser.add_argument("--keep-going", action="store_true",
                        help="for work in progress: write the pages that can be built although others cannot "
                             "(the exit status still says that the site is incomplete)")
    parser.add_argument("--root", metavar="DIR", default=str(here.parent), help=argparse.SUPPRESS)
    parser.add_argument("--quiet", action="store_true", help=argparse.SUPPRESS)
    options = parser.parse_args(argv)

    root = Path(options.root).resolve()
    out = Path(options.out) if options.out else root / "site" / "dist"
    site = None
    deployable = options.repo_url is not None and options.site_url is not None
    try:
        # An address that is given but empty is a mistake of the caller (an unset variable
        # in a deployment script), never a request for a preview.
        for name, value in (("--repo-url", options.repo_url), ("--site-url", options.site_url)):
            if value is not None and not value.strip():
                raise BuildError("%s is empty (leave the option out for a local preview)" % name)
        repo_url = normalise_repo_url(PREVIEW_REPO_URL if options.repo_url is None else options.repo_url)
        site_url = normalise_site_url(PREVIEW_SITE_URL if options.site_url is None else options.site_url)
        site = Site(root, repo_url, site_url, absolute_error_page=options.site_url is not None)
        files = site.build(keep_going=options.keep_going)
        write_output(files, out)
        problems, facts = ([], None)
        if options.check:
            problems, facts = check_output(out, site, explicit_urls=deployable)
            passed = ["--root", str(root)]
            if options.repo_url is not None:
                passed += ["--repo-url", options.repo_url]
            if options.site_url is not None:
                passed += ["--site-url", options.site_url]
            if not site.errors:  # (an incomplete site is not worth building twice)
                problems += check_deterministic(out, passed)
    except BuildErrors as several:
        for problem in several.problems:
            sys.stderr.write("error: %s\n" % problem)
        sys.stderr.write("error: nothing was written (%d problem(s) in the sources; --keep-going writes the pages "
                         "that can be built)\n" % len(several.problems))
        return 1
    except (md.MarkdownError, BuildError, OSError, UnicodeDecodeError) as problem:
        sys.stderr.write("error: %s\n" % problem)
        return 1

    if not options.quiet:
        for notice in site.notices:
            sys.stderr.write("note: %s\n" % notice)
    for problem in site.errors:
        sys.stderr.write("error: %s\n" % problem)
    for problem in problems:
        sys.stderr.write("check failed: %s\n" % problem)
    if site.errors:
        sys.stderr.write("error: INCOMPLETE site in %s: %d problem(s) in the sources (listed above as 'error:')\n"
                         % (out, len(site.errors)))
    if problems:
        sys.stderr.write("error: %d problem(s) in the generated site\n" % len(problems))
    if site.errors or problems:
        return 1
    if not options.quiet:
        style = sum(len(data) for name, data in files.items() if name.endswith(".css"))
        print("wrote %d files to %s" % (len(files) - 1, out))
        print("landing page: %d bytes of HTML, %d bytes of CSS, no script" % (len(files["index.html"]), style))
        if facts:
            print(
                "check passed: %(pages)d pages, %(internal)d internal links, %(outbound)d outbound links, "
                "two runs byte-identical" % facts
            )
        if not deployable:
            print("preview build: pass --repo-url and --site-url for a deployable one")
    return 0


if __name__ == "__main__":
    sys.exit(main())
