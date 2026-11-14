#!/usr/bin/env python3
"""Strict Markdown to HTML, for the project's own documents and nothing else.

The website renders README.md and docs/*.md. Those are security documents, so this
converter follows one rule: **render it exactly, or refuse**. It implements the part of
CommonMark and GitHub-flavoured Markdown that the documents use:

    ATX headings, paragraphs, block quotes, bullet and numbered lists (nested, tight and
    loose), pipe tables (escaped pipes, also inside code spans), fenced and indented code
    blocks, thematic breaks, code spans, emphasis and strong emphasis, inline links,
    autolinks in angle brackets, bare https addresses, backslash escapes, character
    references, hard and soft line breaks.

"Exactly" means as GitHub renders it: where CommonMark versions differ (which characters
are punctuation beside emphasis, how far a closer looks back for its opener, what a blank
line is), the converter does what cmark-gfm 0.29.0.gfm.13 does, the renderer behind
GitHub's Markdown, and its tests quote GitHub's own answers. Heading ids are GitHub's
anchors, so links written for GitHub work here.

Everything else is an error that names the line: setext headings, raw HTML blocks, link
reference definitions and reference links, images, footnotes, task lists, strikethrough,
link titles, lazy continuation lines, tabs, an unterminated code fence, a line inside a
code block that begins like its fence, a backtick that never closes its code span, link
syntax that does not parse as a link, raw HTML in a heading, a heading without a letter or
digit, nesting deeper than MAX_NESTING. Tables are held to a stricter rule than GitHub's,
because GitHub drops or merges cells silently: a row with the wrong number of cells, a row
outside a table, text directly below a table, and a row or a row of dashes that GitHub
would read as something else stop the conversion.

Nothing from the source reaches the output unescaped, and never two braces in a row (see
`escape`). Raw HTML inside text is shown as text, except an HTML comment, which GitHub does
not show and which is left out here too (see `_is_comment`). A link may point to a fragment, to a
relative path or to an `https://` address with a plain host name; every other target
(`http:`, `javascript:`, `data:`, user names in the address, ports, non-ASCII characters)
is refused. Control and bidirectional-formatting characters, and the line and paragraph
separators U+2028 and U+2029, are refused wherever they appear.

Only the Python standard library is used.
"""

from __future__ import annotations

import bisect
import html
import re
import sys
import unicodedata
from html.entities import html5 as _HTML5_ENTITIES

__all__ = [
    "MAX_NESTING",
    "MarkdownError",
    "Renderer",
    "check_https_url",
    "classify_target",
    "escape",
    "parse",
    "plain_text",
    "render",
    "slugify",
    "split_row",
    "to_html",
]


class MarkdownError(Exception):
    """A construct this converter does not implement, or a target it refuses."""

    def __init__(self, message, line=None, source=None):
        super().__init__(message)
        self.message = message
        self.line = line
        self.source = source

    def __str__(self):
        where = self.source or "<markdown>"
        if self.line is not None:
            where = "%s:%d" % (where, self.line)
        return "%s: %s" % (where, self.message)


# --------------------------------------------------------------------------------------
# The tree
# --------------------------------------------------------------------------------------


class Block:
    """A block-level node. `blank_before` is true when a blank line separated it from the
    previous block of the same container (that decides whether a list is loose)."""

    __slots__ = ("kind", "line", "blank_before", "level", "inlines", "text", "info", "ordered", "start", "tight",
                 "items", "blocks", "header", "aligns", "rows", "row_lines")

    def __init__(self, kind, line, **fields):
        self.kind = kind
        self.line = line
        self.blank_before = False
        for name in self.__slots__[3:]:
            setattr(self, name, fields.pop(name, None))
        if fields:
            raise TypeError("unknown fields: %s" % ", ".join(sorted(fields)))


class Inline:
    """An inline node: text, html (raw HTML, shown as the text it is), comment (an HTML
    comment, which is not shown), code, emph, strong, link, autolink, softbreak, hardbreak.
    `line` is the line of the source it begins on;
    `depth` is how many emphasis and link nodes it holds inside each other."""

    __slots__ = ("kind", "text", "children", "target", "line", "depth")

    def __init__(self, kind, text="", children=None, target=None, line=None):
        self.kind = kind
        self.text = text
        self.children = children
        self.target = target
        self.line = line
        self.depth = 1 + max((child.depth for child in children), default=0) if children is not None else 0
        if self.depth > MAX_NESTING:
            raise MarkdownError("emphasis and links nested more than %d deep are not supported" % MAX_NESTING, line)


class _Delimiter:
    """A run of `*`, `_` or `~` that may still open or close. While emphasis is resolved it
    is also a member of a chain of the delimiters still in play (`before`, `after`), and
    `cell` is its place in the list of nodes."""

    __slots__ = ("char", "count", "original", "can_open", "can_close", "line", "position", "before", "after", "cell")
    kind = "delimiter"

    def __init__(self, char, count, can_open, can_close, line):
        self.char = char
        self.count = count
        self.original = count
        self.can_open = can_open
        self.can_close = can_close
        self.line = line
        self.position = self.before = self.after = self.cell = None


class _Bracket:
    """A `[` that may still turn out to open a link; `index` is its place in the nodes."""

    __slots__ = ("active", "line", "index")
    kind = "bracket"

    def __init__(self, line, index):
        self.active = True
        self.line = line
        self.index = index


# --------------------------------------------------------------------------------------
# Characters
# --------------------------------------------------------------------------------------

_ASCII_PUNCTUATION = frozenset("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~")

#: How deep block quotes and list items, or emphasis and links, may be nested. The documents
#: use three levels at most; beyond this the converter refuses instead of recursing on.
MAX_NESTING = 32

# GitHub renders Markdown with cmark-gfm 0.29.0.gfm.13 (its Markdown API names commonmarker
# 0.23.13, which carries that version). Whether a `*`, `_` or `~` run can open or close
# depends on the characters beside it, read with two tables of that renderer's src/utf8.c.
# Both are copied here, because both differ from Python's newer Unicode database:
# cmark_utf8proc_is_space is the category Zs of an older Unicode version plus tab, line
# feed, form feed and carriage return; cmark_utf8proc_is_punctuation is ASCII punctuation
# plus the non-ASCII code points listed below, the categories P* of an older Unicode version
# (U+166D is a symbol today, and punctuation added since is missing). CommonMark 0.31 also
# counts the symbols S* as punctuation; the CommonMark 0.29 that GitHub follows does not.
_GITHUB_SPACE = frozenset("\t\n\f\r              "
                          "  　")


def _code_point_ranges(text):
    """`"00A1 00B6-00B7 …"` as two sorted lists: the first and the last code point of each range."""
    starts, ends = [], []
    for word in text.split():
        first, _, last = word.partition("-")
        starts.append(int(first, 16))
        ends.append(int(last or first, 16))
    return starts, ends


_GITHUB_PUNCTUATION = _code_point_ranges("""
    00A1 00A7 00AB 00B6-00B7 00BB 00BF 037E 0387 055A-055F 0589-058A 05BE 05C0 05C3 05C6
    05F3-05F4 0609-060A 060C-060D 061B 061E-061F 066A-066D 06D4 0700-070D 07F7-07F9
    0830-083E 085E 0964-0965 0970 0AF0 0DF4 0E4F 0E5A-0E5B 0F04-0F12 0F14 0F3A-0F3D 0F85
    0FD0-0FD4 0FD9-0FDA 104A-104F 10FB 1360-1368 1400 166D-166E 169B-169C 16EB-16ED
    1735-1736 17D4-17D6 17D8-17DA 1800-180A 1944-1945 1A1E-1A1F 1AA0-1AA6 1AA8-1AAD
    1B5A-1B60 1BFC-1BFF 1C3B-1C3F 1C7E-1C7F 1CC0-1CC7 1CD3 2010-2027 2030-2043 2045-2051
    2053-205E 207D-207E 208D-208E 2308-230B 2329-232A 2768-2775 27C5-27C6 27E6-27EF
    2983-2998 29D8-29DB 29FC-29FD 2CF9-2CFC 2CFE-2CFF 2D70 2E00-2E2E 2E30-2E42 3001-3003
    3008-3011 3014-301F 3030 303D 30A0 30FB A4FE-A4FF A60D-A60F A673 A67E A6F2-A6F7
    A874-A877 A8CE-A8CF A8F8-A8FA A92E-A92F A95F A9C1-A9CD A9DE-A9DF AA5C-AA5F AADE-AADF
    AAF0-AAF1 ABEB FD3E-FD3F FE10-FE19 FE30-FE52 FE54-FE61 FE63 FE68 FE6A-FE6B FF01-FF03
    FF05-FF0A FF0C-FF0F FF1A-FF1B FF1F-FF20 FF3B-FF3D FF3F FF5B FF5D FF5F-FF65 10100-10102
    1039F 103D0 1056F 10857 1091F 1093F 10A50-10A58 10A7F 10AF0-10AF6 10B39-10B3F
    10B99-10B9C 11047-1104D 110BB-110BC 110BE-110C1 11140-11143 11174-11175 111C5-111C8
    111CD 11238-1123D 114C6 115C1-115C9 11641-11643 12470-12474 16A6E-16A6F 16AF5
    16B37-16B3B 16B44 1BC9F
""")


def _is_whitespace(char):
    """White space beside a delimiter run, as GitHub's renderer reads it; the beginning and
    the end of the text count as white space."""
    return char == "" or char in _GITHUB_SPACE


def _is_punctuation(char):
    """Punctuation beside a delimiter run, as GitHub's renderer reads it (the table above)."""
    if char == "":
        return False
    if char in _ASCII_PUNCTUATION:
        return True
    code = ord(char)
    if code < 128:
        return False
    starts, ends = _GITHUB_PUNCTUATION
    index = bisect.bisect_right(starts, code) - 1
    return index >= 0 and code <= ends[index]


#: Characters that can make a document read differently from what it says: control, format
#: (bidirectional overrides, zero-width), surrogate and private-use characters.
_INVISIBLE_CATEGORIES = frozenset(("Cc", "Cf", "Cs", "Co"))
#: U+2028 and U+2029 look like a line break, but Markdown does not read them as one.
_SEPARATOR_CATEGORIES = frozenset(("Zl", "Zp"))


def _refused_character(char):
    """What `char` is, if it may not stand in a document (nor as a character reference);
    None if it may."""
    category = unicodedata.category(char)
    if category in _INVISIBLE_CATEGORIES:
        kind = "control or formatting character"
    elif category in _SEPARATOR_CATEGORIES:
        kind = "line or paragraph separator"
    else:
        return None
    return "%s U+%04X (%s)" % (kind, ord(char), unicodedata.name(char, "unnamed"))


def _refuse_control_characters(text, source):
    """None of the characters above is allowed anywhere; a tab is tolerated here and judged
    later (it is refused in indentation and in text)."""
    line = 1
    for char in text:
        if char == "\n":
            line += 1
            continue
        if char == "\t" or (" " <= char <= "~"):
            continue
        problem = _refused_character(char)
        if problem:
            raise MarkdownError("%s is not allowed" % problem, line, source)


# --------------------------------------------------------------------------------------
# Link targets
# --------------------------------------------------------------------------------------

_LABEL = r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?"
_HTTPS_URL = re.compile(r"https://(?P<host>%s(?:\.%s)+)(?P<rest>[/?#][!-~]*)?\Z" % (_LABEL, _LABEL))
# CommonMark percent-encodes these when it writes an href; this converter writes addresses
# exactly as they stand in the source, so an address that contains one is refused.
_URL_FORBIDDEN = frozenset("\"'<>[\\]^`{|}")
_RELATIVE_PATH = re.compile(r"[A-Za-z0-9._/-]+\Z")
_FRAGMENT = re.compile(r"[A-Za-z0-9._~%-]+\Z")
_SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")
_CHARACTER_REFERENCE = re.compile(r"&(?:#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[A-Za-z][A-Za-z0-9]{1,31});")


def check_https_url(url):
    """Return None if `url` is an acceptable outbound address, else the reason it is not.

    Accepted: `https://`, a host made of plain ASCII labels with at least one dot, then
    optionally a path, query or fragment of printable ASCII. Refused on purpose: other
    schemes, a user part (`https://good.example@evil.example/`), a port, a backslash, an
    internationalised host that has not been written out as `xn--…`, quotes, and the
    brackets `< > [ ] { }`.
    """
    if not url.startswith("https://"):
        return "only https:// addresses are allowed"
    match = _HTTPS_URL.match(url)
    if not match:
        return "not a plain https address (host name with a dot, no user part, no port, printable ASCII only)"
    rest = match.group("rest") or ""
    bad = sorted(set(rest) & _URL_FORBIDDEN)
    if bad:
        return "the address contains %s" % " ".join(repr(char) for char in bad)
    if _CHARACTER_REFERENCE.search(rest):
        return "the address contains a character reference (write the character itself)"
    return None


def classify_target(target):
    """Sort a link target into ("fragment", id), ("https", url) or ("relative", path, fragment).

    Raises ValueError with the reason for anything else. This is the whole policy for link
    targets; nothing outside these three shapes can be written to an `href`.
    """
    if target != target.strip() or any(char.isspace() for char in target):
        raise ValueError("link target contains white space")
    if not target:
        raise ValueError("empty link target")
    if target.startswith("#"):
        if not _FRAGMENT.match(target[1:]):
            raise ValueError("unusable fragment %r" % target)
        return ("fragment", target[1:])
    if _SCHEME.match(target):
        problem = check_https_url(target)
        if problem:
            raise ValueError("refused link target %r: %s" % (target, problem))
        return ("https", target)
    if target.startswith(("/", "\\")) or "\\" in target:
        raise ValueError("refused link target %r: only relative paths, fragments and https addresses" % target)
    path, _, fragment = target.partition("#")
    if not _RELATIVE_PATH.match(path) or (fragment and not _FRAGMENT.match(fragment)) or ("#" in target and not fragment):
        raise ValueError("refused link target %r: not a plain relative path" % target)
    return ("relative", path, fragment)


# --------------------------------------------------------------------------------------
# Block structure
# --------------------------------------------------------------------------------------

_FENCE = re.compile(r"^(`{3,}|~{3,})(.*)$")
_ATX = re.compile(r"^(#{1,6})(?:[ ]+(.*))?$")
_THEMATIC = re.compile(r"^(?:(?:\*[ ]*){3,}|(?:-[ ]*){3,}|(?:_[ ]*){3,})$")
_SETEXT = re.compile(r"^(?:=+|-+)[ ]*$")
_BULLET = re.compile(r"^([-*+])(?:([ ]+)(.*))?$")
_ORDERED = re.compile(r"^([0-9]{1,9})([.)])(?:([ ]+)(.*))?$")
_DELIMITER_CELL = re.compile(r"^:?-+:?$")
# A link label followed by a colon, where a paragraph would begin. The label may contain
# escaped brackets and may run over several lines.
_REFERENCE_DEFINITION = re.compile(r"^\[(?:[^\[\]\\]|\\.)*\]:", re.S)
_TASK_ITEM = re.compile(r"^\[[ xX]\](?:[ ]|$)")
_INFO_WORD = re.compile(r"^[A-Za-z0-9_+.-]+$")
# Code blocks that GitHub does not show as code: it draws a diagram, a formula, a map or a
# 3D model from them.
_RENDERED_LANGUAGES = frozenset(("mermaid", "math", "geojson", "topojson", "stl"))
_ALERT = re.compile(r"^[ ]*\[![A-Za-z]+\]")

_HTML_BLOCK_NAMES = (
    "address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|"
    "fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|"
    "menuitem|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|"
    "track|ul"
)
# CommonMark's start conditions 1 to 6: these also interrupt a paragraph.
_HTML_BLOCK = re.compile(
    r"^(?:<(?:pre|script|style|textarea)(?:[ \t>]|$)|<!--|<\?|<![A-Za-z]|<!\[CDATA\[|</?(?:%s)(?:[ \t]|/?>|$))" % _HTML_BLOCK_NAMES,
    re.IGNORECASE,
)
# Start condition 7: a line that is one complete tag.
_HTML_TAG_LINE = re.compile(
    r"^(?:<[A-Za-z][A-Za-z0-9-]*(?:[ \t]+[A-Za-z_:][A-Za-z0-9_.:-]*(?:[ \t]*=[ \t]*(?:[^ \t\"'=<>`]+|'[^']*'|\"[^\"]*\"))?)*[ \t]*/?>"
    r"|</[A-Za-z][A-Za-z0-9-]*[ \t]*>)[ \t]*$"
)


def _indent_of(text, line):
    stripped = text.lstrip(" ")
    if stripped.startswith("\t"):
        raise MarkdownError("tab in indentation (use spaces)", line)
    return len(text) - len(stripped)


class _ListMarker:
    __slots__ = ("indent", "ordered", "sign", "start", "offset", "rest")


def _list_marker(text, line):
    """Describe the list marker a line starts with, or return None."""
    indent = _indent_of(text, line)
    if indent > 3:
        return None
    body = text[indent:]
    bullet = _BULLET.match(body)
    ordered = None if bullet else _ORDERED.match(body)
    if not bullet and not ordered:
        return None
    marker = _ListMarker()
    marker.indent = indent
    if bullet:
        marker.ordered, marker.sign, marker.start = False, bullet.group(1), None
        width, spaces, rest = 1, bullet.group(2), bullet.group(3)
    else:
        marker.ordered, marker.sign, marker.start = True, ordered.group(2), int(ordered.group(1))
        width, spaces, rest = len(ordered.group(1)) + 1, ordered.group(3), ordered.group(4)
    if spaces is None or rest is None or rest.strip() == "":
        raise MarkdownError("empty list item", line)
    if len(spaces) > 4:
        raise MarkdownError("a list item that begins with an indented code block is not supported", line)
    marker.offset = indent + width + len(spaces)
    marker.rest = rest
    return marker


def split_row(text, keep_escapes=False):
    """Split a table row into its cells as GitHub's renderer does (cmark-gfm's table
    extension): a pipe directly after a backslash is part of the cell, whatever stands before
    that backslash (`x\\\\|y` is one cell), and one backslash is taken from it, also inside a
    code span; nothing else is unescaped here. Cells lose the spaces and tabs around them,
    and no other white space. With `keep_escapes`, `\\|` stays as it is written (for a caller
    that writes the row out again)."""
    body = text.strip(" \t")
    ended = body.startswith("|")  # (whether the row so far ends in a pipe that is not escaped)
    if ended:
        body = body[1:]
    cells, current, index = [], [], 0
    while index < len(body):
        char = body[index]
        if char == "\\" and body.startswith("|", index + 1):
            current.append("\\|" if keep_escapes else "|")
            index += 2
            ended = False
            continue
        if char == "|":
            cells.append("".join(current).strip(" \t"))
            current = []
            ended = True
        else:
            current.append(char)
            ended = ended and char in " \t"
        index += 1
    last = "".join(current).strip(" \t")
    if last or not ended:
        cells.append(last)
    return cells


def _is_delimiter_row(text):
    if "|" not in text or "-" not in text:
        return False
    cells = split_row(text)
    return bool(cells) and all(_DELIMITER_CELL.match(cell) for cell in cells)


def _is_blank(text):
    """A blank line holds nothing but spaces and tabs (a line of other white space, such as
    U+00A0, is not blank for Markdown, nor for GitHub)."""
    return text.strip(" \t") == ""


def _ends_in_thematic_break(blocks):
    """Do these blocks end in a thematic break, also as the last block of the last item of a
    list at their end? GitHub's renderer keeps such a break open until the next line, so a
    blank line after it is the break's own: it separates nothing, and a list stays tight."""
    while blocks:
        last = blocks[-1]
        if last.kind == "thematic":
            return True
        if last.kind != "list":
            return False
        blocks = last.items[-1]
    return False


def _check_delimiter_row(number, text):
    """A row of dashes below a line with a pipe starts a table only where GitHub's renderer
    lets nothing else claim the line first; elsewhere the author's table would not be one."""
    indent = _indent_of(text, number)
    if indent >= 4:
        raise MarkdownError("a row of dashes indented by four or more spaces: GitHub does not read it as part of a "
                            "table (indent it by at most three spaces)", number)
    if re.match(r"-[ \t]", text[indent:]):
        raise MarkdownError("a row of dashes that begins with '- ': GitHub reads it as a list item, not as part of a "
                            "table (write it as '|---|---|' or '--- | ---')", number)


class _BlockParser:
    def __init__(self):
        self.depth = 0  # how many block quotes and list items enclose what is parsed now

    # -- helpers ------------------------------------------------------------------------

    _blank = staticmethod(_is_blank)

    def _nested(self, lines, line):
        """Parse the lines of a block quote or a list item, one level deeper."""
        if self.depth >= MAX_NESTING:
            raise MarkdownError("block quotes and list items nested more than %d deep are not supported" % MAX_NESTING,
                                line)
        self.depth += 1
        try:
            return self.parse(lines)
        finally:
            self.depth -= 1

    def _starts_block(self, lines, index):
        """Does lines[index] begin something that ends a paragraph before it?

        Returns a short name of the construct, or None. Constructs this converter refuses
        raise instead."""
        number, text = lines[index]
        indent = _indent_of(text, number)
        if indent > 3:
            return None
        body = text[indent:]
        if _FENCE.match(body):
            return "fence"
        heading = _ATX.match(body)
        if heading:
            return "heading"
        if _THEMATIC.match(body.rstrip(" ")):
            return "thematic break"
        if body.startswith(">"):
            return "block quote"
        if _HTML_BLOCK.match(body):
            raise MarkdownError("raw HTML block is not supported (HTML is never passed through)", number)
        marker = _list_marker(text, number)
        if marker and (not marker.ordered or marker.start == 1):
            return "list"
        return None

    # -- the loop -----------------------------------------------------------------------

    def parse(self, lines):
        blocks = []
        index, count = 0, len(lines)
        blank_seen = False

        def add(block):
            block.blank_before = blank_seen and bool(blocks) and not _ends_in_thematic_break(blocks)
            blocks.append(block)

        while index < count:
            number, text = lines[index]
            if self._blank(text):
                blank_seen = True
                index += 1
                continue
            indent = _indent_of(text, number)

            if indent >= 4:
                index = self._indented_code(lines, index, add)
                blank_seen = False
                continue
            body = text[indent:]

            fence = _FENCE.match(body)
            if fence:
                index = self._fenced_code(lines, index, indent, fence, add)
                blank_seen = False
                continue

            heading = _ATX.match(body)
            if heading:
                # (only spaces: other white space, such as U+00A0, is part of the heading)
                content = (heading.group(2) or "").strip(" ")
                content = re.sub(r"(?:^|[ ]+)#+$", "", content).strip(" ")
                if not content:
                    raise MarkdownError("empty heading", number)
                add(Block("heading", number, level=len(heading.group(1)), inlines=_parse_inlines(content, number)))
                index += 1
                blank_seen = False
                continue

            if _THEMATIC.match(body.rstrip(" ")):
                add(Block("thematic", number))
                index += 1
                blank_seen = False
                continue

            if body.startswith(">"):
                index = self._block_quote(lines, index, add)
                blank_seen = False
                continue

            if _HTML_BLOCK.match(body) or _HTML_TAG_LINE.match(body):
                raise MarkdownError("raw HTML block is not supported (HTML is never passed through)", number)

            if _list_marker(text, number):
                index = self._list(lines, index, add)
                blank_seen = False
                continue

            if "|" in body and index + 1 < count and _is_delimiter_row(lines[index + 1][1]):
                _check_delimiter_row(*lines[index + 1])
                index = self._table(lines, index, add)
                blank_seen = False
                continue
            if _is_delimiter_row(body):
                raise MarkdownError("table delimiter row without a header row above it", number)
            if body.startswith("|"):
                raise MarkdownError("table row outside a table (is there a blank line inside the table?)", number)

            index = self._paragraph(lines, index, add)
            blank_seen = False
        return blocks

    # -- leaf blocks --------------------------------------------------------------------

    def _indented_code(self, lines, index, add):
        first = lines[index][0]
        collected = []
        while index < len(lines):
            number, text = lines[index]
            if self._blank(text):
                collected.append(text[4:])
            elif _indent_of(text, number) >= 4:
                collected.append(text[4:])
            else:
                break
            index += 1
        trailing = 0
        while collected and _is_blank(collected[-1]):
            collected.pop()
            trailing += 1
        add(Block("code", first, text="\n".join(collected) + "\n", info=""))
        return index - trailing

    def _fenced_code(self, lines, index, indent, fence, add):
        first = lines[index][0]
        marker, info = fence.group(1), fence.group(2).strip(" \t")
        if marker[0] == "`" and "`" in info:
            # CommonMark reads this as a code span that starts a paragraph.
            raise MarkdownError("backtick in the info string of a code fence", first)
        word = info.split()[0] if info else ""
        if info and (len(info.split()) != 1 or not _INFO_WORD.match(word)):
            raise MarkdownError("unsupported info string %r on a code fence (one word of letters and digits)" % info, first)
        if word.lower() in _RENDERED_LANGUAGES:
            raise MarkdownError("GitHub renders a `%s` block as a picture, not as code; that is not supported" % word, first)
        # A closing fence is a run of the same character at least as long, followed by
        # nothing but spaces or tabs (CommonMark; GitHub does the same).
        run = r"^ {0,3}%s{%d,}" % (re.escape(marker[0]), len(marker))
        closing = re.compile(run + r"[ \t]*$")
        lookalike = re.compile(run)
        collected = []
        index += 1
        while index < len(lines):
            number, text = lines[index]
            if closing.match(text):
                add(Block("code", first, text="".join(line + "\n" for line in collected), info=word))
                return index + 1
            if lookalike.match(text):
                # Such a line is code for GitHub too, but no author means it: it is a fence
                # that lost its closing line, or a closing fence with something after it
                # (U+00A0, a word). Either way the fences after it pair up wrongly, and whole
                # sections would turn into code without a word.
                raise MarkdownError(
                    "this line begins like a code fence but does not close the block opened at line %d (only spaces "
                    "or tabs may follow a closing fence); the fences after it would pair up differently from what "
                    "was meant (to show a fence inside code, open the block with a longer one)" % first,
                    number,
                )
            leading = len(text) - len(text.lstrip(" "))
            collected.append(text[min(indent, leading) :])
            index += 1
        raise MarkdownError("code fence opened here is never closed", first)

    def _paragraph(self, lines, index, add):
        first = lines[index][0]
        collected = [lines[index][1].lstrip(" ")]
        index += 1
        while index < len(lines):
            number, text = lines[index]
            if self._blank(text):
                break
            body = text.strip(" ")
            if _SETEXT.match(body):
                raise MarkdownError("setext heading underline (use '#' headings; put a blank line before a '---' rule)", number)
            if self._starts_block(lines, index):
                break
            if _is_delimiter_row(body):
                raise MarkdownError("a table must be preceded by a blank line", number)
            if ":" in body and _DELIMITER_CELL.match(body) and len(text) - len(text.lstrip(" ")) <= 3:
                # (`:--`, `--:`, `:-:` alone: a table of one column for GitHub, with the line
                # above it as its header)
                raise MarkdownError("a row of dashes with a colon directly below a line of text: GitHub reads the two "
                                    "lines as a table of one column (put a blank line between them)", number)
            if body.startswith("|"):
                raise MarkdownError("table row outside a table (is there a blank line inside the table?)", number)
            collected.append(text.lstrip(" "))
            index += 1
        collected[-1] = collected[-1].rstrip(" ")
        text = "\n".join(collected)
        if _REFERENCE_DEFINITION.match(text):
            raise MarkdownError("link reference definitions and footnotes are not supported", first)
        add(Block("paragraph", first, inlines=_parse_inlines(text, first)))
        return index

    # -- containers ---------------------------------------------------------------------

    def _block_quote(self, lines, index, add):
        first = lines[index][0]
        inner = []
        while index < len(lines):
            number, text = lines[index]
            indent = _indent_of(text, number) if not self._blank(text) else 0
            body = text[indent:]
            if indent <= 3 and body.startswith(">"):
                body = body[1:]
                inner.append((number, body[1:] if body.startswith(" ") else body))
                index += 1
                continue
            break
        if _ALERT.match(inner[0][1]):
            raise MarkdownError("GitHub alerts (a block quote that begins with [!NOTE] and the like) are not supported",
                                first)
        blocks = self._nested(inner, first)
        if index < len(lines) and not self._blank(lines[index][1]) and _ends_in_open_paragraph(blocks, inner):
            if not self._starts_block(lines, index):
                raise MarkdownError("lazy continuation line after a block quote (start it with '>' or put a blank line before it)",
                                    lines[index][0])
        add(Block("quote", first, blocks=blocks))
        return index

    def _list(self, lines, index, add):
        first_number = lines[index][0]
        first = _list_marker(lines[index][1], first_number)
        items, loose = [], False
        count = len(lines)
        while True:
            number, text = lines[index]
            marker = _list_marker(text, number)
            if _TASK_ITEM.match(marker.rest):
                raise MarkdownError("task list items are not supported", number)
            item_lines = [(number, marker.rest)]
            index += 1
            pending = []
            while index < count:
                number, text = lines[index]
                if self._blank(text):
                    pending.append((number, text[marker.offset :]))
                    index += 1
                    continue
                if _indent_of(text, number) >= marker.offset:
                    item_lines.extend(pending)
                    pending = []
                    item_lines.append((number, text[marker.offset :]))
                    index += 1
                    continue
                break
            blocks = self._nested(item_lines, item_lines[0][0])
            if any(block.blank_before for block in blocks):
                loose = True
            items.append(blocks)

            if index >= count:
                break
            number, text = lines[index]
            indent = _indent_of(text, number)
            if indent <= 3 and _THEMATIC.match(text[indent:].rstrip(" ")):
                # A thematic break takes precedence over a list item (`- - -`, `* * *`,
                # `-  --`): it ends the list, as in CommonMark and on GitHub.
                index -= len(pending)
                break
            following = _list_marker(text, number)
            same_kind = following is not None and following.ordered == first.ordered and following.sign == first.sign
            if same_kind and following.indent == first.indent:
                if pending and not _ends_in_thematic_break(blocks):
                    loose = True
                continue
            if same_kind:
                # CommonMark would make it one more item of this list (a loose one after a
                # blank line); written like that it is more likely a mistake than intended.
                raise MarkdownError("list marker indented differently from the other items of its list", number)
            if not pending and _ends_in_open_paragraph(blocks, item_lines) and not self._starts_block(lines, index):
                raise MarkdownError(
                    "lazy continuation line: indent it to the list item's text, or put a blank line before it", number
                )
            # The blank lines after the last item belong to whatever contains the list.
            index -= len(pending)
            break
        add(Block("list", first_number, ordered=first.ordered, start=first.start, tight=not loose, items=items))
        return index

    def _table(self, lines, index, add):
        first = lines[index][0]
        header = split_row(lines[index][1])
        delimiters = split_row(lines[index + 1][1])
        if len(header) != len(delimiters):
            raise MarkdownError(
                "table header has %d cells but the row of dashes below it has %d" % (len(header), len(delimiters)), first
            )
        aligns = []
        for cell in delimiters:
            left, right = cell.startswith(":"), cell.endswith(":")
            aligns.append("center" if left and right else "right" if right else "left" if left else None)
        rows = []
        index += 2
        while index < len(lines):
            number, text = lines[index]
            if self._blank(text) or self._starts_block(lines, index):
                break
            if _indent_of(text, number) >= 4:
                raise MarkdownError("a line indented by four or more spaces directly below a table: GitHub ends the "
                                    "table there and shows the line as code (indent a row by at most three spaces, or "
                                    "put a blank line between the table and the code)", number)
            if _list_marker(text, number):
                # GitHub ends a table at a list item of any number, not only at one that
                # could interrupt a paragraph (`2. x | y` starts a list there).
                break
            if "|" not in text:
                raise MarkdownError("text directly below a table (put a blank line after the table)", number)
            cells = split_row(text)
            if len(cells) != len(header):
                raise MarkdownError(
                    "table row has %d cells, its header has %d (an unescaped '|' in a cell?)" % (len(cells), len(header)),
                    number,
                )
            rows.append((number, [_parse_inlines(cell, number) for cell in cells]))
            index += 1
        add(Block("table", first, header=[_parse_inlines(cell, first) for cell in header], aligns=aligns,
                  rows=[cells for _, cells in rows], row_lines=[number for number, _ in rows]))
        return index


def _ends_in_open_paragraph(blocks, lines):
    """Is the last thing in this container a paragraph that reaches its last line?"""
    if not blocks or not lines or _is_blank(lines[-1][1]):
        return False
    last = blocks[-1]
    while True:
        if last.kind == "paragraph":
            return True
        if last.kind == "list" and last.items and last.items[-1]:
            last = last.items[-1][-1]
        elif last.kind == "quote" and last.blocks:
            last = last.blocks[-1]
        else:
            return False


# --------------------------------------------------------------------------------------
# Inline structure
# --------------------------------------------------------------------------------------

_ANGLE_AUTOLINK = re.compile(r"<([A-Za-z][A-Za-z0-9+.-]{1,31}:[^\s<>]*)>")
# What CommonMark takes for raw HTML inside a paragraph: an opening or closing tag, a
# comment, a processing instruction, a declaration or a CDATA section. "Space" inside a
# tag is spaces with at most one line ending.
_HTML_SPACE = r"(?:[ ]*\n[ ]*|[ ]+)"
_HTML_OPTIONAL_SPACE = r"(?:[ ]*\n[ ]*|[ ]*)"
_HTML_ATTRIBUTE = (
    _HTML_SPACE + r"[A-Za-z_:][A-Za-z0-9_.:-]*"
    r"(?:" + _HTML_OPTIONAL_SPACE + "=" + _HTML_OPTIONAL_SPACE + r"(?:[^ \t\n\"'=<>`]+|'[^']*'|\"[^\"]*\"))?"
)
_INLINE_HTML = re.compile(
    r"<[A-Za-z][A-Za-z0-9-]*(?:" + _HTML_ATTRIBUTE + ")*" + _HTML_OPTIONAL_SPACE + r"/?>"
    r"|</[A-Za-z][A-Za-z0-9-]*" + _HTML_OPTIONAL_SPACE + r">"
    r"|<!-->|<!--->|<!--.*?-->"
    r"|<\?.*?\?>"
    r"|<![A-Za-z][^>]*>"
    r"|<!\[CDATA\[.*?\]\]>",
    re.S,
)
_ANGLE_EMAIL = re.compile(r"<[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?>")
# Wider than what GitHub turns into a mailto: link, so that nothing it would link gets by.
# (It may begin only where a run of such characters begins: the same matches, but a long
# run without an `@` is read once, not once from each of its characters.)
_BARE_EMAIL = re.compile(r"(?<![A-Za-z0-9.+_-])[A-Za-z0-9.+_-]+@[A-Za-z0-9._-]*\.[A-Za-z0-9]+")
# GitHub's extended autolinks, as cmark-gfm implements them: the three schemes in any case,
# and `www.`. The address runs to the next ASCII white space or `<`.
_BARE_URL_START = re.compile(r"(?:[Hh][Tt][Tt][Pp][Ss]?://|[Ff][Tt][Pp]://|www\.)")
_BARE_URL_BODY = re.compile(r"[^\x09-\x0d <]*")
_TRAILING_ENTITY = re.compile(r"&[A-Za-z]+;$")


def _parse_inlines(text, line):
    return _InlineParser(text, line).parse()


def _is_comment(raw):
    """Whether raw HTML that the inline parser matched is an HTML comment by the rule of
    GitHub's renderer (cmark-gfm 0.29, CommonMark 0.29: `<!--`, a text, `-->`, where the text
    does not start with `>` or `->`, does not end with `-` and does not contain `--`; and
    the empty `<!---->`). GitHub shows no comment: its renderer passes it on as raw HTML and
    its sanitiser removes it. A comment is therefore left out of the page and of the text a
    reader sees (`plain_text`). Anything else that begins with `<!--` is shown as text, as
    before: by that rule it is no comment, and newer CommonMark versions disagree."""
    if not (raw.startswith("<!--") and raw.endswith("-->")):
        return False
    if raw == "<!---->":
        return True
    inner = raw[4:-3]
    return bool(inner) and not inner.startswith((">", "->")) and not inner.endswith("-") and "--" not in inner


class _InlineParser:
    def __init__(self, text, line):
        self.text = text
        self.first_line = line
        self._breaks = [match.start() for match in re.finditer("\n", text)]
        self._found = {}  # closing mark -> where a search for it last found it (-1: nowhere)

    def _line(self, position):
        """The line of the source that the character at `position` stands on."""
        return self.first_line + bisect.bisect_left(self._breaks, position)

    def _error(self, message, position):
        raise MarkdownError(message, self._line(position))

    def _next(self, mark, position):
        """Where `mark` next occurs at or after `position`, or -1. A search goes on from the
        last answer, so that many openers without a closing mark cost one pass in all."""
        found = self._found.get(mark)
        if found is None or (found != -1 and found < position):
            found = self.text.find(mark, position)
            self._found[mark] = found
        return found

    def _raw_html(self, index):
        """Raw HTML that begins at `index`, as CommonMark reads it inside a paragraph, or
        None. The forms that run to a closing mark are tried only where that mark comes
        later (as cmark does): otherwise each opener would read the rest of the text."""
        text = self.text
        if text.startswith("<!--", index) and not text.startswith(("<!-->", "<!--->"), index):
            closing = self._next("-->", index + 4)
        elif text.startswith("<![CDATA[", index):
            closing = self._next("]]>", index + 9)
        elif text.startswith("<!", index):
            closing = self._next(">", index + 2)
        elif text.startswith("<?", index):
            closing = self._next("?>", index + 2)
        else:
            closing = 0
        return None if closing == -1 else _INLINE_HTML.match(text, index)

    def _flank_before(self, position):
        """The character GitHub's renderer reads before a `*` or `_` run that begins at
        `position`. It steps over `~`, the character of its strikethrough extension; a `~`
        at the very beginning, like the beginning itself, counts as white space ("")."""
        if position == 0:
            return ""
        before = position - 1
        while before > 0 and self.text[before] == "~":
            before -= 1
        return "" if self.text[before] == "~" else self.text[before]

    def _flank_after(self, position):
        """The same after a run that ends just before `position`."""
        text, length = self.text, len(self.text)
        while position < length and text[position] == "~":
            position += 1
        return text[position] if position < length else ""

    def parse(self):
        text, length = self.text, len(self.text)
        nodes, brackets, pending = [], [], []
        pending_at = None  # where the text that is pending begins

        def keep(piece, position):
            nonlocal pending_at
            if not pending:
                pending_at = position
            pending.append(piece)

        def flush():
            if pending:
                nodes.append(Inline("text", "".join(pending), line=self._line(pending_at)))
                del pending[:]

        index = 0
        while index < length:
            char = text[index]

            if char == "\t":
                self._error("tab character in text (use spaces)", index)

            if char == "\\":
                following = text[index + 1] if index + 1 < length else ""
                if following == "\n":
                    flush()
                    nodes.append(Inline("hardbreak"))
                    index += 2
                    while index < length and text[index] == " ":
                        index += 1
                elif following in _ASCII_PUNCTUATION and following != "":
                    keep(following, index)
                    index += 2
                else:
                    keep("\\", index)
                    index += 1
                continue

            if char == "`":
                end = index
                while end < length and text[end] == "`":
                    end += 1
                run = end - index
                search = end
                while True:
                    start = text.find("`", search)
                    if start == -1:
                        self._error("backtick that never closes its code span", index)
                    stop = start
                    while stop < length and text[stop] == "`":
                        stop += 1
                    if stop - start == run:
                        break
                    search = stop
                content = text[end:start].replace("\n", " ")
                if len(content) >= 2 and content[0] == " " and content[-1] == " " and content.strip(" "):
                    content = content[1:-1]
                flush()
                nodes.append(Inline("code", content, line=self._line(index)))
                index = stop
                continue

            if char == "<":
                match = _ANGLE_AUTOLINK.match(text, index)
                if match:
                    url = match.group(1)
                    problem = check_https_url(url)
                    if problem:
                        self._error("refused autolink <%s>: %s" % (url, problem), index)
                    flush()
                    nodes.append(Inline("autolink", url, line=self._line(index)))
                    index = match.end()
                    continue
                if _ANGLE_EMAIL.match(text, index):
                    self._error("e-mail autolinks are not supported (mailto: is not an allowed target)", index)
                match = self._raw_html(index)
                if match:
                    # Raw HTML. CommonMark would pass it through untouched; here it is shown
                    # as the text it is, equally untouched: nothing inside it is markup. An
                    # HTML comment is the one exception: GitHub shows none, so none is shown.
                    if "\t" in match.group(0):
                        self._error("tab character in text (use spaces)", index)
                    flush()
                    kind = "comment" if _is_comment(match.group(0)) else "html"
                    nodes.append(Inline(kind, match.group(0), line=self._line(index)))
                    index = match.end()
                    continue
                keep("<", index)
                index += 1
                continue

            if char == "&":
                match = _CHARACTER_REFERENCE.match(text, index)
                if match:
                    decoded = _decode_reference(match.group(0))
                    if decoded is not None:
                        for character in decoded:
                            problem = _refused_character(character)
                            if problem:
                                self._error("character reference %s names a %s" % (match.group(0), problem), index)
                        # A node of its own, so that a space it stands for is never one of
                        # the two spaces before a line end that make a hard line break:
                        # CommonMark and GitHub count only spaces written as spaces.
                        flush()
                        nodes.append(Inline("text", decoded, line=self._line(index)))
                        index = match.end()
                        continue
                keep("&", index)
                index += 1
                continue

            if char == "!" and text.startswith("![", index):
                self._error("images are not supported", index)

            if char == "[":
                if text.startswith("[^", index):
                    self._error("footnotes are not supported", index)
                flush()
                bracket = _Bracket(self._line(index), len(nodes))
                nodes.append(bracket)
                brackets.append(bracket)
                index += 1
                continue

            if char == "]":
                following = text[index + 1] if index + 1 < length else ""
                if not brackets:
                    if following == "(":
                        self._error("'](' without an opening '[' (escape the bracket if it is meant literally)", index)
                    keep("]", index)
                    index += 1
                    continue
                bracket = brackets.pop()
                if following == "[":
                    self._error("reference-style links are not supported", index)
                if following == "(":
                    if not bracket.active:
                        self._error("a link inside the text of another link", index)
                    target, after = self._link_target(index + 1)
                    flush()
                    children = _resolve_emphasis(nodes[bracket.index + 1 :])
                    del nodes[bracket.index :]
                    if _contains_link(children):
                        self._error("a link inside the text of another link", index)
                    if not plain_text(children).strip():
                        self._error("link without text", index)
                    nodes.append(Inline("link", children=children, target=target, line=bracket.line))
                    for other in brackets:
                        other.active = False
                    index = after
                    continue
                flush()
                nodes[bracket.index] = Inline("text", "[", line=bracket.line)
                keep("]", index)
                index += 1
                continue

            if char in "*_":
                end = index
                while end < length and text[end] == char:
                    end += 1
                before, after = self._flank_before(index), self._flank_after(end)
                left = not _is_whitespace(after) and (
                    not _is_punctuation(after) or _is_whitespace(before) or _is_punctuation(before)
                )
                right = not _is_whitespace(before) and (
                    not _is_punctuation(before) or _is_whitespace(after) or _is_punctuation(after)
                )
                if char == "*":
                    can_open, can_close = left, right
                else:
                    can_open = left and (not right or _is_punctuation(before))
                    can_close = right and (not left or _is_punctuation(after))
                if can_open or can_close:
                    flush()
                    nodes.append(_Delimiter(char, end - index, can_open, can_close, self._line(index)))
                else:
                    keep(text[index:end], index)  # (a run that can do neither is text, as in cmark)
                index = end
                continue

            if char == "~":
                # GitHub's strikethrough extension reads at most 100 tildes at a time, looks
                # only at the characters directly beside them, and makes a delimiter of a
                # run of one or two. Where it would draw strikethrough, the converter refuses
                # (in _resolve_emphasis); elsewhere the tildes are text.
                end = index
                while end < length and text[end] == "~":
                    end += 1
                position = index
                while position < end:
                    stop = min(end, position + 100)
                    before = text[position - 1] if position > 0 else ""
                    after = text[stop] if stop < length else ""
                    left = not _is_whitespace(after) and (
                        not _is_punctuation(after) or _is_whitespace(before) or _is_punctuation(before)
                    )
                    right = not _is_whitespace(before) and (
                        not _is_punctuation(before) or _is_whitespace(after) or _is_punctuation(after)
                    )
                    if (left or right) and stop - position <= 2:
                        flush()
                        nodes.append(_Delimiter("~", stop - position, left, right, self._line(position)))
                    else:
                        keep(text[position:stop], position)
                    position = stop
                index = end
                continue

            if char == "\n":
                spaces = 0
                while pending and pending[-1] == " ":
                    pending.pop()
                    spaces += 1
                flush()
                nodes.append(Inline("hardbreak" if spaces >= 2 else "softbreak"))
                index += 1
                while index < length and text[index] == " ":
                    index += 1
                continue

            if char in "hHfFw" and _BARE_URL_START.match(text, index):
                before = text[index - 1] if index > 0 else ""
                # Where GitHub's specification allows an autolink to begin.
                boundary = before == "" or before in " \n*_~("
                if char == "w":
                    if boundary:
                        self._error("bare www. address (GitHub would link it as http://; write the https:// address, "
                                    "or put it in a code span)", index)
                    # Anywhere else it is ordinary text, for GitHub as well.
                elif not (before.isascii() and before.isalpha()):
                    # (GitHub reads letters directly before the scheme as part of it, and
                    # then links nothing: `xhttps://…` is text.)
                    if brackets:
                        self._error("address inside square brackets (GitHub does not link it there; move it out of "
                                    "the brackets, or put it in a code span)", index)
                    if not boundary:
                        self._error("address directly after %r (GitHub links it there although its specification does "
                                    "not; put a space before it, or put it in a code span)" % before, index)
                    url, after = self._bare_url(index)
                    flush()
                    nodes.append(Inline("autolink", url, line=self._line(index)))
                    index = after
                    continue

            keep(char, index)
            index += 1

        flush()
        for bracket in brackets:
            nodes[bracket.index] = Inline("text", "[", line=bracket.line)
        nodes = _merge_text(_resolve_emphasis(nodes))
        self._refuse_bare_email(nodes)
        self._refuse_github_text_features(nodes)
        return nodes

    def _link_target(self, position):
        """Parse `(target)` starting at the parenthesis; return (target, index after it)."""
        text, length = self.text, len(self.text)
        index = position + 1
        while index < length and text[index] == " ":
            index += 1
        if index < length and text[index] == "<":
            self._error("link targets in angle brackets are not supported", position)
        depth, collected = 0, []
        while index < length:
            char = text[index]
            if char == "\\" and index + 1 < length and text[index + 1] in _ASCII_PUNCTUATION:
                collected.append(text[index + 1])
                index += 2
                continue
            if char == "(":
                depth += 1
            elif char == ")":
                if depth == 0:
                    break
                depth -= 1
            elif char in " \n":
                break
            collected.append(char)
            index += 1
        target = "".join(collected)
        while index < length and text[index] == " ":
            index += 1
        if index < length and text[index] in "\"'(":
            self._error("link titles are not supported", position)
        if index >= length or text[index] != ")":
            self._error("malformed link: expected ')' after the target", position)
        try:
            classify_target(target)
        except ValueError as problem:
            self._error(str(problem), position)
        return target, index + 1

    def _bare_url(self, position):
        """A bare address in running text (GitHub's extended autolink): return (url, end)."""
        text = self.text
        if not text.startswith("https://", position):
            self._error("bare address that does not begin with a lower-case https:// (GitHub would link it; only "
                        "https:// addresses are allowed, or put it in a code span)", position)
        end = _BARE_URL_BODY.match(text, position).end()
        candidate = text[position:end]
        while candidate:
            last = candidate[-1]
            if last in "?!.,:*_~":
                candidate = candidate[:-1]
            elif last == ")" and candidate.count(")") > candidate.count("("):
                candidate = candidate[:-1]
            elif last == ";":
                entity = _TRAILING_ENTITY.search(candidate)
                candidate = candidate[: entity.start()] if entity else candidate[:-1]
            else:
                break
        problem = check_https_url(candidate)
        if problem:
            self._error("refused address %r: %s (put it in a code span if it is not meant as a link)" % (candidate, problem),
                        position)
        return candidate, position + len(candidate)

    def _refuse_bare_email(self, nodes):
        for node in nodes:
            if node.kind in ("text", "html"):
                match = _BARE_EMAIL.search(node.text)
                if match:
                    raise MarkdownError(
                        "bare e-mail address %r (GitHub would turn it into a mailto: link; put it in a code span)"
                        % match.group(0),
                        node.line + node.text.count("\n", 0, match.start()),
                    )
            if node.children:
                self._refuse_bare_email(node.children)

    def _refuse_github_text_features(self, nodes):
        """Two things GitHub does to plain text outside its Markdown specification. The text
        of the nodes is read as one (an emoji short code may run over two of them); each
        error names the line where what it found begins."""
        pieces = list(_text_nodes(nodes))
        starts, offset = [], 0
        for node in pieces:
            starts.append(offset)
            offset += len(node.text)
        text = "".join(node.text for node in pieces)

        def line_at(position):
            index = bisect.bisect_right(starts, position) - 1
            return pieces[index].line + pieces[index].text.count("\n", 0, position - starts[index])

        emoji = _EMOJI_SHORTCODE.search(text)
        if emoji:
            raise MarkdownError(
                "%r looks like an emoji short code (GitHub may replace it with a picture; put it in a code span)"
                % emoji.group(0),
                line_at(emoji.start()),
            )
        first = text.find("$")
        second = text.find("$", first + 1) if first != -1 else -1
        if second != -1:
            message = ("two dollar signs in one piece of text (GitHub may typeset what is between them as a formula; "
                       "put them in code spans)")
            if line_at(second) != line_at(first):
                message += "; the second is on line %d" % line_at(second)
            raise MarkdownError(message, line_at(first))


# `:name:`. Names are lower-case words; the few that have no letter are listed. (The letter
# is looked for ahead, so that a long word after a colon is read once, not split every way.)
_EMOJI_SHORTCODE = re.compile(r":(?:(?=[a-z0-9_+-]*[a-z])[a-z0-9_+-]+|100|1234|\+1|-1):")


def _text_nodes(nodes):
    """The text and raw-HTML nodes below `nodes`, in reading order (no code, no addresses)."""
    for node in nodes:
        if node.kind in ("text", "html"):
            yield node
        elif node.children:
            for inner in _text_nodes(node.children):
                yield inner


def _decode_reference(reference):
    """`&amp;`, `&#38;`, `&#x26;` to the character; None if it is not a known reference."""
    body = reference[1:-1]
    if body.startswith("#"):
        try:
            code = int(body[2:], 16) if body[1] in "xX" else int(body[1:])
        except ValueError:
            return None
        if code == 0 or code > 0x10FFFF or 0xD800 <= code <= 0xDFFF:
            return "�"
        return chr(code)
    return _HTML5_ENTITIES.get(body + ";")


def _contains_link(nodes):
    for node in nodes:
        if node.kind in ("link", "autolink"):
            return True
        if getattr(node, "children", None) and _contains_link(node.children):
            return True
    return False


def _contains_kind(nodes, kind):
    for node in nodes:
        if node.kind == kind:
            return True
        if getattr(node, "children", None) and _contains_kind(node.children, kind):
            return True
    return False


def _merge_text(nodes):
    """Adjacent text nodes become one, joined once (not by repeated concatenation)."""
    merged, run = [], []

    def close():
        if len(run) == 1:
            merged.append(run[0])
        elif run:
            merged.append(Inline("text", "".join(node.text for node in run), line=run[0].line))
        del run[:]

    for node in nodes:
        if node.kind == "text":
            run.append(node)
            continue
        close()
        if node.children:
            node.children = _merge_text(node.children)
        merged.append(node)
    close()
    return merged


class _Cell:
    """A place in the doubly linked list of nodes that emphasis is resolved on."""

    __slots__ = ("node", "prev", "next")

    def __init__(self, node, prev=None, following=None):
        self.node = node
        self.prev = prev
        self.next = following


def _unchain(delimiter):
    """Take a delimiter out of play; its characters stay in the nodes, as text."""
    if delimiter.before is not None:
        delimiter.before.after = delimiter.after
    if delimiter.after is not None:
        delimiter.after.before = delimiter.before
    delimiter.before = delimiter.after = None


def _remove_cell(cell):
    cell.prev.next = cell.next
    if cell.next is not None:
        cell.next.prev = cell.prev


def _resolve_emphasis(nodes):
    """CommonMark's emphasis algorithm exactly as GitHub's renderer runs it (process_emphasis
    in src/inlines.c of cmark-gfm 0.29.0.gfm.13), its strikethrough extension included:

    * closers are taken from left to right; each looks back for the nearest opener of its
      character, passing over a pair that the rule of three forbids;
    * a closer that finds none sets a bound below which later closers of the same character
      and the same run length modulo 3 do not look. The bound decides results, not only the
      time taken: CommonMark 0.30 changed it (it depends on whether the closer can open as
      well), and GitHub has the older one. It also keeps the work linear;
    * a `~` closer that finds a `~` opener of its own length makes strikethrough, which is
      refused here; one of another length takes itself, the opener and every delimiter
      between them out of play, which can leave a `*` without its partner.

    Delimiter runs that find no partner stay text, as in CommonMark (`set*id`)."""
    head = tail = _Cell(None)
    chain = []
    for node in nodes:
        if node.kind == "bracket":
            raise AssertionError("unresolved bracket")
        cell = _Cell(node, tail)
        tail.next = cell
        tail = cell
        if node.kind == "delimiter":
            node.cell, node.position, node.after = cell, len(chain), None
            node.before = chain[-1] if chain else None
            if chain:
                chain[-1].after = node
            chain.append(node)
    if not chain:
        return list(nodes)

    bottoms = {}  # (character, run length % 3) -> the lowest position an opener may have
    closer = chain[0]
    while closer is not None:
        if not closer.can_close:
            closer = closer.after
            continue
        bottom = bottoms.get((closer.char, closer.original % 3), 0)
        opener = closer.before
        while opener is not None and opener.position >= bottom:
            if opener.can_open and opener.char == closer.char and (
                not (closer.can_open or opener.can_close)
                or closer.original % 3 == 0
                or (opener.original + closer.original) % 3 != 0
            ):
                break
            opener = opener.before
        else:
            opener = None
        old_closer = closer
        if opener is None:
            closer = closer.after
            bottoms[(old_closer.char, old_closer.original % 3)] = old_closer.position
            if not old_closer.can_open:
                _unchain(old_closer)
        elif closer.char == "~":
            if opener.count == closer.count:
                raise MarkdownError("strikethrough (~text~) is not supported; escape the tildes if they are meant "
                                    "literally", closer.line)
            closer = closer.after
            delimiter = old_closer
            while delimiter is not None and delimiter is not opener:
                previous = delimiter.before
                _unchain(delimiter)
                delimiter = previous
            _unchain(opener)
        else:
            closer = _insert_emphasis(opener, closer)

    result = []
    cell = head.next
    while cell is not None:
        node = cell.node
        if node.kind != "delimiter":
            result.append(node)
        elif node.count:
            result.append(_literal(node))
        cell = cell.next
    return result


def _insert_emphasis(opener, closer):
    """Wrap what stands between `opener` and `closer` in emphasis (taking one character of
    each run) or strong emphasis (two); return the closer to go on with (S_insert_emph)."""
    use = 2 if opener.count >= 2 and closer.count >= 2 else 1
    opener.count -= use
    closer.count -= use
    delimiter = closer.before
    while delimiter is not None and delimiter is not opener:
        previous = delimiter.before
        _unchain(delimiter)
        delimiter = previous
    children = []
    cell = opener.cell.next
    while cell is not closer.cell:
        children.append(_literal(cell.node) if cell.node.kind == "delimiter" else cell.node)
        cell = cell.next
    if use == 2 and _contains_kind(children, "strong"):
        # CommonMark nests the two; GitHub's renderer writes one <strong>. Neither is
        # wrong, so there is no single faithful rendering.
        raise MarkdownError("strong emphasis inside strong emphasis (CommonMark and GitHub render it differently)",
                            opener.line)
    wrapped = _Cell(Inline("strong" if use == 2 else "emph", children=children, line=opener.line), opener.cell,
                    closer.cell)
    opener.cell.next = wrapped
    closer.cell.prev = wrapped
    if opener.count == 0:
        _remove_cell(opener.cell)
        _unchain(opener)
    if closer.count == 0:
        following = closer.after
        _remove_cell(closer.cell)
        _unchain(closer)
        return following
    return closer


def _literal(delimiter):
    """A delimiter run that found no partner is literal text, as in CommonMark (`set*id`)."""
    return Inline("text", delimiter.char * delimiter.count, line=delimiter.line)


# --------------------------------------------------------------------------------------
# Entry points
# --------------------------------------------------------------------------------------


def parse(text, source=None, repair=None):
    """Parse a document into a list of blocks. Raises MarkdownError.

    `repair`, if given, receives the numbered lines `[(number, text), …]` and returns the
    list to parse. It exists for stated corrections of known defects in a source that the
    caller may not edit; line numbers in errors stay those of the file."""
    if "\r" in text:
        text = text.replace("\r\n", "\n")
        if "\r" in text:
            line = text.count("\n", 0, text.index("\r")) + 1
            raise MarkdownError("carriage return without a line feed", line, source)
    try:
        _refuse_control_characters(text, source)
        lines = [(number, line) for number, line in enumerate(text.split("\n"), 1)]
        if lines and lines[-1][1] == "":
            lines.pop()
        if repair is not None:
            lines = repair(lines)
        try:
            return _BlockParser().parse(lines)
        except RecursionError:
            # (the nesting limits keep the converter far from this; it is the last guard)
            raise MarkdownError("the document is nested too deeply to be converted", None, source)
    except MarkdownError as error:
        if error.source is None:
            error.source = source
        raise


def plain_text(inlines):
    """The text of inline nodes without any markup (for slugs, labels and tests); raw HTML
    counts as the text it is shown as, and an HTML comment, which is not shown, as nothing."""
    parts = []
    for node in inlines:
        if node.kind == "comment":
            continue
        if node.kind in ("text", "html", "code", "autolink"):
            parts.append(node.text)
        elif node.kind in ("softbreak", "hardbreak"):
            parts.append(" ")
        elif node.children:
            parts.append(plain_text(node.children))
    return "".join(parts)


#: Letters that are not in a letter category but that GitHub keeps in a heading's anchor:
#: circled and squared Latin letters (symbols with the Unicode property Alphabetic).
_ALPHABETIC_SYMBOLS = _code_point_ranges("24B6-24E9 1F130-1F149 1F150-1F169 1F170-1F189")


def _in_ranges(ranges, code):
    starts, ends = ranges
    index = bisect.bisect_right(starts, code) - 1
    return index >= 0 and code <= ends[index]


def slugify(text):
    """GitHub's heading anchor, as its Markdown API shows it: each character in lower case
    on its own (a final Σ becomes σ, not ς), then everything dropped but letters (also
    letter numbers such as Ⅻ and circled letters such as Ⓐ), marks, decimal digits,
    connector punctuation such as '_', '-' and spaces, then spaces to hyphens. Nothing is
    trimmed (` a` in a code span becomes `-a`). Links written for GitHub therefore work
    here. An empty result means GitHub gives the heading no usable anchor."""
    kept = []
    for char in text:
        for lower in char.lower():
            category = unicodedata.category(lower)
            if (lower in "- " or category[0] in "LM" or category in ("Nd", "Nl", "Pc")
                    or _in_ranges(_ALPHABETIC_SYMBOLS, ord(lower))):
                kept.append("-" if lower == " " else lower)
    return "".join(kept)


_ROW_ID = re.compile(r"^[A-Z]{1,3}[0-9]{0,3}$")
_BOX_DRAWING = re.compile("[─-╿]")
_DOUBLE_BRACE = re.compile(r"\{(?=\{)")


def escape(text, quote=True):
    """html.escape, and never two braces in a row: a document may show `{{SITE_URL}}`, but
    the output then never contains it, so the site generator's check for a placeholder
    left unsubstituted is not misled (`&#123;` is the same character to a reader)."""
    return _DOUBLE_BRACE.sub("&#123;", html.escape(text, quote=quote))


class Renderer:
    """Turns the tree into HTML. Subclass or pass callables to decide where links go.

    resolve(target, line) -> (href, outbound)   for `[text](target)`; the default keeps
                                                fragments and relative paths as they are.
    autolink(url) -> html                       for bare and angle-bracket addresses.

    Ids: a heading gets GitHub's anchor, numbered among the headings only, as GitHub
    numbers them (`used_ids` may be shared between renderers; a document's title counts,
    see `reserve_title`). With `row_ids`, a numbered table row gets `row-` and its label,
    an id of this site's own. `reserved_ids` are ids the page around the output uses. Where
    two of these would be the same, the converter refuses: renaming one silently would
    move a link written for GitHub, or one of the site's, somewhere else.
    """

    OUTBOUND_REL = "noopener noreferrer"

    def __init__(self, resolve=None, autolink=None, source=None, heading_anchors=False, row_ids=False, used_ids=None,
                 heading_shift=0, reserved_ids=()):
        self._resolve = resolve
        self._autolink = autolink
        self.source = source
        self.heading_anchors = heading_anchors
        self.row_ids = row_ids
        self.heading_shift = heading_shift
        self.ids = used_ids if used_ids is not None else set()
        self.rows = {}  # id of a table row -> its line
        self.reserved = frozenset(reserved_ids)
        self.headings = []  # (level, id, html, plain text)
        self.outbound = []  # every outbound address written, in order

    # -- ids ----------------------------------------------------------------------------

    def _anchor(self, inlines, line, what):
        """GitHub's anchor for a heading's text, numbered after the headings before it."""
        if _contains_kind(inlines, "html") or _contains_kind(inlines, "comment"):
            raise MarkdownError("raw HTML in %s (GitHub renders the tag and leaves it out of the anchor; here it is "
                                "shown as text, so the two would differ)" % what, line, self.source)
        wanted = slugify(plain_text(inlines))
        if not wanted:
            raise MarkdownError("%s has no letter or digit, so GitHub gives it no usable anchor" % what, line,
                                self.source)
        candidate, counter = wanted, 0
        while candidate in self.ids:
            counter += 1
            candidate = "%s-%d" % (wanted, counter)
        if candidate in self.rows:
            raise MarkdownError("%s gets the anchor %r on GitHub, which the table row at line %d has on this site "
                                "(rename the heading or the row)" % (what, candidate, self.rows[candidate]), line,
                                self.source)
        if candidate in self.reserved:
            raise MarkdownError("%s gets the anchor %r on GitHub, which the page around the document uses already "
                                "(rename the heading)" % (what, candidate), line, self.source)
        self.ids.add(candidate)
        return candidate

    def reserve_title(self, block):
        """Count a document's title as GitHub does: the first heading, whose anchor a later
        heading of the same name cannot have (that one is numbered). Returns the anchor;
        the page decides whether an element carries it."""
        return self._anchor(block.inlines, block.line, "the title")

    # -- inlines ------------------------------------------------------------------------

    def link_attributes(self, target, line):
        try:
            kind = classify_target(target)
        except ValueError as problem:
            raise MarkdownError(str(problem), line, self.source)
        if self._resolve is not None:
            try:
                href, outbound = self._resolve(target, kind)
            except ValueError as problem:
                raise MarkdownError(str(problem), line, self.source)
        else:
            href, outbound = target, kind[0] == "https"
        if outbound:
            problem = check_https_url(href)
            if problem:
                raise MarkdownError("refused link target %r: %s" % (href, problem), line, self.source)
            self.outbound.append(href)
            return 'href="%s" rel="%s"' % (escape(href), self.OUTBOUND_REL)
        return 'href="%s"' % escape(href)

    def inlines(self, nodes):
        try:
            return self._inlines(nodes)
        except RecursionError:
            raise MarkdownError("nested too deeply to be rendered", None, self.source)

    def _inlines(self, nodes):
        out = []
        for node in nodes:
            kind = node.kind
            if kind in ("text", "html"):
                out.append(escape(node.text, quote=False))
            elif kind == "comment":
                pass  # (GitHub shows no HTML comment; see _is_comment)
            elif kind == "code":
                out.append("<code>%s</code>" % escape(node.text, quote=False))
            elif kind == "emph":
                out.append("<em>%s</em>" % self._inlines(node.children))
            elif kind == "strong":
                out.append("<strong>%s</strong>" % self._inlines(node.children))
            elif kind == "link":
                out.append("<a %s>%s</a>" % (self.link_attributes(node.target, node.line), self._inlines(node.children)))
            elif kind == "autolink":
                problem = check_https_url(node.text)
                if problem:
                    raise MarkdownError("refused address %r: %s" % (node.text, problem), node.line, self.source)
                self.outbound.append(node.text)
                if self._autolink is not None:
                    out.append(self._autolink(node.text))
                else:
                    escaped = escape(node.text)
                    out.append('<a href="%s" rel="%s">%s</a>' % (escaped, self.OUTBOUND_REL, escaped))
            elif kind == "softbreak":
                out.append("\n")
            elif kind == "hardbreak":
                out.append("<br>\n")
            else:
                raise AssertionError("unknown inline node %r" % kind)
        return "".join(out)

    # -- blocks -------------------------------------------------------------------------

    def blocks(self, blocks, tight=False):
        try:
            return self._blocks(blocks, tight)
        except RecursionError:
            line = blocks[0].line if blocks else None
            raise MarkdownError("nested too deeply to be rendered", line, self.source)

    def _blocks(self, blocks, tight=False):
        out = []
        for block in blocks:
            kind = block.kind
            if kind == "paragraph":
                body = self._inlines(block.inlines)
                out.append(body if tight else "<p>%s</p>" % body)
            elif kind == "heading":
                out.append(self._heading(block))
            elif kind == "code":
                out.append(self._code(block))
            elif kind == "thematic":
                out.append("<hr>")
            elif kind == "quote":
                out.append("<blockquote>\n%s\n</blockquote>" % self._blocks(block.blocks))
            elif kind == "list":
                out.append(self._list(block))
            elif kind == "table":
                out.append(self._table(block))
            else:
                raise AssertionError("unknown block %r" % kind)
        return "\n".join(out)

    def _heading(self, block):
        level = block.level + self.heading_shift
        if not 1 <= level <= 6:
            raise MarkdownError("heading level %d cannot be shifted by %d" % (block.level, self.heading_shift), block.line,
                                self.source)
        text = plain_text(block.inlines)
        if _contains_link(block.inlines):
            raise MarkdownError("links inside headings are not supported", block.line, self.source)
        identifier = self._anchor(block.inlines, block.line, "the heading %r" % text)
        body = self._inlines(block.inlines)
        self.headings.append((level, identifier, body, text))
        heading = '<h%d id="%s">%s</h%d>' % (level, escape(identifier), body, level)
        if self.heading_anchors and level > 1:
            # Beside the heading, not inside it: the heading's name stays its own text.
            return '<div class="heading">%s<a class="anchor" href="#%s" aria-label="Permalink: %s">#</a></div>' % (
                heading,
                escape(identifier),
                escape(text),
            )
        return heading

    def _code(self, block):
        # Every code block takes keyboard focus: whether it scrolls depends on the window,
        # the font size the reader set and how deep it stands in a list, none of which is
        # known here, and a block that scrolls must be reachable without a mouse.
        attributes = ' class="diagram"' if _BOX_DRAWING.search(block.text) else ""
        language = ' class="language-%s"' % escape(block.info) if block.info else ""
        return '<pre%s tabindex="0"><code%s>%s</code></pre>' % (attributes, language, escape(block.text, quote=False))

    def _list(self, block):
        tag = "ol" if block.ordered else "ul"
        start = ' start="%d"' % block.start if block.ordered and block.start != 1 else ""
        out = ["<%s%s>" % (tag, start)]
        for item in block.items:
            if block.tight:
                parts = []
                for child in item:
                    rendered = self._blocks([child], tight=True)
                    parts.append(rendered)
                out.append("<li>%s</li>" % "\n".join(parts))
            else:
                out.append("<li>\n%s\n</li>" % self._blocks(item))
        out.append("</%s>" % tag)
        return "\n".join(out)

    def _table(self, block):
        labels = [plain_text(cell).strip() for cell in block.header]
        columns = len(labels)
        out = ['<div class="table-wrap">', '<table class="cols-%d" role="table">' % columns]
        if any(labels):
            out.append('<thead role="rowgroup">')
            cells = []
            for index, cell in enumerate(block.header):
                cells.append('<th role="columnheader" scope="col"%s>%s</th>' % (self._align(block, index), self._inlines(cell)))
            out.append('<tr role="row">%s</tr>' % "".join(cells))
            out.append("</thead>")
        if block.rows:
            out.append('<tbody role="rowgroup">')
        identified = self.row_ids and labels[0] in ("#", "ID")
        for row, line in zip(block.rows, block.row_lines):
            attributes = ""
            first = plain_text(row[0]).strip()
            if identified and _ROW_ID.match(first):
                wanted = "row-" + first.lower()
                if wanted in self.rows:
                    raise MarkdownError("two table rows are both called %s (the other is at line %d)"
                                        % (first, self.rows[wanted]), line, self.source)
                if wanted in self.ids:
                    raise MarkdownError("table row %s would get the id %r, which a heading has as its anchor on "
                                        "GitHub (rename the heading or the row)" % (first, wanted), line, self.source)
                if wanted in self.reserved:
                    raise MarkdownError("table row %s would get the id %r, which the page around the document uses "
                                        "already" % (first, wanted), line, self.source)
                self.rows[wanted] = line
                attributes = ' id="%s"' % wanted
            cells = []
            for index, cell in enumerate(row):
                label = ' data-label="%s"' % escape(labels[index]) if labels[index] else ""
                cells.append('<td role="cell"%s%s>%s</td>' % (label, self._align(block, index), self._inlines(cell)))
            out.append('<tr role="row"%s>%s</tr>' % (attributes, "".join(cells)))
        if block.rows:
            out.append("</tbody>")
        out.append("</table>")
        out.append("</div>")
        return "\n".join(out)

    @staticmethod
    def _align(block, index):
        align = block.aligns[index]
        return ' class="align-%s"' % align if align else ""


def render(blocks, renderer=None):
    """Render blocks with `renderer` (a fresh default one if None); returns the HTML."""
    renderer = renderer or Renderer()
    return renderer.blocks(blocks)


def to_html(text, source=None, **options):
    """Convenience: parse and render in one step."""
    return render(parse(text, source), Renderer(source=source, **options)) + "\n"


def main(argv=None):
    """`python3 md.py FILE` prints the HTML; an error names FILE and the line, status 1."""
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) != 1:
        sys.stderr.write("usage: md.py FILE\n")
        return 2
    try:
        with open(arguments[0], encoding="utf-8") as handle:
            text = handle.read()
        sys.stdout.write(to_html(text, source=arguments[0]))
    except (MarkdownError, OSError, UnicodeDecodeError) as error:
        sys.stderr.write("error: %s\n" % error)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
