#!/usr/bin/env python3
"""Unit tests for site/md.py, the strict Markdown converter.

    python3 -m unittest discover -s site -p "test_*.py"

Three groups:

  * every construct the project's documents use, with the exact HTML expected;
  * everything the converter refuses, each with the file and the line it must name;
  * hostile input, where only two outcomes are acceptable: a refusal, or output in which
    nothing that came from the source is markup.

Only the Python standard library is used, and nothing is written anywhere.
"""

from __future__ import annotations

import contextlib
import io
import os
import random
import sys
import tempfile
import time
import unittest
from html.parser import HTMLParser
from pathlib import Path

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import md  # noqa: E402  (the converter lives next to this file)

SOURCE = "doc.md"

#: Everything the converter itself may write. Anything else in its output came from the
#: source, and that is what must never happen.
OWN_ELEMENTS = frozenset(
    "p h1 h2 h3 h4 h5 h6 a code pre em strong ul ol li table thead tbody tr th td div blockquote hr br".split()
)
OWN_ATTRIBUTES = frozenset("href rel id class role scope start tabindex data-label aria-label".split())
OWN_CLASSES = frozenset("table-wrap heading anchor diagram align-left align-center align-right".split())


class OwnMarkupOnly(HTMLParser):
    """Reads converter output and notes everything that the converter does not write."""

    def __init__(self):
        HTMLParser.__init__(self, convert_charrefs=True)
        self.foreign = []
        self.open = []
        self.hrefs = []

    def handle_starttag(self, tag, attrs):
        if tag not in OWN_ELEMENTS:
            self.foreign.append("<%s>" % tag)
        for name, value in attrs:
            if name not in OWN_ATTRIBUTES:
                self.foreign.append("%s= on <%s>" % (name, tag))
            if name == "class":
                for word in (value or "").split():
                    if word not in OWN_CLASSES and not word.startswith(("cols-", "language-")):
                        self.foreign.append("class %s" % word)
            if name == "href":
                self.hrefs.append(value)
                if not (value.startswith("#") or md.check_https_url(value) is None or _is_relative(value)):
                    self.foreign.append("href %r" % value)
            if name == "rel" and value != "noopener noreferrer":
                self.foreign.append("rel %r" % value)
        if tag not in ("hr", "br"):
            self.open.append(tag)

    def handle_startendtag(self, tag, attrs):
        self.foreign.append("<%s/>" % tag)

    def handle_endtag(self, tag):
        if not self.open or self.open[-1] != tag:
            self.foreign.append("</%s> out of place" % tag)
        else:
            self.open.pop()

    def handle_comment(self, data):
        self.foreign.append("comment")

    def handle_decl(self, decl):
        self.foreign.append("declaration")

    def handle_pi(self, data):
        self.foreign.append("processing instruction")

    def unknown_decl(self, data):
        self.foreign.append("marked section")


def _is_relative(target):
    try:
        return md.classify_target(target)[0] == "relative"
    except ValueError:
        return False


def foreign_markup(html_text):
    """What `html_text` contains that the converter does not write itself (a list)."""
    scan = OwnMarkupOnly()
    scan.feed(html_text)
    scan.close()
    return scan.foreign + ["unclosed <%s>" % tag for tag in scan.open]


class ConverterCase(unittest.TestCase):
    maxDiff = None

    def assertRenders(self, markdown, expected, **options):
        self.assertEqual(md.to_html(markdown, source=SOURCE, **options), expected)

    def assertRefused(self, markdown, line, part, **options):
        """The conversion stops, and the error names the file, the line and the reason."""
        with self.assertRaises(md.MarkdownError) as caught:
            md.to_html(markdown, source=SOURCE, **options)
        error = caught.exception
        self.assertIn(part, error.message)
        self.assertEqual((error.source, error.line), (SOURCE, line), str(error))
        where = "%s: " % SOURCE if line is None else "%s:%d: " % (SOURCE, line)
        self.assertTrue(str(error).startswith(where), str(error))


# --------------------------------------------------------------------------------------
# What the documents use
# --------------------------------------------------------------------------------------


class Headings(ConverterCase):
    def test_levels_and_ids(self):
        self.assertRenders(
            "# Title\n\n## Second level\n\n### Third level\n",
            '<h1 id="title">Title</h1>\n<h2 id="second-level">Second level</h2>\n<h3 id="third-level">Third level</h3>\n',
        )

    def test_ids_are_githubs_anchors_for_the_documents_own_headings(self):
        # Links written for GitHub (README.md -> docs/THREAT_MODEL.md#…) must work here.
        for heading, anchor in (
            ("Phase 1 — this machine (one minute)", "phase-1--this-machine-one-minute"),
            ("4.1 `/proc` snooping and local observation (adversaries O, U)",
             "41-proc-snooping-and-local-observation-adversaries-o-u"),
            ("5. Not defended — read this before relying on it", "5-not-defended--read-this-before-relying-on-it"),
            ("Read this first: which device are you on?", "read-this-first-which-device-are-you-on"),
            ("Option 1 — `kernel.yama.ptrace_scope = 3`", "option-1--kernelyamaptrace_scope--3"),
            ("Your own OAuth application", "your-own-oauth-application"),
        ):
            with self.subTest(heading=heading):
                self.assertIn('<h2 id="%s">' % anchor, md.to_html("## %s\n" % heading))

    def test_slugify(self):
        self.assertEqual(md.slugify("a_b-c d"), "a_b-c-d")
        self.assertEqual(md.slugify("C++ & C#"), "c--c")
        self.assertEqual(md.slugify("Über Größe"), "über-größe")
        self.assertEqual(md.slugify("!!!"), "")

    def test_slugify_does_what_githubs_markdown_api_does(self):
        # Each pair is GitHub's own answer (its Markdown API, 2026-11-14; renderer-8).
        for text, anchor in (
            ("ΟΔΟΣ ΤΕΣΤ", "οδοσ-τεστ"),  # one character at a time: no final ς
            (" a lead", "-a-lead"),  # nothing is trimmed (` a` in a code span)
            ("trail b ", "trail-b-"),
            ("⒜ paren ⓜ circ 🇩🇪 flag", "-paren-ⓜ-circ--flag"),  # circled letters stay, the rest goes
            ("Ⓐ capital", "ⓐ-capital"),
            ("🄰 squared 🅐 negative", "🄰-squared-🅐-negative"),  # (probe6)
            ("Ⅻ twelve", "ⅻ-twelve"),  # letter numbers stay
            ("Ⅷ ² ½ Ⓐ x٣", "ⅷ---ⓐ-x٣"),
            ("᙭ x", "-x"),
            ("x\u00a0y", "xy"),
            ("Ünïcode Ärger ÉCOLE", "ünïcode-ärger-école"),
            ("4.3 Display & code (the deliberate holes) — x_y?", "43-display--code-the-deliberate-holes--x_y"),
        ):
            with self.subTest(text=text):
                self.assertEqual(md.slugify(text), anchor)

    def test_a_repeated_heading_gets_a_number_as_on_github(self):
        self.assertRenders(
            "## Status\n\n## Status\n\n## Status\n",
            '<h2 id="status">Status</h2>\n<h2 id="status-1">Status</h2>\n<h2 id="status-2">Status</h2>\n',
        )

    def test_a_heading_without_a_letter_or_digit_is_refused(self):
        # GitHub gives it the id "" (and the next one "-1"): no anchor a link can use.
        self.assertRefused("text\n\n## !!!\n", 3, "no letter or digit")
        self.assertRefused("## ???\n", 1, "no letter or digit")

    def test_markup_inside_a_heading(self):
        self.assertRenders(
            "### `code` and *emphasis* & more\n",
            '<h3 id="code-and-emphasis--more"><code>code</code> and <em>emphasis</em> &amp; more</h3>\n',
        )

    def test_closing_hashes_are_dropped_and_others_kept(self):
        self.assertRenders("## Title ##\n\n## Trailing #hash\n",
                           '<h2 id="title">Title</h2>\n<h2 id="trailing-hash">Trailing #hash</h2>\n')

    def test_a_heading_needs_the_space_and_at_most_six_hashes(self):
        self.assertRenders("#hashtag\n", "<p>#hashtag</p>\n")
        self.assertRenders("####### seven\n", "<p>####### seven</p>\n")

    def test_a_heading_ends_a_paragraph_without_a_blank_line(self):
        self.assertRenders("text\n## Heading\nmore\n", '<p>text</p>\n<h2 id="heading">Heading</h2>\n<p>more</p>\n')

    def test_permalinks_stand_beside_the_heading_not_inside_it(self):
        self.assertRenders(
            "# Top\n\n## Two & `three`\n",
            '<h1 id="top">Top</h1>\n'
            '<div class="heading"><h2 id="two--three">Two &amp; <code>three</code></h2>'
            '<a class="anchor" href="#two--three" aria-label="Permalink: Two &amp; three">#</a></div>\n',
            heading_anchors=True,
        )

    def test_headings_can_be_shifted_down(self):
        self.assertRenders("## One\n", '<h3 id="one">One</h3>\n', heading_shift=1)
        self.assertRefused("\n###### Six\n", 2, "cannot be shifted", heading_shift=1)

    def test_the_renderer_remembers_its_headings_for_a_table_of_contents(self):
        renderer = md.Renderer()
        renderer.blocks(md.parse("# T\n\n## A `b`\n\n### C\n"))
        self.assertEqual(renderer.headings, [(1, "t", "T", "T"), (2, "a-b", "A <code>b</code>", "A b"), (3, "c", "C", "C")])

    def test_refused_in_headings(self):
        self.assertRefused("text\n\n# \n", 3, "empty heading")
        self.assertRefused("## See [the manual](#x)\n", 1, "links inside headings")


class Paragraphs(ConverterCase):
    def test_lines_of_a_paragraph_stay_lines(self):
        self.assertRenders("A paragraph\nover two lines.\n\nAnother one.\n",
                           "<p>A paragraph\nover two lines.</p>\n<p>Another one.</p>\n")

    def test_several_blank_lines_are_one_separator(self):
        self.assertRenders("a\n\n\n\nb\n", "<p>a</p>\n<p>b</p>\n")

    def test_leading_spaces_of_continuation_lines_are_dropped(self):
        self.assertRenders("one\n   two\n", "<p>one\ntwo</p>\n")

    def test_text_is_escaped(self):
        self.assertRenders('5 < 6 > 4; AT&T; "quoted" and \'single\'\n',
                           '<p>5 &lt; 6 &gt; 4; AT&amp;T; "quoted" and \'single\'</p>\n')

    def test_characters_outside_ascii_are_kept(self):
        self.assertRenders("Microsoft → Xbox → Minecraft — “quoted” ×3\n", "<p>Microsoft → Xbox → Minecraft — “quoted” ×3</p>\n")

    def test_character_references_are_decoded_and_escaped_again(self):
        self.assertRenders("&amp; &lt;b&gt; &#60; &#x41; &copy; &nosuch; &#0;\n",
                           "<p>&amp; &lt;b&gt; &lt; A © &amp;nosuch; �</p>\n")

    def test_backslash_escapes(self):
        self.assertRenders("a\\*b\\*c \\_x\\_ \\<b\\> \\\\ \\# \\[not a link\\](x)\n",
                           "<p>a*b*c _x_ &lt;b&gt; \\ # [not a link](x)</p>\n")

    def test_a_backslash_before_anything_else_is_a_backslash(self):
        self.assertRenders("back\\slash and \\n and end\\\n", "<p>back\\slash and \\n and end\\</p>\n")

    def test_hard_line_breaks(self):
        self.assertRenders("line one  \nline two\n", "<p>line one<br>\nline two</p>\n")
        self.assertRenders("line one\\\nline two\n", "<p>line one<br>\nline two</p>\n")

    def test_thematic_break(self):
        self.assertRenders("Before\n\n---\n\nAfter\n", "<p>Before</p>\n<hr>\n<p>After</p>\n")
        self.assertRenders("***\n___\n- - -\n", "<hr>\n<hr>\n<hr>\n")

    def test_windows_line_endings(self):
        self.assertRenders("a\r\nb\r\n", "<p>a\nb</p>\n")
        self.assertRefused("a\rb\n", 1, "carriage return without a line feed")
        self.assertRefused("# T\r\n\r\ntext\rmore\n", 3, "carriage return without a line feed")

    def test_block_quote(self):
        self.assertRenders("> quoted *text*\n> more\n>\n> second\n\nafter\n",
                           "<blockquote>\n<p>quoted <em>text</em>\nmore</p>\n<p>second</p>\n</blockquote>\n<p>after</p>\n")


class CodeBlocks(ConverterCase):
    def test_fence_with_a_language(self):
        self.assertRenders(
            "```sh\ncargo build --release   # or: makepkg -si\n```\n",
            '<pre tabindex="0"><code class="language-sh">cargo build --release   # or: makepkg -si\n</code></pre>\n',
        )

    def test_fence_without_a_language(self):
        self.assertRenders("```\nplain\n```\n", "<pre tabindex=\"0\"><code>plain\n</code></pre>\n")

    def test_the_content_is_text_and_nothing_else(self):
        # From docs/TOKEN_INJECTION.md: angle brackets, quotes, a backslash, a pipe.
        self.assertRenders(
            "```\n$ tr '\\0' ' ' < /proc/<jvm>/cmdline | tail -c 120\n*not emphasis* [not](a-link) &amp; <b>\n```\n",
            "<pre tabindex=\"0\"><code>$ tr '\\0' ' ' &lt; /proc/&lt;jvm&gt;/cmdline | tail -c 120\n"
            "*not emphasis* [not](a-link) &amp;amp; &lt;b&gt;\n</code></pre>\n",
        )

    def test_blank_lines_and_indentation_inside_are_kept(self):
        self.assertRenders("```toml\n[a]\n\n  b = \"c\"\n```\n",
                           '<pre tabindex="0"><code class="language-toml">[a]\n\n  b = "c"\n</code></pre>\n')

    def test_tilde_fence_and_longer_fence(self):
        self.assertRenders("~~~sh\necho ```\n~~~\n", '<pre tabindex="0"><code class="language-sh">echo ```\n</code></pre>\n')
        self.assertRenders("````\n```\ninner\n```\n````\n", "<pre tabindex=\"0\"><code>```\ninner\n```\n</code></pre>\n")

    def test_a_fence_ends_a_paragraph(self):
        self.assertRenders("text\n```\ncode\n```\nmore\n", "<p>text</p>\n<pre tabindex=\"0\"><code>code\n</code></pre>\n<p>more</p>\n")

    def test_fence_inside_a_list_item(self):
        # As in the README's numbered instructions.
        self.assertRenders(
            '1. Register:\n\n   ```toml\n   client_id = "x"\n     indented = true\n   ```\n\n2. Next.\n',
            '<ol>\n<li>\n<p>Register:</p>\n<pre tabindex="0"><code class="language-toml">client_id = "x"\n  indented = true\n</code></pre>\n'
            "</li>\n<li>\n<p>Next.</p>\n</li>\n</ol>\n",
        )

    def test_indented_code(self):
        self.assertRenders("text\n\n    indented <code>\n\n    after a blank line\n\ntext\n",
                           "<p>text</p>\n<pre tabindex=\"0\"><code>indented &lt;code&gt;\n\nafter a blank line\n</code></pre>\n<p>text</p>\n")

    def test_a_drawing_made_of_box_characters_is_marked(self):
        # The threat model's trust-boundary drawing: the style sheet must not wrap it.
        self.assertRenders("```\n ┌──┐\n └──┘\n```\n", '<pre class="diagram" tabindex="0"><code> ┌──┐\n └──┘\n</code></pre>\n')

    def test_every_code_block_can_be_reached_with_the_keyboard(self):
        # Whether a block scrolls depends on the window, the reader's font size and the
        # depth of the list it stands in; the converter knows none of them. The manual's
        # `[auth]` block (50 characters) scrolled at 360 px without being focusable (web-2).
        for width in (1, 16, 37, 40, 50, 61, 120):
            with self.subTest(width=width):
                self.assertRenders("```\n%s\n```\n" % ("x" * width), '<pre tabindex="0"><code>%s\n</code></pre>\n' % ("x" * width))
        self.assertRenders("- item\n\n      indented\n", '<ul>\n<li>\n<p>item</p>\n<pre tabindex="0"><code>indented\n</code></pre>\n</li>\n</ul>\n')

    def test_an_unterminated_fence_is_an_error_at_the_line_that_opened_it(self):
        self.assertRefused("intro\n\n```sh\nnever closed\n", 3, "never closed")
        self.assertRefused("~~~\nnot closed by backticks\n```\n", 1, "never closed")
        self.assertRefused("- item\n\n  ```\n  still open\n", 3, "never closed")

    def test_the_info_string_is_one_plain_word(self):
        self.assertRefused('```sh" onclick="x\necho\n```\n', 1, "unsupported info string")
        self.assertRefused("```sh linenos\necho\n```\n", 1, "unsupported info string")
        self.assertRefused("``` a`b\n", 1, "backtick in the info string")

    def test_blocks_github_would_draw_instead_of_showing(self):
        for language in ("mermaid", "math", "geojson", "topojson", "stl", "Mermaid"):
            with self.subTest(language=language):
                self.assertRefused("```%s\nx\n```\n" % language, 1, "renders a `%s` block as a picture" % language)


class Lists(ConverterCase):
    def test_tight_bullet_list(self):
        self.assertRenders("- one\n- two with `code`\n- three\n",
                           "<ul>\n<li>one</li>\n<li>two with <code>code</code></li>\n<li>three</li>\n</ul>\n")

    def test_tight_numbered_list(self):
        self.assertRenders("1. first\n2. second\n3. third\n", "<ol>\n<li>first</li>\n<li>second</li>\n<li>third</li>\n</ol>\n")
        self.assertRenders("1) paren\n2) list\n", "<ol>\n<li>paren</li>\n<li>list</li>\n</ol>\n")

    def test_a_list_that_does_not_start_at_one_says_so(self):
        self.assertRenders("3. three\n4. four\n", '<ol start="3">\n<li>three</li>\n<li>four</li>\n</ol>\n')

    def test_item_text_over_several_lines(self):
        self.assertRenders("- first line\n  second line\n- next\n", "<ul>\n<li>first line\nsecond line</li>\n<li>next</li>\n</ul>\n")

    def test_blank_lines_between_items_make_paragraphs(self):
        self.assertRenders("- a\n\n- b\n", "<ul>\n<li>\n<p>a</p>\n</li>\n<li>\n<p>b</p>\n</li>\n</ul>\n")

    def test_nested_lists(self):
        self.assertRenders(
            "- a\n  - b\n    - c\n- d\n",
            "<ul>\n<li>a\n<ul>\n<li>b\n<ul>\n<li>c</li>\n</ul></li>\n</ul></li>\n<li>d</li>\n</ul>\n",
        )

    def test_the_runbooks_numbered_steps(self):
        # docs/SCRAM_RUNBOOK.md, phase 2: loose items, addresses on lines of their own, a
        # nested bullet list.
        self.assertRenders(
            "1. **Change the password.**\n"
            "   https://go.microsoft.com/fwlink/p/?linkid=2071806\n"
            "   If you cannot sign in:\n"
            "   https://go.microsoft.com/fwlink/p/?LinkID=238656\n"
            "\n"
            "2. **Sign out.** Both:\n"
            "   https://account.live.com/proofs/manage/additional\n"
            "   - *Sign out everywhere*: yes\n"
            "   - *Recovery code*: no\n",
            "<ol>\n<li>\n<p><strong>Change the password.</strong>\n"
            '<a href="https://go.microsoft.com/fwlink/p/?linkid=2071806" rel="noopener noreferrer">'
            "https://go.microsoft.com/fwlink/p/?linkid=2071806</a>\n"
            "If you cannot sign in:\n"
            '<a href="https://go.microsoft.com/fwlink/p/?LinkID=238656" rel="noopener noreferrer">'
            "https://go.microsoft.com/fwlink/p/?LinkID=238656</a></p>\n</li>\n"
            "<li>\n<p><strong>Sign out.</strong> Both:\n"
            '<a href="https://account.live.com/proofs/manage/additional" rel="noopener noreferrer">'
            "https://account.live.com/proofs/manage/additional</a></p>\n"
            "<ul>\n<li><em>Sign out everywhere</em>: yes</li>\n<li><em>Recovery code</em>: no</li>\n</ul>\n</li>\n</ol>\n",
        )

    def test_an_item_with_several_blocks(self):
        self.assertRenders(
            "- item\n\n  second paragraph\n\n      indented code\n- next\n",
            "<ul>\n<li>\n<p>item</p>\n<p>second paragraph</p>\n<pre tabindex=\"0\"><code>indented code\n</code></pre>\n</li>\n"
            "<li>\n<p>next</p>\n</li>\n</ul>\n",
        )

    def test_another_marker_starts_another_list(self):
        self.assertRenders("* star\n* list\n\n+ plus\n", "<ul>\n<li>star</li>\n<li>list</li>\n</ul>\n<ul>\n<li>plus</li>\n</ul>\n")

    def test_the_list_ends_at_a_paragraph_after_a_blank_line(self):
        self.assertRenders("- a\n- b\n\ntext after\n", "<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n<p>text after</p>\n")

    def test_only_a_list_that_starts_at_one_interrupts_a_paragraph(self):
        self.assertRenders("The year was\n1986. A good one.\n", "<p>The year was\n1986. A good one.</p>\n")
        self.assertRenders("text\n1. item\n", "<p>text</p>\n<ol>\n<li>item</li>\n</ol>\n")

    def test_refused_in_lists(self):
        self.assertRefused("- item\nlazy line\n", 2, "lazy continuation line")
        self.assertRefused("* a\n  * b\n   * c\n", 3, "indented differently")
        self.assertRefused("- a\n-\n", 2, "empty list item")
        self.assertRefused("text\n\n- [ ] to do\n", 3, "task list items")
        self.assertRefused("- [x] done\n", 1, "task list items")
        self.assertRefused("-      five spaces\n", 1, "begins with an indented code block")


class Tables(ConverterCase):
    def test_a_table_as_the_threat_model_writes_them(self):
        self.assertRenders(
            "| # | Attack | Defence |\n|---|---|---|\n| J5 | x32 ABI | **bold** and *em* |\n",
            '<div class="table-wrap">\n<table class="cols-3" role="table">\n<thead role="rowgroup">\n'
            '<tr role="row"><th role="columnheader" scope="col">#</th><th role="columnheader" scope="col">Attack</th>'
            '<th role="columnheader" scope="col">Defence</th></tr>\n</thead>\n<tbody role="rowgroup">\n'
            '<tr role="row"><td role="cell" data-label="#">J5</td><td role="cell" data-label="Attack">x32 ABI</td>'
            '<td role="cell" data-label="Defence"><strong>bold</strong> and <em>em</em></td></tr>\n'
            "</tbody>\n</table>\n</div>\n",
        )

    def test_an_escaped_pipe_is_a_pipe_also_inside_a_code_span(self):
        # docs/THREAT_MODEL.md, rows J5 and E4.
        html_text = md.to_html("| a |\n|---|\n| `nr \\| 0x40000000` and `core_pattern=\\|systemd-coredump` and \\| |\n")
        self.assertIn('<td role="cell" data-label="a"><code>nr | 0x40000000</code> and '
                      "<code>core_pattern=|systemd-coredump</code> and |</td>", html_text)

    def test_a_table_with_an_empty_header_has_no_head(self):
        # The README's status table.
        self.assertRenders(
            "| | |\n|---|---|\n| Display | **Not** on a real Wayland session |\n",
            '<div class="table-wrap">\n<table class="cols-2" role="table">\n<tbody role="rowgroup">\n'
            '<tr role="row"><td role="cell">Display</td><td role="cell"><strong>Not</strong> on a real Wayland session</td></tr>\n'
            "</tbody>\n</table>\n</div>\n",
        )

    def test_links_and_code_in_cells(self):
        html_text = md.to_html("| a | b |\n|---|---|\n| [x](#y) | `code` https://example.com/z |\n")
        self.assertIn('<td role="cell" data-label="a"><a href="#y">x</a></td>', html_text)
        self.assertIn('<code>code</code> <a href="https://example.com/z" rel="noopener noreferrer">', html_text)

    def test_alignment(self):
        html_text = md.to_html("| L | C | R | N |\n|:---|:---:|---:|---|\n| a | b | c | d |\n")
        self.assertIn('<th role="columnheader" scope="col" class="align-left">L</th>', html_text)
        self.assertIn('<td role="cell" data-label="C" class="align-center">b</td>', html_text)
        self.assertIn('<td role="cell" data-label="R" class="align-right">c</td>', html_text)
        self.assertIn('<td role="cell" data-label="N">d</td>', html_text)

    def test_without_the_outer_pipes(self):
        html_text = md.to_html("a | b\n--- | ---\n1 | 2\n")
        self.assertIn('<td role="cell" data-label="a">1</td><td role="cell" data-label="b">2</td>', html_text)

    def test_a_table_without_rows(self):
        html_text = md.to_html("| a | b |\n|---|---|\n")
        self.assertIn("</thead>\n</table>", html_text)
        self.assertNotIn("<tbody", html_text)

    def test_rows_of_a_numbered_table_can_be_linked_to(self):
        html_text = md.to_html("| # | What |\n|---|---|\n| J1 | one |\n| E11 | two |\n| note | three |\n", row_ids=True)
        self.assertIn('<tr role="row" id="row-j1">', html_text)
        self.assertIn('<tr role="row" id="row-e11">', html_text)
        self.assertIn('<tr role="row"><td role="cell" data-label="#">note</td>', html_text)
        # Only where it was asked for, and only in tables whose first column is the number.
        self.assertNotIn("id=", md.to_html("| # | What |\n|---|---|\n| J1 | one |\n"))
        self.assertNotIn("id=", md.to_html("| Name | What |\n|---|---|\n| J1 | one |\n", row_ids=True))

    def test_two_rows_with_one_name_are_refused(self):
        self.assertRefused("| ID | W |\n|---|---|\n| E1 | one |\n\n| # | W |\n|---|---|\n| E1 | again |\n", 7,
                           "both called E1 (the other is at line 3)", row_ids=True)

    def test_what_github_would_hide_is_an_error_here(self):
        # GitHub drops the cells a row has too many, and ends a table at a blank line.
        self.assertRefused("| a | b |\n|---|---|\n| 1 | 2 | 3 |\n", 3, "table row has 3 cells, its header has 2")
        self.assertRefused("| a | b |\n|---|---|\n| 1 |\n", 3, "table row has 1 cells, its header has 2")
        self.assertRefused("| a | b |\n|---|---|\n| 1 | 2 |\n\n| 3 | 4 |\n", 5, "table row outside a table")
        self.assertRefused("| a | b |\n|---|---|\n| 1 | 2 |\nbelow\n", 4, "text directly below a table")
        self.assertRefused("| a | b |\n|---|---|---|\n", 1, "header has 2 cells but the row of dashes below it has 3")
        self.assertRefused("para\n| a | b |\n|---|---|\n", 2, "table row outside a table")
        self.assertRefused("para\n\n|---|---|\n", 3, "delimiter row without a header row")


class Inlines(ConverterCase):
    def test_code_spans(self):
        self.assertRenders("`code` and `a  b` and `<b>&amp;`\n",
                           "<p><code>code</code> and <code>a  b</code> and <code>&lt;b&gt;&amp;amp;</code></p>\n")

    def test_code_spans_that_contain_backticks(self):
        self.assertRenders("``double `tick` span`` and `` ` `` and ` padded `\n",
                           "<p><code>double `tick` span</code> and <code>`</code> and <code>padded</code></p>\n")

    def test_nothing_inside_a_code_span_is_markup(self):
        self.assertRenders("`*a* [b](c) \\_ https://example.com <x>`\n",
                           "<p><code>*a* [b](c) \\_ https://example.com &lt;x&gt;</code></p>\n")

    def test_emphasis_and_strong(self):
        self.assertRenders(
            "Text with *emphasis*, **strong**, _under_, __strong__ and ***both***.\n",
            "<p>Text with <em>emphasis</em>, <strong>strong</strong>, <em>under</em>, <strong>strong</strong> and "
            "<em><strong>both</strong></em>.</p>\n",
        )

    def test_emphasis_holds_other_markup_and_spans_lines(self):
        self.assertRenders("**strong with `code`** and *emphasis\nover lines with **strong***\n",
                           "<p><strong>strong with <code>code</code></strong> and <em>emphasis\nover lines with "
                           "<strong>strong</strong></em></p>\n")

    def test_underscores_inside_words_are_text(self):
        # The documents are full of names like ptrace_scope and allow_x11.
        self.assertRenders("snake_case_name, allow_x11 and RLIMIT_CORE\n", "<p>snake_case_name, allow_x11 and RLIMIT_CORE</p>\n")
        self.assertRenders("foo_bar_ and _foo_bar\n", "<p>foo_bar_ and _foo_bar</p>\n")
        # (a star may open and close inside a word; an underscore may not)
        self.assertRenders("5*6*7\n", "<p>5<em>6</em>7</p>\n")

    def test_stars_that_do_not_flank_text_are_text(self):
        self.assertRenders("5 * 3 * 2 and a * b\n", "<p>5 * 3 * 2 and a * b</p>\n")
        self.assertRenders("*not closed\n", "<p>*not closed</p>\n")

    def test_strong_inside_strong_has_no_single_faithful_rendering(self):
        self.assertRefused("x\n**a **b** c**\n", 2, "strong emphasis inside strong emphasis")

    def test_plain_text_of_inlines(self):
        inlines = md.parse("Some *emphasis*, `code`, [a link](#x), <https://example.com/> and a\nbreak\n")[0].inlines
        self.assertEqual(md.plain_text(inlines), "Some emphasis, code, a link, https://example.com/ and a break")


class Links(ConverterCase):
    def test_the_three_kinds_of_target(self):
        self.assertRenders(
            "[a](#x) [b](docs/B.md) [c](docs/B.md#z) [d](https://example.com) [e](https://example.com/p?q=1&r=2#f)\n",
            '<p><a href="#x">a</a> <a href="docs/B.md">b</a> <a href="docs/B.md#z">c</a> '
            '<a href="https://example.com" rel="noopener noreferrer">d</a> '
            '<a href="https://example.com/p?q=1&amp;r=2#f" rel="noopener noreferrer">e</a></p>\n',
        )

    def test_link_text_with_markup(self):
        # The README links its documents like this.
        self.assertRenders("[`docs/TOKEN_INJECTION.md`](docs/TOKEN_INJECTION.md) and [link *with* emphasis](#x)\n",
                           '<p><a href="docs/TOKEN_INJECTION.md"><code>docs/TOKEN_INJECTION.md</code></a> and '
                           '<a href="#x">link <em>with</em> emphasis</a></p>\n')

    def test_brackets_that_are_not_links_are_text(self):
        self.assertRenders("[just brackets] and [x] (y) and a ] alone\n", "<p>[just brackets] and [x] (y) and a ] alone</p>\n")

    def test_spaces_around_the_target_are_allowed(self):
        self.assertRenders("[text]( #frag )\n", '<p><a href="#frag">text</a></p>\n')

    def test_autolink_in_angle_brackets(self):
        # README: "until approved: <https://aka.ms/AppRegInfo>."
        self.assertRenders(
            "until approved: <https://aka.ms/AppRegInfo>. This can take time.\n",
            '<p>until approved: <a href="https://aka.ms/AppRegInfo" rel="noopener noreferrer">https://aka.ms/AppRegInfo</a>. '
            "This can take time.</p>\n",
        )

    def test_a_bare_address_is_linked_as_github_links_it(self):
        self.assertRenders(
            "Visit https://example.com/path. Or (https://example.com/a_(b)) and https://example.com/x, then.\n",
            '<p>Visit <a href="https://example.com/path" rel="noopener noreferrer">https://example.com/path</a>. Or '
            '(<a href="https://example.com/a_(b)" rel="noopener noreferrer">https://example.com/a_(b)</a>) and '
            '<a href="https://example.com/x" rel="noopener noreferrer">https://example.com/x</a>, then.</p>\n',
        )

    def test_what_follows_a_bare_address_is_not_part_of_it(self):
        for tail in (".", ",", ":", "!", "?", "...", ")", "*", "&amp;"):
            with self.subTest(tail=tail):
                html_text = md.to_html("see https://example.com/q%s\n" % tail)
                self.assertIn('<a href="https://example.com/q" rel="noopener noreferrer">https://example.com/q</a>', html_text)

    def test_an_ampersand_in_an_address_is_escaped_in_both_places(self):
        self.assertRenders("https://example.com/a?b=1&c=2\n",
                           '<p><a href="https://example.com/a?b=1&amp;c=2" rel="noopener noreferrer">'
                           "https://example.com/a?b=1&amp;c=2</a></p>\n")

    def test_an_address_inside_a_code_span_is_not_a_link(self):
        self.assertRenders("value `http://localhost` or `https://example.com`\n",
                           "<p>value <code>http://localhost</code> or <code>https://example.com</code></p>\n")

    def test_refused_link_syntax(self):
        self.assertRefused("[text][ref]\n", 1, "reference-style links")
        self.assertRefused("\n[ref]: https://example.com\n", 2, "link reference definitions")
        self.assertRefused('[t](https://example.com "title")\n', 1, "link titles")
        self.assertRefused("[t](<https://example.com>)\n", 1, "angle brackets")
        self.assertRefused("[unclosed](https://example.com\n", 1, "expected ')' after the target")
        self.assertRefused("x](https://example.com)\n", 1, "without an opening '['")
        self.assertRefused("[a [b](#c) d](#e)\n", 1, "a link inside the text of another link")
        self.assertRefused("[a <https://example.com/> b](#e)\n", 1, "a link inside the text of another link")
        self.assertRefused("[](https://example.com)\n", 1, "link without text")
        self.assertRefused("one\ntwo ![alt](image.png)\n", 2, "images are not supported")
        self.assertRefused("A note[^1]\n", 1, "footnotes")


class LinkPolicy(unittest.TestCase):
    """Nothing but a fragment, a relative path or a plain https address reaches an href."""

    def test_acceptable_https_addresses(self):
        for url in (
            "https://example.com",
            "https://example.com/",
            "https://go.microsoft.com/fwlink/p/?linkid=2071806",
            "https://account.live.com/proofs/manage/additional",
            "https://example.com/a%20b?c=d&e=f#frag-ment_1.2~3",
            "https://xn--bcher-kva.example/",
            "https://a.b.c.example/path(with)parens",
        ):
            with self.subTest(url=url):
                self.assertIsNone(md.check_https_url(url))

    def test_refused_addresses(self):
        for url in (
            "http://example.com/",
            "HTTPS://example.com/",
            "javascript:alert(1)",
            "data:text/html,x",
            "//example.com/",
            "https://",
            "https://localhost/",
            "https://example.com:8443/",
            "https://good.example@evil.example/",
            "https://good.example:pw@evil.example/",
            "https://exaмple.com/",  # a Cyrillic letter in the host
            "https://example.com/café",
            "https://example.com/a b",
            "https://example.com/\n",
            "https://example.com/\t",
            'https://example.com/"onmouseover="alert(1)',
            "https://example.com/'x",
            "https://example.com/<script>",
            "https://example.com/a\\b",
            "https://example.com/[x]",
            "https://example.com/{x}",
            "https://example.com/`x`",
            "https://example.com/a|b",
            "https://example.com/?q=&quot;",
            "https://-bad.example/",
            "https://bad-.example/",
            "https://exam_ple.com/",
            "https://example..com/",
            " https://example.com/",
            "",
        ):
            with self.subTest(url=url):
                self.assertIsNotNone(md.check_https_url(url))

    def test_classify(self):
        self.assertEqual(md.classify_target("#frag"), ("fragment", "frag"))
        self.assertEqual(md.classify_target("docs/FILE.md"), ("relative", "docs/FILE.md", ""))
        self.assertEqual(md.classify_target("../README.md#status"), ("relative", "../README.md", "status"))
        self.assertEqual(md.classify_target("https://example.com/x"), ("https", "https://example.com/x"))

    def test_classify_refuses_everything_else(self):
        for target in (
            "",
            "#",
            "# x",
            "#a b",
            "#a\"b",
            "docs/FILE.md#",
            "docs/FILE.md#a#b",
            "/etc/passwd",
            "//example.com/x",
            "\\\\server\\share",
            "docs\\FILE.md",
            "a b.md",
            "docs/FILE.md?x=1",
            "docs/café.md",
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "vbscript:x",
            "data:text/html;base64,PHNjcmlwdD4=",
            "mailto:someone@example.com",
            "file:///etc/passwd",
            "ftp://example.com/",
            "http://example.com/",
            "x:y",
            " #frag",
            "#frag\n",
            "docs/FILE.md\n",
        ):
            with self.subTest(target=target):
                with self.assertRaises(ValueError):
                    md.classify_target(target)


class RendererHooks(ConverterCase):
    @staticmethod
    def resolve(target, kind):
        if kind[0] == "https":
            return target, True
        if kind[0] == "fragment":
            return "page.html#" + kind[1], False
        if kind[1] == "gone.md":
            raise ValueError("link target %r does not exist" % target)
        if kind[1] == "src/main.rs":
            return "https://repository.invalid/blob/main/src/main.rs", True
        if kind[1] == "insecure":
            return "http://insecure.example/", True
        return "other.html" + ("#" + kind[2] if kind[2] else ""), False

    def test_the_caller_decides_where_links_go(self):
        renderer = md.Renderer(resolve=self.resolve, source=SOURCE)
        html_text = renderer.blocks(md.parse("[a](#x) [b](docs/B.md#y) [c](src/main.rs) [d](https://example.com/)\n", SOURCE))
        self.assertEqual(
            html_text,
            '<p><a href="page.html#x">a</a> <a href="other.html#y">b</a> '
            '<a href="https://repository.invalid/blob/main/src/main.rs" rel="noopener noreferrer">c</a> '
            '<a href="https://example.com/" rel="noopener noreferrer">d</a></p>',
        )
        self.assertEqual(renderer.outbound, ["https://repository.invalid/blob/main/src/main.rs", "https://example.com/"])

    def test_a_target_the_caller_cannot_place_is_an_error_at_the_links_line(self):
        for markdown, part in (("\n\n[x](gone.md)\n", "does not exist"), ("\n\n[x](insecure)\n", "only https://")):
            with self.subTest(markdown=markdown):
                with self.assertRaises(md.MarkdownError) as caught:
                    md.Renderer(resolve=self.resolve, source=SOURCE).blocks(md.parse(markdown, SOURCE))
                self.assertEqual((caught.exception.source, caught.exception.line), (SOURCE, 3))
                self.assertIn(part, caught.exception.message)

    def test_what_the_caller_returns_is_escaped_like_everything_else(self):
        renderer = md.Renderer(resolve=lambda target, kind: ('page.html?a=1&b="2"', False))
        self.assertEqual(renderer.blocks(md.parse("[x](#y)\n")), '<p><a href="page.html?a=1&amp;b=&quot;2&quot;">x</a></p>')

    def test_the_caller_can_draw_bare_addresses_itself(self):
        renderer = md.Renderer(autolink=lambda url: "[%s]" % url.upper())
        self.assertEqual(renderer.blocks(md.parse("<https://example.com/a> and https://example.com/b\n")),
                         "<p>[HTTPS://EXAMPLE.COM/A] and [HTTPS://EXAMPLE.COM/B]</p>")
        self.assertEqual(renderer.outbound, ["https://example.com/a", "https://example.com/b"])

    def test_ids_are_unique_across_renderers_that_share_a_set(self):
        used = set()
        first = md.Renderer(used_ids=used).blocks(md.parse("## Status\n"))
        second = md.Renderer(used_ids=used).blocks(md.parse("## Status\n"))
        self.assertEqual((first, second), ('<h2 id="status">Status</h2>', '<h2 id="status-1">Status</h2>'))
        self.assertEqual(used, {"status", "status-1"})


class ParseAndCommandLine(ConverterCase):
    def test_a_repair_sees_numbered_lines_and_errors_keep_the_files_line_numbers(self):
        seen = []

        def repair(lines):
            seen.extend(lines)
            return [(number, text) for number, text in lines if text != "drop me"]

        blocks = md.parse("first\ndrop me\n\nsecond\n", source=SOURCE, repair=repair)
        self.assertEqual(seen, [(1, "first"), (2, "drop me"), (3, ""), (4, "second")])
        self.assertEqual([(block.kind, block.line) for block in blocks], [("paragraph", 1), ("paragraph", 4)])
        with self.assertRaises(md.MarkdownError) as caught:
            md.parse("drop me\n\n\n![x](y.png)\n", source=SOURCE, repair=repair)
        self.assertEqual(caught.exception.line, 4)

    def test_blocks_know_their_lines(self):
        blocks = md.parse("# T\n\ntext\n\n- a\n- b\n\n| a |\n|---|\n| 1 |\n")
        self.assertEqual([(block.kind, block.line) for block in blocks],
                         [("heading", 1), ("paragraph", 3), ("list", 5), ("table", 8)])

    def run_main(self, arguments):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            status = md.main(arguments)
        return status, out.getvalue(), err.getvalue()

    def test_command_line(self):
        with tempfile.TemporaryDirectory(prefix="bastion-md-test-") as scratch:
            good, bad = os.path.join(scratch, "good.md"), os.path.join(scratch, "bad.md")
            with open(good, "w", encoding="utf-8") as handle:
                handle.write("ok *fine*\n")
            with open(bad, "w", encoding="utf-8") as handle:
                handle.write("line one\n\n![no](images.png)\n")
            self.assertEqual(self.run_main([good]), (0, "<p>ok <em>fine</em></p>\n", ""))
            self.assertEqual(self.run_main([bad]), (1, "", "error: %s:3: images are not supported\n" % bad))
            status, out, err = self.run_main([os.path.join(scratch, "missing.md")])
            self.assertEqual((status, out), (1, ""))
            self.assertTrue(err.startswith("error: "), err)
            self.assertEqual(self.run_main([]), (2, "", "usage: md.py FILE\n"))


# --------------------------------------------------------------------------------------
# What the converter refuses: every one names the file and the line
# --------------------------------------------------------------------------------------


class UnknownConstructs(ConverterCase):
    """`render it exactly, or refuse`: each construct the converter does not implement
    stops the conversion with the file and the line, never with a guess."""

    CASES = (
        ("setext heading", "Title\n=====\n", 2, "setext heading underline"),
        ("setext heading with dashes", "intro\n\nTitle\n-----\n", 4, "setext heading underline"),
        ("raw HTML block", "text\n\n<div>\nhi\n</div>\n", 3, "raw HTML block"),
        ("script block", "<script>alert(1)</script>\n", 1, "raw HTML block"),
        ("HTML comment block", "a\n\n<!-- hidden -->\n", 3, "raw HTML block"),
        ("a line that is one tag", "<custom-element>\n", 1, "raw HTML block"),
        ("details block", "<details>\n<summary>x</summary>\n</details>\n", 1, "raw HTML block"),
        ("HTML block after a paragraph line", "text\n<div>\n", 2, "raw HTML block"),
        ("link reference definition", "[ref]: https://example.com\n", 1, "link reference definitions"),
        ("reference link", "see [text][ref]\n", 1, "reference-style links"),
        ("image", "![alt](image.png)\n", 1, "images are not supported"),
        ("footnote", "A note[^1]\n", 1, "footnotes"),
        ("task list", "- [ ] task\n", 1, "task list items"),
        ("strikethrough", "a\n\n~~struck~~\n", 3, "strikethrough"),
        ("single-tilde strikethrough", "~struck~\n", 1, "strikethrough"),
        ("link title", '[t](https://example.com "title")\n', 1, "link titles"),
        ("lazy continuation in a list", "- item\nlazy\n", 2, "lazy continuation"),
        ("lazy continuation after a quote", "> quote\nlazy\n", 2, "lazy continuation"),
        ("tab in indentation", "text\n\n\tcode\n", 3, "tab in indentation"),
        ("tab in text", "a\tb\n", 1, "tab character in text"),
        ("unterminated fence", "```\ncode\n", 1, "never closed"),
        ("unterminated code span", "a\nb `open\n", 2, "backtick that never closes"),
        ("GitHub alert", "> [!NOTE]\n> text\n", 1, "GitHub alerts"),
        ("emoji short code", "fine :smile: text\n", 1, "emoji short code"),
        ("formula", "costs $5 and $6\n", 1, "two dollar signs"),
        ("e-mail autolink", "<someone@example.com>\n", 1, "e-mail autolinks"),
        ("bare e-mail address", "write to someone@example.com\n", 1, "bare e-mail address"),
        ("bare www address", "see www.example.com\n", 1, "bare www. address"),
        ("bare http address", "see http://example.com/ now\n", 1, "does not begin with a lower-case https://"),
        ("bare ftp address", "see ftp://example.com/ now\n", 1, "does not begin with a lower-case https://"),
        ("address in capitals", "see HTTPS://example.com/ now\n", 1, "does not begin with a lower-case https://"),
        ("address inside brackets", "[https://example.com/]\n", 1, "address inside square brackets"),
        ("address directly after punctuation", "x:https://example.com/\n", 1, "address directly after"),
        ("control character", "a\x07b\n", 1, "control or formatting character U+0007"),
        ("escape character", "ok\n\x1b[31mred\n", 2, "U+001B"),
        ("zero-width space", "zero​width\n", 1, "U+200B"),
        ("right-to-left override", "safe\n\n‮gnp.exe\n", 3, "U+202E"),
        ("byte order mark", "﻿text\n", 1, "U+FEFF"),
        ("soft hyphen", "soft­hyphen\n", 1, "U+00AD"),
        ("private-use character", "ab\n", 1, "U+E000"),
        ("reference to a control character", "&#x202e; override\n", 1, "names a control or formatting character"),
        ("line separator", "one\ntwo\u2028three\n", 2, "line or paragraph separator U+2028"),
        ("paragraph separator", "one\u2029two\n", 1, "line or paragraph separator U+2029"),
        ("reference to a line separator", "a\n\nb &#x2028; c\n", 3, "names a line or paragraph separator U+2028"),
    )

    def test_each_is_refused_with_file_and_line(self):
        for name, markdown, line, part in self.CASES:
            with self.subTest(construct=name):
                self.assertRefused(markdown, line, part)

    def test_the_error_reads_file_line_reason(self):
        with self.assertRaises(md.MarkdownError) as caught:
            md.to_html("# Fine\n\nTitle\n=====\n", source="docs/EXAMPLE.md")
        self.assertEqual(
            str(caught.exception),
            "docs/EXAMPLE.md:4: setext heading underline (use '#' headings; put a blank line before a '---' rule)",
        )

    def test_without_a_file_name_the_error_still_has_the_line(self):
        with self.assertRaises(md.MarkdownError) as caught:
            md.to_html("ok\n\n![x](y.png)\n")
        self.assertEqual(str(caught.exception), "<markdown>:3: images are not supported")


# --------------------------------------------------------------------------------------
# Hostile input
# --------------------------------------------------------------------------------------


class Hostile(ConverterCase):
    def test_html_in_text_is_shown_as_text(self):
        self.assertRenders("Hello <script>alert(1)</script> world\n",
                           "<p>Hello &lt;script&gt;alert(1)&lt;/script&gt; world</p>\n")
        self.assertRenders("Text <img src=x onerror=alert(1)> more\n", "<p>Text &lt;img src=x onerror=alert(1)&gt; more</p>\n")
        self.assertRenders('a <a href="javascript:alert(1)">click</a> b\n',
                           '<p>a &lt;a href="javascript:alert(1)"&gt;click&lt;/a&gt; b</p>\n')
        self.assertRenders("x <svg/onload=alert(1)> y\n", "<p>x &lt;svg/onload=alert(1)&gt; y</p>\n")

    def test_an_html_comment_is_left_out_as_github_leaves_it_out(self):
        # GitHub's renderer passes a comment on as raw HTML and its sanitiser removes it, so
        # a reader on GitHub sees nothing of it: README.md marks numbers that way
        # (`576 <!-- COUNTS --> tests`). Nothing of it is written, and it is no part of the
        # text a reader sees (plain_text, and with it {{REQUIRE}} and slugs).
        self.assertRenders("text <!-- comment --> text\n", "<p>text  text</p>\n")
        self.assertRenders("576 <!-- COUNTS --> tests, round <!-- COUNTS -->, after\n",
                           "<p>576  tests, round , after</p>\n")
        self.assertRenders("a <!-- one\nline -- no, two\nlines --> b\n",
                           "<p>a &lt;!-- one\nline -- no, two\nlines --&gt; b</p>\n")  # (`--` inside: no comment)
        self.assertRenders("a <!-- over\ntwo lines --> b\n", "<p>a  b</p>\n")
        self.assertRenders("**What \"5 <!-- C --> tests\" counts.** x\n", '<p><strong>What "5  tests" counts.</strong> x</p>\n')
        self.assertIn('<td role="cell" data-label="a">1 </td><td role="cell" data-label="b">2</td>',
                      md.to_html("| a | b |\n|---|---|\n| 1 <!-- C --> | 2 |\n", source=SOURCE))
        self.assertRenders("- one <!-- x --> item\n", "<ul>\n<li>one  item</li>\n</ul>\n")
        self.assertRenders("x <!----> y\n", "<p>x  y</p>\n")
        inlines = md.parse("576 <!-- COUNTS --> tests\n")[0].inlines
        self.assertEqual(md.plain_text(inlines), "576  tests")

    def test_what_is_no_comment_by_githubs_rule_stays_text(self):
        # cmark-gfm 0.29: the text of a comment does not start with `>` or `->`, does not end
        # with `-` and does not contain `--`. Anything else stays the text it is.
        for raw in ("<!-->", "<!--->", "<!--->x -->", "<!-- a -- b -->", "<!-- a--->", "<!-->x -->"):
            with self.subTest(raw=raw):
                self.assertRenders("a %s b\n" % raw, "<p>a %s b</p>\n" % md.escape(raw, quote=False))

    def test_a_comment_in_a_heading_is_refused(self):
        # (its anchor on GitHub was not asked)
        self.assertRefused("## Title <!-- x -->\n", 1, "raw HTML in the heading")

    def test_markdown_inside_html_in_text_is_not_applied(self):
        # Otherwise `<b title="*">` and a later `*` would become emphasis around a tag.
        self.assertRenders('<b title="*x">*y*\n', '<p>&lt;b title="*x"&gt;<em>y</em></p>\n')

    def test_html_written_with_character_references_stays_text(self):
        self.assertRenders("&lt;script&gt;alert(1)&lt;/script&gt; &#60;b&#62; &#x3c;i&#x3e;\n",
                           "<p>&lt;script&gt;alert(1)&lt;/script&gt; &lt;b&gt; &lt;i&gt;</p>\n")

    def test_a_javascript_link_is_refused(self):
        for target in ("javascript:alert(1)", "JaVaScRiPt:alert(1)", "javascript://example.com/%0Aalert(1)",
                       "vbscript:msgbox(1)", "data:text/html;base64,PHNjcmlwdD4=", "file:///etc/passwd"):
            with self.subTest(target=target):
                self.assertRefused("intro\n\n[click](%s)\n" % target, 3, "only https:// addresses are allowed")
        self.assertRefused("<javascript:alert(1)>\n", 1, "refused autolink")
        self.assertRefused("[click](&#106;avascript:alert(1))\n", 1, "refused link target")

    def test_a_link_cannot_leave_through_a_disguised_address(self):
        self.assertRefused("[x](https://good.example@evil.example/)\n", 1, "not a plain https address")
        self.assertRefused("[x](https://example.com:8443/)\n", 1, "not a plain https address")
        self.assertRefused("[x](https://exaмple.com/)\n", 1, "not a plain https address")
        self.assertRefused("[x](//evil.example/)\n", 1, "only relative paths, fragments and https addresses")
        self.assertRefused("[x](/absolute)\n", 1, "only relative paths, fragments and https addresses")
        self.assertRefused("[x](http://example.com/)\n", 1, "only https:// addresses are allowed")
        self.assertRefused("<http://example.com/>\n", 1, "only https:// addresses are allowed")

    def test_an_address_cannot_close_its_attribute(self):
        self.assertRefused('[x](https://example.com/"onmouseover="alert(1))\n', 1, "the address contains '\"'")
        self.assertRefused("[x](https://example.com/'onmouseover='alert(1))\n", 1, "the address contains")
        self.assertRefused('[x](docs/a"b)\n', 1, "not a plain relative path")
        self.assertRefused('[x](#a"b)\n', 1, "unusable fragment")
        self.assertRefused("[x](https://example.com/?q=&quot;x)\n", 1, "character reference")
        self.assertRefused('<https://example.com/"x>\n', 1, "the address contains")

    def test_text_cannot_close_the_attributes_the_converter_writes(self):
        # A heading's text goes into aria-label, a header cell's into data-label.
        html_text = md.to_html('## A "quoted" &lt;b&gt; & heading\n', heading_anchors=True)
        self.assertIn('aria-label="Permalink: A &quot;quoted&quot; &lt;b&gt; &amp; heading"', html_text)
        self.assertEqual(foreign_markup(html_text), [])
        html_text = md.to_html('| a "b" <i> | c |\n|---|---|\n| 1 | 2 |\n')
        self.assertIn('data-label="a &quot;b&quot; &lt;i&gt;"', html_text)
        self.assertEqual(foreign_markup(html_text), [])

    def test_text_that_reads_differently_from_what_it_says_is_refused(self):
        self.assertRefused("see ‮gnp.exe\n", 1, "RIGHT-TO-LEFT OVERRIDE")
        self.assertRefused("```\nif admin⁧ then\n```\n", 2, "U+2067")
        self.assertRefused("`a‍b`\n", 1, "U+200D")

    def test_every_hostile_piece_in_every_place_is_refused_or_stays_text(self):
        pieces = (
            "<script>alert(1)</script>",
            "<img src=x onerror=alert(1)>",
            '"><script>alert(1)</script>',
            "'><svg onload=alert(1)>",
            "</code></pre><script>alert(1)</script>",
            "</td></tr></table><script>x</script>",
            "<!--",
            "-->",
            "<![CDATA[x]]>",
            "<?php x ?>",
            "&lt;script&gt;",
            "&#60;script&#62;",
            "javascript:alert(1)",
            "[x](javascript:alert(1))",
            "[x](data:text/html,<script>)",
            "<javascript:alert(1)>",
            '" onmouseover="alert(1)',
            "`<script>`",
            "{{REPO_URL}}",
            "<base href=//evil.example/>",
            "<meta http-equiv=refresh content=0;url=//evil.example/>",
            "<style>*{display:none}</style>",
            "<iframe src=//evil.example/></iframe>",
            "<a href=//evil.example/ target=_blank>x</a>",
        )
        places = (
            "%s\n",
            "text %s text\n",
            "# %s\n",
            "## a %s b\n",
            "- %s\n",
            "1. item\n\n   %s\n",
            "> %s\n",
            "| a | b |\n|---|---|\n| %s | x |\n",
            "| %s | b |\n|---|---|\n| 1 | 2 |\n",
            "*%s*\n",
            "**%s**\n",
            "[%s](#x)\n",
            "[text](%s)\n",
            "`%s`\n",
            "```\n%s\n```\n",
            "```%s\ncode\n```\n",
            "    %s\n",
            "https://example.com/%s\n",
            "<https://example.com/%s>\n",
        )
        rendered = refused = 0
        for piece in pieces:
            for place in places:
                markdown = place % piece
                with self.subTest(markdown=markdown):
                    for options in ({}, {"heading_anchors": True, "row_ids": True}):
                        try:
                            html_text = md.to_html(markdown, source=SOURCE, **options)
                        except md.MarkdownError as error:
                            self.assertEqual(error.source, SOURCE)
                            refused += 1
                            continue
                        rendered += 1
                        self.assertEqual(foreign_markup(html_text), [], html_text)
                        self.assertNotIn("<script", html_text.lower())
        # Both outcomes occur; if one count were zero the test would not test what it says.
        self.assertGreater(rendered, 100)
        self.assertGreater(refused, 100)

    def test_random_input_is_refused_or_rendered_but_never_crashes(self):
        pieces = [
            "*", "**", "_", "__", "`", "``", "[", "]", "(", ")", "<", ">", "!", "#", "##", "-", "+", "1.", "2)", "|", "---",
            "===", "\\", "&", "&amp;", "&#60;", "&lt;", ";", ":", "~", "~~", "\n", "\n\n", " ", "  ", "    ", "a", "b", "word",
            "https://example.com/x", "http://e.example", "www.example.com", "javascript:alert(1)", "<script>", "</script>",
            "<b>", "<!--", "-->", "```", "~~~", "> ", "x@example.com", "$", ":smile:", "[x](#y)",
            "[x](https://example.com/)", "<https://example.com/>", "| a | b |", "|---|---|", '"', "'", "=", "é",
            "→", "onclick=", "(#a)", "](", "![", "[^1]", "- [ ] ", "\t",
        ]
        generator = random.Random(20261113)
        rendered = refused = 0
        for _ in range(4000):
            markdown = "".join(generator.choice(pieces) for _ in range(generator.randint(1, 14)))
            try:
                html_text = md.to_html(markdown, source=SOURCE, row_ids=True)
            except md.MarkdownError as error:
                self.assertEqual(error.source, SOURCE, markdown)
                refused += 1
                continue
            # (any other exception fails the test: the converter must not crash)
            rendered += 1
            self.assertEqual(foreign_markup(html_text), [], "%r -> %r" % (markdown, html_text))
        self.assertGreater(rendered, 500)
        self.assertGreater(refused, 500)


# --------------------------------------------------------------------------------------
# What the review of the converter found (2026-11-14): each case renders as GitHub renders it
# (its Markdown API's answer is quoted where there is one), or is refused
# --------------------------------------------------------------------------------------


class FencesPairAsOnGitHub(ConverterCase):
    """renderer-1: a tab after a closing fence left the block open, and the next fence closed it."""

    def test_spaces_and_tabs_may_follow_a_closing_fence(self):
        self.assertRenders("```\nP13 code\n```\t\nP14 after\n",
                           '<pre tabindex="0"><code>P13 code\n</code></pre>\n<p>P14 after</p>\n')  # (probe1)
        self.assertRenders("~~~sh\na\n~~~ \t \nb\n", '<pre tabindex="0"><code class="language-sh">a\n</code></pre>\n<p>b</p>\n')
        self.assertRenders(
            "Remove the caches:\n\n```sh\nrm -rf ~/.cache/x\n```\t\n\nDo not run the next one:\n\n```sh\nrm -rf ~/x\n```\n",
            '<p>Remove the caches:</p>\n<pre tabindex="0"><code class="language-sh">rm -rf ~/.cache/x\n</code></pre>\n'
            '<p>Do not run the next one:</p>\n<pre tabindex="0"><code class="language-sh">rm -rf ~/x\n</code></pre>\n',
        )

    def test_a_line_inside_a_block_that_begins_like_its_fence_is_refused(self):
        # GitHub keeps such a line as code (probe5 F1), but it is always a fence that lost
        # its closing line or a closing fence with something after it: the fences after it
        # would pair up wrongly, and the error now names that line.
        for markdown, line in (
            ("```\nF1 inside\n```sh\nF1 still inside\n```\n", 3),
            ("intro\n\n```\na\n``` x\nb\n```\n", 5),
            ("```\na\n``` \nb\n```\n", 3),
            ("````\na\n   `````x\n````\n", 3),
            ("- item\n\n  ```\n  a\n  ```sh\n  ```\n", 5),
        ):
            with self.subTest(markdown=markdown):
                self.assertRefused(markdown, line, "begins like a code fence but does not close the block")
        # A shorter run, the other character or four spaces of indentation are content.
        self.assertRenders("````\n```\n````\n", '<pre tabindex="0"><code>```\n</code></pre>\n')
        self.assertRenders("```\n~~~x\n```\n", '<pre tabindex="0"><code>~~~x\n</code></pre>\n')
        self.assertRenders("```\n    ```x\n```\n", '<pre tabindex="0"><code>    ```x\n</code></pre>\n')


class EmphasisAsOnGitHub(ConverterCase):
    """renderer-2: which runs can open or close, and which opener a closer finds, as GitHub's
    renderer (cmark-gfm 0.29) decides."""

    def test_symbols_beside_a_run_are_not_punctuation(self):
        # CommonMark 0.31 counts symbols as punctuation; GitHub does not (probe1 P01).
        self.assertRenders(
            "P01 *£*bravo. a*→b* x**→** 5**×**3 x**✓ done** *$*alpha.\n",
            "<p>P01 <em>£</em>bravo. a<em>→b</em> x<strong>→</strong> 5<strong>×</strong>3 x<strong>✓ done</strong> "
            "*$*alpha.</p>\n",
        )

    def test_punctuation_and_white_space_outside_ascii(self):
        # (probe5 E1: U+00A0 is white space; ¡ « » ‿ are punctuation)
        self.assertRenders(
            "E1 *x* *y* and a *b* c and ¡*d*¡ and «*e*» and ‿*f*‿\n",
            "<p>E1 <em>x</em> <em>y</em> and a <em>b</em> c and ¡<em>d</em>¡ and «<em>e</em>» and "
            "‿<em>f</em>‿</p>\n",
        )

    def test_the_tables_copied_from_githubs_renderer(self):
        for char in ("!", "~", "¡", "§", "«", "‿", "—", "᙭"):  # (U+166D: a symbol today, punctuation there)
            with self.subTest(char=char):
                self.assertTrue(md._is_punctuation(char))
        for char in ("a", "£", "€", "→", "×", "✓", "©", "⹐", ""):  # (U+2E50: punctuation added since)
            with self.subTest(char=char):
                self.assertFalse(md._is_punctuation(char))
        for char in ("", " ", "\t", "\n", " ", " ", "　"):
            with self.subTest(char=char):
                self.assertTrue(md._is_whitespace(char))
        for char in ("a", " ", " ", "​", "\x0b"):
            with self.subTest(char=char):
                self.assertFalse(md._is_whitespace(char))

    def test_the_bound_of_the_search_for_an_opener_is_githubs(self):
        # A closer that found no opener stops later closers of its character and length
        # modulo 3 from looking further back; CommonMark 0.30 changed that, GitHub did not
        # (probe5 H1; CommonMark 0.31 gives "*<em>a x<em>y</em> b</em>").
        self.assertRenders("H1 **a x*y* b*\n", "<p>H1 **a x<em>y</em> b*</p>\n")

    def test_tildes_take_part_as_githubs_strikethrough_extension_lets_them(self):
        # A ~ pair of different lengths takes the * between them out of play (probe5 H2).
        self.assertRenders("H2 ~a *b~~ c*\n", "<p>H2 ~a *b~~ c*</p>\n")
        # A ~ beside a * or _ is stepped over when deciding what the run can do (probe5 H3;
        # probe6 K1 and probe7 L7: read as punctuation, the ~ would make `_b_` emphasis).
        self.assertRenders("H3 a~*b*~c\n", "<p>H3 a~<em>b</em>~c</p>\n")
        self.assertRenders("L7 a~_b_~c\n", "<p>L7 a~_b_~c</p>\n")
        self.assertRenders("M1 a~_b_ c\n", "<p>M1 a~_b_ c</p>\n")  # (probe8: the ~ before the run)
        self.assertRenders("M2 a _b_~c\n", "<p>M2 a _b_~c</p>\n")  # (probe8: the ~ after the run)
        self.assertRenders("M3 a~*b* c and x *b*~c\n", "<p>M3 a~<em>b</em> c and x <em>b</em>~c</p>\n")
        # Where GitHub draws strikethrough, the converter refuses, at the closing run.
        self.assertRefused("H3 and\n~*d*~\n", 2, "strikethrough")
        self.assertRefused("P08 ~one~ and ~~two~~\n", 1, "strikethrough")
        # Elsewhere tildes are text (probe3 R5, probe7 L3).
        self.assertRenders("about ~120 MB, ~ and ~~ x\n", "<p>about ~120 MB, ~ and ~~ x</p>\n")
        self.assertRenders("cp ~/Downloads/a-*.jar ~/.local/b and x ~/c\n", "<p>cp ~/Downloads/a-*.jar ~/.local/b and x ~/c</p>\n")


class TablesAsOnGitHub(ConverterCase):
    """renderer-3 and renderer-4: where a table begins and ends, and where a cell ends."""

    def test_a_row_indented_by_four_spaces_is_refused(self):
        # GitHub ends the table and shows the line as code (probe1 P16).
        self.assertRefused("| c | d |\n|---|---|\n| P15 | x |\n    | P16 indented | y |\n", 4,
                           "indented by four or more spaces directly below a table")
        self.assertIn('<td role="cell" data-label="d1">D1 three</td>',
                      md.to_html("| d1 | d2 |\n|---|---|\n| 1 | 2 |\n   | D1 three | spaces |\n"))  # (probe5 D1)

    def test_a_list_item_of_any_number_ends_a_table(self):
        # (probe3: `<ol start="2"><li>x | r4</li></ol>`; probe5 C1 with `2)`)
        for line, start in (("2. x | r4", '<ol start="2">'), ("2) x | r4", '<ol start="2">'), ("- x | r4", "<ul>")):
            with self.subTest(line=line):
                html_text = md.to_html("| a | b |\n|---|---|\n| 1 | 2 |\n%s\n" % line)
                self.assertIn("</table>\n</div>\n%s\n<li>x | r4</li>" % start, html_text)

    def test_rows_of_dashes_that_github_does_not_take_for_one(self):
        self.assertRefused("q3h | q3h2\n- | -\nq3a | q3b\n", 2, "begins with '- ': GitHub reads it as a list item")  # (probe2 q3)
        self.assertRefused("T1 h1 | h2\n    |---|---|\n", 2, "row of dashes indented by four or more spaces")  # (probe4 T1)
        self.assertIn("<table", md.to_html("a | b\n   --- | ---\n"))  # (probe7 L4a)

    def test_a_row_of_dashes_with_a_colon_and_no_pipe_is_refused(self):
        # GitHub makes a table of one column of the line above it (probe5 A1, A2).
        for markdown in ("A1 head\n:--\n", "A2 x\n--:\nA2 row\n", "x\n:-:\n", "first\nsecond\n  :---\n"):
            with self.subTest(markdown=markdown):
                self.assertRefused(markdown, markdown.count("\n", 0, markdown.rindex(":")) + 1, "a table of one column")
        self.assertRenders("x\n    :--\n", "<p>x\n:--</p>\n")  # (indented four spaces: the paragraph's; probe7 L5)
        self.assertRenders("x\n\n:--\n", "<p>x</p>\n<p>:--</p>\n")  # (probe7 L6)

    def test_a_pipe_after_a_backslash_never_ends_a_cell(self):
        # cmark-gfm reads every `\|` as a pipe inside the cell, whatever stands before the
        # backslash, and takes one backslash from it (probe4 S1, S3; probe5 e1).
        self.assertIn('<td role="cell" data-label="a">x|y</td><td role="cell" data-label="b">z</td>',
                      md.to_html("| a | b |\n|---|---|\n| x\\\\|y | z |\n"))
        self.assertIn('<td role="cell" data-label="a"><code>a\\|b</code></td>', md.to_html("| a | b |\n|---|---|\n| `a\\\\|b` | z |\n"))
        # A short row no longer shifts the cells silently: it is a row one cell short.
        self.assertRefused("| a | b | c |\n|---|---|---|\n| S2 x\\\\|y | z |\n", 3, "table row has 2 cells, its header has 3")

    def test_split_row(self):
        self.assertEqual(md.split_row("| a | b |"), ["a", "b"])
        self.assertEqual(md.split_row("a | b"), ["a", "b"])
        self.assertEqual(md.split_row("| a |  |"), ["a", ""])
        self.assertEqual(md.split_row("| a \\| b | `c \\| d` |"), ["a | b", "`c | d`"])
        self.assertEqual(md.split_row("| x\\\\|y | z |"), ["x\\|y", "z"])
        self.assertEqual(md.split_row("| x\\\\|y | z |", keep_escapes=True), ["x\\\\|y", "z"])
        self.assertEqual(md.split_row("| a | b \\|"), ["a", "b |"])
        self.assertEqual(md.split_row("|   | r9b  |"), [" ", "r9b "])
        self.assertEqual(md.split_row("|"), [])


class ListsAndRulesAsOnGitHub(ConverterCase):
    """renderer-5: a thematic break and the list items around it."""

    def test_a_thematic_break_takes_precedence_over_a_list_item(self):
        # (probe2 q2 and q2c, probe3 r8, probe4 X1)
        for markdown, items in (
            ("- q2a\n- q2b\n-  --\n", ["q2a", "q2b"]),
            ("- q2c\n- ----\n", ["q2c"]),
            ("* r8a\n* * * *\n", ["r8a"]),
            ("- X1\n- - -\n", ["X1"]),
        ):
            with self.subTest(markdown=markdown):
                self.assertRenders(markdown + "\nafter\n",
                                   "<ul>\n%s\n</ul>\n<hr>\n<p>after</p>\n" % "\n".join("<li>%s</li>" % item for item in items))
        self.assertRenders("1. a\n2. ---\n", "<ol>\n<li>a</li>\n<li><hr></li>\n</ol>\n")  # (an item that holds a rule; probe6 K3)

    def test_a_blank_line_after_a_thematic_break_keeps_a_list_tight(self):
        # GitHub's renderer keeps the rule open until the next line, so a blank line after
        # it separates nothing (probe2 q1c, probe5 B1 and B2).
        self.assertRenders("- * * *\n\n- q1c\n", "<ul>\n<li><hr></li>\n<li>q1c</li>\n</ul>\n")
        self.assertRenders("- B1\n- * * *\n\n\n- B1 next\n", "<ul>\n<li>B1</li>\n<li><hr></li>\n<li>B1 next</li>\n</ul>\n")
        self.assertRenders("- B2\n  - * * *\n\n- B2 next\n",
                           "<ul>\n<li>B2\n<ul>\n<li><hr></li>\n</ul></li>\n<li>B2 next</li>\n</ul>\n")
        self.assertRenders("- * * *\n\n  text\n- b\n", "<ul>\n<li><hr>\ntext</li>\n<li>b</li>\n</ul>\n")  # (probe7 L1)
        # A heading does not continue over a blank line: there the list is loose (probe4 W1).
        self.assertRenders("- W1\n- # heading in item\n\n- W2\n",
                           '<ul>\n<li>\n<p>W1</p>\n</li>\n<li>\n<h1 id="heading-in-item">heading in item</h1>\n</li>\n'
                           "<li>\n<p>W2</p>\n</li>\n</ul>\n")


class WhiteSpaceAsOnGitHub(ConverterCase):
    """renderer-6 and renderer-7."""

    def test_only_spaces_and_tabs_make_a_blank_line(self):
        self.assertRenders("P12 a\n \nb\n", "<p>P12 a\n \nb</p>\n")  # (probe1 P12)
        self.assertRenders("G1 a\n \t \nG1 b\n", "<p>G1 a</p>\n<p>G1 b</p>\n")  # (probe5 G1)

    def test_cells_lose_spaces_and_tabs_only(self):
        self.assertIn('<td role="cell" data-label="r9"> </td><td role="cell" data-label="x">r9b </td>',
                      md.to_html("| r9 | x |\n|---|---|\n|   | r9b  |\n"))  # (probe3 r9)

    def test_a_heading_keeps_white_space_that_is_not_a_space(self):
        self.assertRenders("## R10 trailing \n", '<h2 id="r10-trailing">R10 trailing </h2>\n')  # (probe3 R10)

    def test_a_space_written_as_a_reference_does_not_break_the_line(self):
        # Only spaces written as spaces count for a hard line break (probe1 P11).
        self.assertRenders("P11 foo&#32;&#32;\nbar\n", "<p>P11 foo  \nbar</p>\n")
        self.assertRenders("foo&#32;  \nbar\n", "<p>foo <br>\nbar</p>\n")  # (probe6 K6)
        self.assertRenders("foo  &#32;\nbar\n", "<p>foo   \nbar</p>\n")  # (probe7 L2)


class LimitsAndLines(ConverterCase):
    """renderer-9 and renderer-10."""

    def test_deep_nesting_is_refused_with_its_line(self):
        self.assertRefused("> " * 33 + "x\n", 1, "nested more than 32 deep")
        self.assertRefused("text\n\n" + "> " * 600 + "x\n", 3, "nested more than 32 deep")
        self.assertRenders("> " * 32 + "x\n", "<blockquote>\n" * 32 + "<p>x</p>" + "\n</blockquote>" * 32 + "\n")
        nested = "".join("  " * level + "- x\n" for level in range(600))
        self.assertRefused(nested, 33, "nested more than 32 deep")
        def emphasis(depth):  # *w _w *w … w* w_ w*: emphasis inside emphasis, `depth` deep
            marks = ["*_"[level % 2] for level in range(depth)]
            return " ".join(mark + "w" for mark in marks) + " " + " ".join("w" + mark for mark in reversed(marks)) + "\n"

        self.assertEqual(md.to_html(emphasis(32)).count("<em>"), 32)
        self.assertRefused(emphasis(33), 1, "emphasis and links nested more than 32 deep")
        self.assertRefused("x\n" + "[" * 40 + "a" + "](#b)" * 40 + "\n", 2, "")

    def test_long_paragraphs_take_time_in_proportion(self):
        # The review timed the first three at 2.7 s to 5.2 s for 8000 repetitions, sixteen
        # times as long for four times the input; the next ones were quadratic as well. A
        # paragraph of 160 000 characters of each takes a fraction of a second now, and at
        # least ten seconds with any of those paths quadratic again: the bound is generous
        # either way.
        for piece in ("a* ", "[", "*a_", "a", "a.", ":a", "x <!-- ", "x <? ", "x <!a ", "[a](#b) ", "&amp;"):
            with self.subTest(piece=piece):
                started = time.perf_counter()
                try:
                    md.to_html(piece * (160000 // len(piece)) + "\n")
                except md.MarkdownError:
                    pass
                self.assertLess(time.perf_counter() - started, 5.0)

    def test_raw_html_that_never_closes_is_text(self):
        # (probe6 K5: GitHub shows the opener as text, and emphasis after it applies)
        self.assertRenders("K5 x <!-- open *a* and <? pi *b*\n",
                           "<p>K5 x &lt;!-- open <em>a</em> and &lt;? pi <em>b</em></p>\n")
        self.assertRenders("a <!-- c --> *b*\n", "<p>a  <em>b</em></p>\n")  # (a comment is left out)

    def test_a_refusal_inside_a_paragraph_names_the_line_of_the_problem(self):
        self.assertRefused("line one\nline two\nwrite to user@example.com now\n", 3, "bare e-mail address")
        self.assertRefused("first line\nsecond line costs $5\nthird line costs $6\n", 2,
                           "two dollar signs in one piece of text (GitHub may typeset what is between them as a formula; "
                           "put them in code spans); the second is on line 3")
        self.assertRefused("one\ntwo :smile: three\n", 2, "emoji short code")
        self.assertRefused("| a |\n|---|\n| fine |\n| :smile: |\n", 4, "emoji short code")
        self.assertRefused("# T\n\ntext\rmore\n", 3, "carriage return without a line feed")


class AnchorsAsOnGitHub(ConverterCase):
    """renderer-8, as far as the converter decides it (the title and the page around a
    document are the generator's; test_build.py covers them)."""

    def test_headings_are_numbered_among_headings_only(self):
        # (probe1 and probe2: foo, foo-1, foo-1-1; zeta-1, zeta, zeta-2)
        self.assertRenders("## Foo\n\n## Foo\n\n## Foo-1\n",
                           '<h2 id="foo">Foo</h2>\n<h2 id="foo-1">Foo</h2>\n<h2 id="foo-1-1">Foo-1</h2>\n')
        self.assertRenders("## Zeta-1\n\n## Zeta\n\n## Zeta\n",
                           '<h2 id="zeta-1">Zeta-1</h2>\n<h2 id="zeta">Zeta</h2>\n<h2 id="zeta-2">Zeta</h2>\n')

    def test_the_title_counts_as_the_first_heading(self):
        # (probe1: "# Alpha" then "## Alpha" gives alpha and alpha-1)
        renderer = md.Renderer()
        blocks = md.parse("# Alpha\n\n## Alpha\n")
        self.assertEqual(renderer.reserve_title(blocks[0]), "alpha")
        self.assertEqual(renderer.blocks(blocks[1:]), '<h2 id="alpha-1">Alpha</h2>')

    def test_a_row_and_a_heading_that_want_one_id_are_refused_in_either_order(self):
        self.assertRefused("| # | W |\n|---|---|\n| D1 | one |\n\n## Row D1\n", 5, "which the table row at line 3 has",
                           row_ids=True)
        self.assertRefused("## Row D1\n\n| # | W |\n|---|---|\n| D1 | one |\n", 5, "which a heading has as its anchor",
                           row_ids=True)
        self.assertRenders("## Row D1\n\n| # | W |\n|---|---|\n| D1 | one |\n",
                           md.to_html("## Row D1\n\n| # | W |\n|---|---|\n| D1 | one |\n"))  # (no row ids: no clash)

    def test_an_id_the_page_around_uses_is_refused(self):
        self.assertRefused("text\n\n## Site content\n", 3, "which the page around the document uses",
                           reserved_ids={"site-content"})
        self.assertRefused("| # | W |\n|---|---|\n| X1 | y |\n", 3, "which the page around the document uses",
                           row_ids=True, reserved_ids={"row-x1"})

    def test_raw_html_in_a_heading_is_refused(self):
        # GitHub renders the tag and leaves it out of the anchor (probe4 V1: v1-key-name).
        self.assertRefused("## V1 <kbd>Key</kbd> name\n", 1, "raw HTML in the heading")
        self.assertRenders("## V1 `<kbd>` name\n", '<h2 id="v1-kbd-name">V1 <code>&lt;kbd&gt;</code> name</h2>\n')  # (probe7)

    def test_a_document_may_show_two_braces(self):
        # (renderer-11: the output never holds two braces in a row, so that the generator's
        # check for placeholders is not misled; a reader sees the same characters)
        self.assertRenders("Write `{{SITE_URL}}`, {{x}} and {{{y}}}.\n",
                           "<p>Write <code>&#123;{SITE_URL}}</code>, &#123;{x}} and &#123;&#123;{y}}}.</p>\n")
        self.assertEqual(md.escape('{{a}} "b"'), "&#123;{a}} &quot;b&quot;")


# --------------------------------------------------------------------------------------
# The documents themselves
# --------------------------------------------------------------------------------------


class TheDocuments(unittest.TestCase):
    """The documents the site renders (the list is DOCUMENTS in site/build.py; test_build.py
    reads it, this module does not import the generator)."""

    ROOT = HERE.parent
    DOCUMENTS = ("README.md", "docs/THREAT_MODEL.md", "docs/TOKEN_INJECTION.md", "docs/MC_VAULT_ACCOUNT.md",
                 "docs/MODS.md", "docs/COMPANION.md", "docs/PHONE.md", "docs/SCRAM_RUNBOOK.md")

    def test_they_convert_and_contain_only_the_converters_own_markup(self):
        for name in self.DOCUMENTS:
            with self.subTest(document=name):
                text = (self.ROOT / name).read_text(encoding="utf-8")
                html_text = md.to_html(text, source=name, heading_anchors=True, row_ids=True)
                self.assertEqual(foreign_markup(html_text), [])
                self.assertEqual(html_text.count("<h1 "), 1)
                # (README.md marks numbers with comments, <!-- COUNTS -->: GitHub shows none)
                self.assertNotIn("&lt;!--", html_text)

    def test_the_runbook_keeps_every_address_exactly(self):
        text = (self.ROOT / "docs/SCRAM_RUNBOOK.md").read_text(encoding="utf-8")
        renderer = md.Renderer(source="docs/SCRAM_RUNBOOK.md")
        renderer.blocks(md.parse(text, "docs/SCRAM_RUNBOOK.md"))
        self.assertGreater(len(renderer.outbound), 0)
        for address in renderer.outbound:
            self.assertIn(address, text)
            self.assertIsNone(md.check_https_url(address))


if __name__ == "__main__":
    unittest.main()
