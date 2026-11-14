# Website: hand-over file

State: **closed, then brought up to the documents of the third and fourth rounds
(2026-11-14); never published.** The site builds from the repository's own documents,
passes its own checks as a preview and as deployed, was reviewed through three lenses,
every finding was fixed or handed to the owner of the file it concerns, and every page was
looked at again at five widths. After it was merged into the tree of the fourth round it
was brought up to that tree's documents (the section "Rounds 3 and 4" below). Publishing
is the owner's step (`docs/notes/website.md`, section 3); what is still open is listed
under "Open" below.

This file folds together the hand-over files of the team that built the site on
2026-11-13, of the integrator who made the parts one site (2026-11-13, 23:20, to
2026-11-14), of the three fixers who acted on the review (2026-11-14, 01:06 to 02:20) and
of the engineer who closed the website after them (2026-11-14, from 02:22), and of the
engineer who brought it up to the documents of the third and fourth rounds after the merge
(2026-11-14, from 03:11, in the clone `target/agents/r4-site`). The notes for
the user (how to preview and publish, what the workflow does, the palette, what to
refresh, what was found in the documents, a short account of the review) are in
`docs/notes/website.md`. The team's working agreement is `site/CONTRACT.md`; every request
file and every per-role and per-area hand-over file was folded in here and deleted once
nothing in it was open.

Who built what: before the team, two engineers wrote the strict converter
(`site/md.py`) and the first generator (`site/build.py`). Then five roles worked side by
side in one working copy, each restricted to its own files: **engine** (generator,
converter, their unit tests, `tests/site.rs`), **design** (style sheet, mark, the two
drawings of the layers, page shells, footers), **landing** (the overview), **pages**
(getting started, the security overview, the error page) and **deploy** (the workflow,
the screenshot script and pictures, `robots.txt`, `.gitignore`, `docs/notes/website.md`).
Each role had two to four engineers in turn (sessions were cut off or moved to another
model; nothing was started over). Then one **integrator** took the whole website; three
reviewers looked at it through three lenses (renderer, accuracy, web); three **fixers**
acted on the findings side by side, each in an area of files (engine: `site/md.py`,
`site/build.py`, their tests and `tests/site.rs`; design: the style sheet, the drawings and
the shells; content: the pages, the workflow, the screenshot script and
`docs/notes/website.md`); and last one engineer **closed** the website alone in the
working copy.

## Rounds 3 and 4: the site brought up to the documents (2026-11-14)

The site was merged (`694c81c`) into a tree with two later rounds of features and
documents. The build then stopped on 15 `{{REQUIRE}}` phrases that had left `README.md` or
`docs/THREAT_MODEL.md`, and `--check` named 9 problems that followed (three pages not
built, six published files unused). `docs/notes/website.md`, section 11, is the account
for the owner; this is the detail.

### Run on the final tree of this pass

- `python3 -I site/build.py --check --out target/r4site/check13`: passes; 12 pages, 916
  internal links, 78 outbound links, two runs byte-identical, no `note:`. The same with
  `--repo-url https://github.com/example-owner/example-repo --site-url
  https://example-owner.github.io/example-repo/`: passes, the same counts.
- `python3 -I -B -m unittest discover -s site -p "test_*.py"`: 361 tests (139 of the
  converter, 222 of the generator), OK, in about 13 s.
- `cargo test --test site`: 5 passed. `cargo fmt --all --check`: nothing to change (the one
  Rust file touched is `tests/site.rs`).
- `cargo test -p bastion-mc-gui` with the four XDG directories, the runtime directory and
  the display variables pointed away from the user's session: 126 passed (120 unit, 4
  emulator, 2 live, which are opt-in and returned at once); then `site/update-screenshots.sh`:
  three pictures unchanged, `shot-security.png` updated (looked at; its `alt` text adapted).
- Every changed page (the overview, getting started, the security overview, the error page,
  the three new pages, the manual's status table) rendered with headless Firefox 159.0.1 in
  bubblewrap without a network and with a throw-away profile under `target/r4site/shots/`,
  at 360, 768 and 1280 px, laid out as sheets and looked at. The manual and the threat model
  are taller at 360 px than Firefox can draw in one picture ("Failed to allocate a surface
  due to invalid size"); they were rendered in parts (`target/r4site/tools/split.py`).
- Sideways overflow measured on all twelve pages at 44 widths (320, 360 to 1920 in steps
  of 40, and 736, 992, 1056): 528 measurements, nothing wider than its window
  (`target/r4site/tools/measure.sh`; it catches an element made too wide on purpose, and it
  measures less than the integrator's tool did: nothing about panels, clipping or overlap).
- Not repeated: the simulation of the workflow's build job and the lookup of the action
  pins (the deploy role's tools were not in this clone; nothing was fetched).

### The 15 ties that failed

| Page | Phrase that had left the document | Outcome |
|---|---|---|
| index | "7.0 lets the launcher confine itself" (README) | rewritten: self-confinement needs Landlock ABI 8 for files and network and ABI 9 for all of it, partial and said so on older kernels |
| getting started | "7.0 lets the launcher confine itself, 7.2 adds UDP rules" (README) | rewritten: the same, with ABI 10 (Linux 7.2) for UDP |
| getting started | "cargo build --release # or: cd packaging/arch && makepkg -si" (README) | rewritten: one way to build (cargo); the PKGBUILD is a template that does not build the checkout |
| getting started | "the window run on the X11 session; a real demo launch driven through its backend worker" (README) | rewritten: the window started and closed on a private X11 display; the demo launch also through the window's own controls; no click in a real window, not on Wayland |
| security | "7.0 lets the launcher confine itself" (README) | rewritten: as on the overview, with what the launcher gives up before it reads anything from the game |
| security | "NVIDIA; Wayland; launching Java 8-era versions" (TM §7) | re-quoted: NVIDIA is still not exercised |
| security | J13's join and withheld replies | re-quoted, and the withheld encoded reply and the broker's memory budget added |
| security | J13's residual ("so it can be made to appear on that server as you…") | rewritten: the join is not tied to the server it is spent on; a mod can authorise a login as you on another server, once per game connection it opens |
| security | §5, the words between items 1 and 2 | re-quoted; point 1 restates the age vault the item now has |
| security | §5, items 2 to 3 | rewritten: a join good for a login on any server |
| security | §5, items 3 to 4 | rewritten: recorded sound as datagrams to a server whose rule names a voice port |
| security | §5, items 5 to 6 | rewritten: limits read back, the launcher's own bounds, the helpers' limits |
| security | §5, items 6 to 7 | rewritten: Realms brokered with an approval outside the game; UDP only as the voice relay (plus the launcher's DNS responder, after J10) |
| security | §5, item 10 to §6 | rewritten: items 11 to 16 restated and tied (the companion is no defence, server rules, a mod malicious as published, the phone is interface only, the browser while it runs, versions not started) |
| security | A2 ("removes every refresh token … forgets identities") | rewritten: scram also stops the token keeper, removes the Spotify token and the Realms TOTP secret, deletes the phone browser's profile, and says `COULD NOT` |

### What else changed

- **Pages.** Read in full against the README and the threat model; the claim maps below are
  the record. The overview: a second paragraph in the hero, the callout (the phone's
  services that need an account; the companion in the real game), blocks 01 to 06 with the
  mod catalog, the voice relay, Realms, the limits read back, the Wayland listing, the
  clipboard cut-off's report, `alsa`, scram's new reach, the front end's one secret, and a
  block 07 on the companion mod and the phone; the front end and "How to get it" as the
  README has them now; three more documents under "Where to read on". Getting started:
  requirements (Landlock ABI, Java from jdk packages, the phone's packages, JDK 17 for
  building), one way to build and the PKGBUILD as a template, the settings file, `alsa`, a
  section "Mods, the companion mod and the phone", the front end. The security overview:
  three assets, adversary W, the services treated like Mojang's, the network modes, the
  voice relay, what a launcher taken over still holds, `alsa`, a section on the companion
  mod and the phone, the sixteen points, scram. The error page and both footers list the
  three new pages. Links to the README's "The display" follow its new heading
  (`#display-isolation`, `#sound`).
- **Generator.** `DOCUMENTS` renders `docs/MODS.md` (`mods.html`), `docs/COMPANION.md`
  (`companion.html`) and `docs/PHONE.md` (`phone.html`); `REPAIRS` is empty; the
  manual's description follows the README's structure. Unit tests: the small site has the
  three documents, a test that their links reach each other's pages, a test that a tie runs
  over an HTML comment, the counts of pages and files that follow, and the test of the real
  documents holds that nothing is repaired.
- **Converter.** An HTML comment inside text is left out (GitHub shows none; README.md marks
  its numbers with `<!-- COUNTS -->`). Tests for each case; the test of the real documents
  covers all eight and holds the manual free of a shown comment.
- **Look.** The rule "an odd last item takes the whole row" also for a three-column grid at
  three columns (`:last-child:nth-child(3n + 1)`); `site/CONTRACT.md` says so.
- **Drawings.** Two labels corrected by hand in both (the self-confinement and the front
  end), the stacked drawing one line taller (328×1453; the pages' `img` sizes follow).
- **Notes.** `docs/notes/website.md`: sections 1, 4, 6, 7, 8, 9 brought up to date and
  section 11 added; this file.

### Decided in this pass

- **No navigation entry for the new pages.** The header holds five entries and on a phone
  stands in two rows; a sixth would make it three (or squeeze the rows the integrator
  arranged). The pages are reached from the overview, getting started, the security
  overview, the manual, the error page and every footer.
- **`docs/README.md` is not rendered** (`docs/notes/website.md`, section 1): its links are
  mostly to the engineering notes, which the site does not publish, and its pictures are
  Markdown images, which the converter refuses on purpose.
- **HTML comments are left out, not refused**: refusing them would stop the build on the
  README's markers, which the integrator fills in and must keep; showing them was not what
  GitHub shows.
- **The drawings were corrected, not redrawn.** What they say about the frames of section 3
  is still true; two labels were not, and those were changed. What the fourth round added
  to section 3 (the companion channel, the dispatcher, the helpers, the socket to the
  launcher in Fabric profiles) is said in the captions instead.
- **No picture of the Realms prompt**: the front end's fixture pairs it with a vanilla
  instance, which never gets one (`docs/notes/website.md`, section 7).
- **The player name in the pictures stays** (the owner's decision, 2026-11-14).
- **Where the README and the threat model disagree, the pages follow the threat model and
  the code**, and the disagreement goes to section 7 (Realms without a phone, clips in a
  burner session).

## Last state (closing, 2026-11-14)

Run in the working copy on the final tree (the closing commits; the last one changes only
these notes):

- `python3 site/build.py --out target/site-close --check`: passes; 9 pages, 443 internal
  links, 46 outbound links, two runs byte-identical; the two notes of the stated repairs
  of `docs/THREAT_MODEL.md` and no other note.
- The same with `--repo-url https://github.com/example-owner/example-repo --site-url
  https://example-owner.github.io/example-repo/`: passes.
- `python3 -B -m unittest discover -s site -p "test_*.py"`: 356 tests (136 of the
  converter, 220 of the generator), OK.
- `cargo fmt --all` changes nothing; `cargo clippy --workspace --all-targets`: no warning;
  `cargo test --test site`: 5 passed; `cargo test -p bastion-mc-proto`: 6 passed.
- `site/update-screenshots.sh ../a1-gui/target/gui-snapshots` (the redesigned front end's
  snapshots; the newest there were written 2026-11-13 22:28): "unchanged" for all four
  pictures, so the published pictures are that interface's newest snapshots; each was
  looked at again against its `alt` text and caption.
- The integrator's measuring tool on the final build: at 39 widths from 360 to 1920 px,
  each without and with a 12 px scrollbar, on all nine pages (702 measurements): no page
  wider than its window, nothing sticking out of the panel, cell, callout or code box it
  stands in, nothing clipped, no two pieces of text drawn over each other.
- Every page rendered with the task's Firefox recipe at 360, 480, 768, 1024 and 1280 px
  and looked at (`target/shots/close-*.png`), and the emergency page with its print rules
  applied on screen (see "The close").
- The review's own cases (`target/site-review-renderer/cases/`) run against the final
  converter: each gives GitHub's answer or a refusal with the right line (see "The
  review and the fixes").
- The four action pins looked up again through GitHub's public API: each is still the
  commit of the action's latest release.
- The deploy role's simulation of the workflow's build job
  (`target/deploy-tools/simulate.sh`) on `6259e08`, the closing commit of the site (the
  commit after it changes only these notes): the job's own scripts in an empty
  environment (356 unit tests; the build with `--check` and two example addresses), the
  artifact packed with the action's `tar` call (19 files, 1 259 520 bytes, no dot file:
  the generator's marker stays out), served below `/example-repo/` on the loopback address
  and crawled: 19 addresses answered, 542 internal references (302 with a fragment)
  resolved below the sub-path, no file unreferenced, 0 problems.

## How to check it again

    python3 site/build.py --out target/site-check --check
    python3 site/build.py --out target/site-check-abs --check \
        --repo-url https://github.com/example-owner/example-repo \
        --site-url https://example-owner.github.io/example-repo/
    python3 -B -m unittest discover -s site -p "test_*.py"
    cargo test --test site            # both builds, runbook::LINKS, bastion-mc --help, the unit tests
    cargo test -p bastion-mc-proto

To look at a page, render it with the Firefox recipe of the task (headless, in
bubblewrap, no network, a throw-away profile). Two things to know (see "Learned"): take
`loading="lazy"` out of a copy of the output first, or the pictures come out as their
`alt` text; and give a window 12 px wider than the layout you want, then cut that strip
off, because headless Firefox keeps it for a scrollbar whenever the page is taller than
the window.

## Done and verified, by part

### The whole (integrator)

- **Ran everything** in "Last state" before changing anything (23:20); all of it passed
  on the roles' hand-in state. Every request in the five request files was answered;
  the two that were left for the integrator (engine 15 and design 5: bring
  `docs/notes/website.md` up to date) are done in that file.
- **Classes against the style sheet:** every class the nine pages use is styled, except
  `note` (a `callout note` is the default callout) and the converter's `language-*` on
  code. Styled and unused today: the contract's `status`, `tested`, `partly`, `untested`
  and `readout` (the overview keeps the README's status table as it is, see
  "Decisions"), and the generator's `align-*` and `cols-4`, `cols-6` to `cols-8` for
  tables the documents do not have yet.
- **A measuring tool of its own** (`target/integrate/measure.py`, not committed): a copy
  of each page with a relaxed policy measures itself in a frame of each width and writes
  the result as a bit pattern that the harness's screenshot carries out; ffmpeg reads it.
  Shown to catch each kind of fault on a page broken on purpose (a line that does not
  wrap, text pushed over other text, text in a box that clips it).
- **The visual pass**, three rounds: every page rendered at 360, 480, 768, 1024 and 1280
  px and looked at; the changed parts again after each change; the print rules applied on
  screen at 680 px (about an A4 column) for the runbook and the overview; getting started
  without the style sheet. What was changed (all in `site/src/assets/site.css`, plus one
  line in each of `getting-started.html` and `security.html`):
  1. The header's five entries stood four above one from 30em to 36em, Emergency alone on
     the second row; now three above two up to 36em, with the low rows of a phone (the
     list is kept narrower than four entries and wider than three).
  2. Callouts were 58rem wide: lines of about 130 characters at 1280 px. They keep to the
     measure now and line up with the paragraphs and the table of contents.
  3. A rendered document was 92rem wide from 80em while the header and the footer are
     76rem: its table of contents began left of the name in the header and its column
     ended right of the navigation. Every page is 76rem now.
  4. The runbook was a 54rem column centred on its own; it now begins under the name in
     the header like every page (the column is still 54rem).
  5. Three-column grids: the first column was squeezed to a word per line beside a column
     of long code ("The / display"; `-XX:+DisableAttachAgent` over three lines at 768 px).
     A first cell now has at least 8rem, and 13rem when it holds code; rows with an
     identifier (N, E4) keep their narrow first column.
  6. In a stacked record, a first cell without a header (the dedicated-account guide's
     "Which one") had no emphasis; it is the record's heading now, as in two-column
     records.
  7. The tables of contents of getting started and the security overview had no label;
     they say "On this page" now, as on the rendered documents.
  8. An odd last item of a grid stood alone beside a hole (the three panels under "If the
     account is compromised anyway" and of "Read further" while three columns are two;
     the seventh layer panel, the launcher, on the security overview). It takes the whole
     row now, its text at the measure.
- **The pictures:** the four committed pictures are the newest snapshots of the
  redesigned front end; each was looked at and its `alt` text and caption read against
  it (launch, refused, accounts, security).

### The generator and the converter (engine)

- `site/md.py`, the strict converter: it implements what the five documents use and
  stops with file and line on anything else; everything is escaped; raw HTML is shown as
  text, never passed through. Where CommonMark versions differ it follows GitHub's own
  renderer, cmark-gfm 0.29.0.gfm.13 (the version GitHub's Markdown API names; its source
  was read by the engine fixer, never run, and GitHub's API answered eight probes:
  `target/site-review-renderer/github-api/`, `target/fix-engine-scratch/github/`). Run
  against the specification examples (two outcomes accepted: the specification's HTML
  apart from what the converter adds on purpose, or a refusal), after the fixes:
  CommonMark 0.31.2, 652 examples: 339 identical, 298 refused, 15 different (one of them,
  example 354, `*£*bravo`, now renders as GitHub renders it, against 0.31); the 0.29-based
  GFM specification, all 672 examples: 341 identical, 319 refused, 12 different (11 raw
  HTML shown as text, 1 an image in raw HTML); the GFM extension examples: 6 identical, 18
  refused. Heading anchors are GitHub's: the rule rests on the probes' answers, and the
  title's anchor stands on each page's `h1`.
- `site/build.py`: renders the five documents, builds the hand-written pages from
  `site/src/`, writes `sitemap.xml` and `robots.txt`, and with `--check` verifies the
  output: every internal link and fragment resolves (also after percent-decoding); no page
  loads anything from another origin; the Content-Security-Policy and the referrer policy
  stand in `<head>`, one of each, on every page; outbound links are `https` with
  `rel="noopener noreferrer"`; the emergency page links to exactly the allowlist in
  `crates/proto/src/runbook.rs`, with the device warning before the first address;
  commands in `<code>`/`<pre>` on hand-written pages exist in the help text of
  `src/cli.rs`, sub-commands and options per command (in the documents a mismatch is a
  note); one `<h1>`, headings in order, the skip link to the one `<main>`; the trademark
  statement and the host sentence in every footer; images with `alt` and intrinsic size
  in their picture's proportions; every scrollbar thumb the style sheet colours at 3 : 1
  against its track; `tabindex="0"` on every `<pre>`; no script, no inline style, no
  comment; published SVG and PNG files that can do nothing when opened by themselves; two
  runs byte-identical. It refuses a directive that lost a brace or stands in a page
  header, an empty or non-`https` address option, a relative reference on the deployed
  `404.html` (GitHub serves it at any depth), a link to the site's own address, and SVG
  animation or `<feImage>`.
- Two stated repairs for table defects of `docs/THREAT_MODEL.md` (`REPAIRS`), each with a
  note at every build until the document is fixed (`docs/notes/website.md`, section 7).
- Tests: `site/test_md.py` (136) and `site/test_build.py` (220) build small sites of
  their own in a temporary directory, break one thing at a time and state the one error
  that stops the build, or the problems `--check` reports, and that there are no others.
  Mutation checks by the engine roles: 219 breaks of `build.py`, all caught; 40 of
  `md.py`, 39 caught, the 40th equivalent (white space is refused before that pattern is
  ever used); after the review, 33 breaks that each undo one fix, all caught, and the two
  formerly quadratic paths put back fail the timing test by seconds.
  `tests/site.rs` (5): builds the real site both ways with `--check`, compares the
  emergency links with the compiled `bastion_mc_proto::runbook::LINKS`, the help text
  with `bastion-mc --help`, no `{{` in any generated text file, and runs the unit tests.
- From a `git archive` of the tree with `env -i PATH=/usr/bin` (as the runner gets it)
  and again with `LC_ALL=POSIX PYTHONUTF8=0 PYTHONCOERCECLOCALE=0`: the unit tests and the
  deployable build with `--check` passed; no `__pycache__` left; the four Python files
  parse as Python 3.8.

### The look (design)

- `site/src/assets/site.css`: tokens (near-black gunmetal, one cyan accent, amber for
  caution, red for danger), system font stacks only, panels with a cut corner and a
  luminous lead, small-caps labels, monospace for commands and data. Three layout rules
  keep everything inside the window: grid and flex children may shrink; what is wider
  than its column by nature (code blocks) scrolls inside its own box; tables of four or
  more columns are stacked records at every width, three- and two-column tables below
  44em. Code in running text and in table cells is one box (see "Decisions"). Print
  rules: black on white, no navigation, every outbound address written out, the device
  warning first in a heavy frame. Forced colours and reduced motion are handled (the only
  motion is 0.12 s transitions of colours and of the skip link, inside
  `prefers-reduced-motion: no-preference`).
- `mark.svg` (a bastion in plan; legible at 16 px on dark and light tabs);
  `layers.svg` (960×606, shown from 62em) and `layers-stacked.svg` (328×1438, below):
  section 3 of the threat model, the same words in both, colour never the only carrier;
  since the review both carry the note "with audio = "pulse"" under "1 AUDIO SOCKET" (row
  D3's scope), in their labels and in their `<desc>`. How they were drawn:
  `target/fix-design/layers.py` (not committed; it stops when a text would not fit its
  room; the files are plain SVG and can be edited by hand).
- Shells: `template.html` (skip link, masthead with a "pre-release" chip, navigation),
  `document.html` (label, title, source line, the table of contents twice: a closed
  `details` below 80em, a sticky rail from 80em, `hidden` so that a reader without the
  style sheet gets one), `emergency.html` (the device warning, the lead, the runbook, the
  note about the compiled list), `partials/footer.html`, `partials/footer-minimal.html`
  (the emergency page: no outbound link).
- Verified by the design role: no sideways scrolling at 27 widths from 320 to 1920 px,
  with and without a scrollbar (540 measurements); keyboard focus visible on ten kinds of
  element (a 2 px cyan outline 2 px off, checked through a class standing in for
  `:focus-visible`, see "Learned"); without the style sheet the pages read in order with
  one drawing and one table of contents; the contrast table in `docs/notes/website.md`,
  section 5, checked against the tokens of the committed file.

### The overview (landing)

- `site/src/pages/index.html`: hero (what it is, for whom), the pre-release callout,
  "What it does, layer by layer" (the drawing, six blocks 01 to 06 each with "In force"
  and "Stays open"), "What it does not defend" (section 5 of the threat model, included
  word for word), the front end (four pictures, said to be test snapshots), "What has
  been tested, and what has not" (the README's "Status" and the threat model's "Not
  exercised", included), "How to get it" (the README's "Requirements", included; no
  releases, no binaries), "Where to read on".
- Every statement was read against `README.md` and `docs/THREAT_MODEL.md`; statements
  that said more than their source were corrected (lists in the former hand-over file,
  now summarised in the claim map below). 18 `{{REQUIRE}}` ties (Appendix A; 13 before
  the review).

### Getting started, the security overview, the error page (pages)

- `getting-started.html`: requirements, two ways to build, the display and sound
  decisions before the first launch, your own sign-in application, the first commands
  (doctor, an instance, a demo launch that needs no account, sign-in, play), the front
  end. `security.html`: assets, adversaries by letter, the layers and what each stops
  (with the drawing), the deliberate holes, what the credential broker changes, the ten
  points of section 5 in plainer words, what has not been verified (included), the
  account after a compromise. `404.html`: the pages that exist, the runbook, every address
  absolute under `{{ROOT}}`.
- Every statement checked against `README.md`, `docs/THREAT_MODEL.md`,
  `docs/MC_VAULT_ACCOUNT.md` and the help text; 124 `{{REQUIRE}}` ties (36, 87 and 1; 119
  before the review), each tried: with its phrase changed in a scratch copy of the
  documents, one build stops and names every phrase and page. Every code block takes
  keyboard focus (`tabindex="0"`), and `--check` requires it.

### Deployment and the pictures (deploy)

- `.github/workflows/pages.yml`: the four official Pages actions pinned to the commits of
  their latest releases (looked up twice through the public API on 2026-11-13; table and
  update procedure in `docs/notes/website.md`, section 4); least privilege (top level
  `contents: read`, the deploy job alone `pages: write` and `id-token: write`); one
  concurrency group per branch, a running deployment never cancelled; triggered by every
  push to `main` (no path filter, see "Decisions") and by hand; the build job runs the
  unit tests, then the generator with `--check`; only `main` is deployed. The pins were
  looked up again on 2026-11-14 and are still the latest releases. Valid against the
  public workflow schema. The build
  job's own scripts were run on the committed tree in an empty environment, the artifact
  packed with the action's `tar` call, served below a sub-path on the loopback address
  and crawled: every reference resolved below the sub-path.
- `site/update-screenshots.sh`: installs exactly the pictures the pages ask for
  (`{{SHOT name "alt"}}`), from either naming scheme of the front end's snapshots, byte
  for byte after checking each is a complete PNG without text, time or EXIF chunks; all or
  nothing. Tried in a scratch tree against both schemes and the failure cases.
- `site/src/robots.txt`, `.gitignore` (`/site/dist/`, `/site/__pycache__/`,
  `/site/.update-screenshots.*/`), `docs/notes/website.md`.

## Not verified

- **The workflow has never run.** Its first run, after the owner's two steps, is its
  first test: `pages: read` for "Configure Pages" comes from GitHub's permission table,
  not from a run; the runner's Python is 3.14.4 and only 3.14.9 exists here (the 3.14
  changelog was read; nothing the site uses is known to differ).
- **GitHub Pages itself:** serving `404.html` for missing addresses below the sub-path,
  and its headers, are documented behaviour, not seen here.
- **Browsers:** only headless Firefox 159. No other engine, no real phone, no screen
  reader (the structure and `alt` texts were read, not heard). Some rules are recent CSS
  (`text-wrap: balance`, `:has()`, the gutter labels floated in a `flow-root` cell);
  another engine may wrap differently, and `:has()` falling away only narrows a first
  table column back to 8rem.
- **Paper:** the print rules were applied on screen at 680 px; real pagination and
  `break-inside` were not seen (no print-to-file here).
- **Keyboard focus** in a real browser window: the recipe cannot show focus. That no ring
  is cut off by a box around it (web-3) was shown by the design fixer's probe
  (`target/fix-design/probe.py`); no repository test can hold it, because neither the
  generator nor the unit tests have a layout engine.
- **GitHub's renderer beyond what was asked.** The converter follows cmark-gfm
  0.29.0.gfm.13 where CommonMark versions differ; its source was read, never run, and
  GitHub's Markdown API answered eight probes. The anchor rule is taken to be "letters
  (Unicode property Alphabetic), marks, decimal digits, connector punctuation, `-`,
  space", which every probe agrees with; characters newer than GitHub's Unicode version
  would be kept here and dropped there; of the Alphabetic symbols only circled, squared
  and negative circled letters were asked. That github.com renders a README exactly as
  the API does was not compared.
- **The documents are trusted text.** The nesting limit and the linear paths make an
  accident fail quickly; they were not reviewed against an adversary writing the
  documents.
- The mutation checks prove that each listed break fails a test, not that every break
  would.

## The review and the fixes (2026-11-14)

After the integration three reviewers looked at the site, each through one lens:
**renderer** (11 findings, `target/website-handins/renderer-review.json`), **accuracy** (7)
and **web** (5). Three fixers then worked side by side, each in an area of files: engine,
design, content. Every finding was reproduced before it was touched, and every one held.
The engineer who closed the website checked each fix again in the final tree (the
reviewers' own cases run against the final converter, the pages read against their
sources, the style sheet and the workflow read) and fixed what was still open. Outcome of
every finding:

| Finding | What it was | Outcome | Test or check |
|---|---|---|---|
| renderer-1 (high) | a tab after a closing code fence paired the fences wrongly; whole sections became code and `--check` passed | fixed: spaces or tabs may follow a closing fence; a line inside a block that begins like its fence stops the build | `FencesPairAsOnGitHub`; `TheRealSources.test_a_tab_after_any_closing_fence_changes_nothing` (all 25 closing fences); the reviewer's end-to-end case gives a byte-identical page |
| renderer-2 | emphasis next to symbols followed CommonMark 0.31, not GitHub | fixed: cmark-gfm's punctuation and white-space tables, its opener bound and the strikethrough extension's effect on `*`/`_`; strikethrough refused where GitHub would draw it | `EmphasisAsOnGitHub`; the reviewer's case renders GitHub's answer |
| renderer-3 | tables recognised and ended differently | fixed: rows indented by four refused, a list item of any number ends a table, rows of dashes GitHub does not read refused, `:--` without pipes refused | `TablesAsOnGitHub`; the threat model with row P4 indented stops the build at its line |
| renderer-4 | an escaped backslash before a pipe split a cell | fixed: `md.split_row` splits as cmark-gfm; `build._raw_cells` uses it | `TablesAsOnGitHub`, `Repairs.test_cells_are_counted_as_the_converter_counts_them` |
| renderer-5 | a rule after list items became an item | fixed, with cmark-gfm's tight list after a rule | `ListsAndRulesAsOnGitHub` (a heading in an item loosens the list, as GitHub answered) |
| renderer-6 | Unicode white space taken for Markdown white space; U+2028/9 passed | fixed: spaces and tabs only; GitHub's space table for emphasis; U+2028/9 refused | `WhiteSpaceAsOnGitHub`, `UnknownConstructs` |
| renderer-7 | character references for spaces made a hard break | fixed | `WhiteSpaceAsOnGitHub` |
| renderer-8 | heading anchors: the title not counted, ids shared with rows and the shell, `slugify` differences | fixed by the engine fixer (title counted, clashes refused, GitHub's slug); completed when closing: the title's anchor stands on each page's `h1` (`{{TITLE_ID}}`), so a link written for GitHub to a document's title lands at the top instead of failing `--check` | `AnchorsAsOnGitHub`, `Documents.test_the_title_counts_among_the_headings_as_on_github`, `test_every_title_carries_its_anchor`, `test_a_title_cannot_take_an_id_of_the_page_around_it` |
| renderer-9 | deep nesting crashed with a traceback; quadratic paths | fixed: a nesting limit of 32 with the line, a backstop for `RecursionError`, linear paths | `LimitsAndLines` (the timing test fails by seconds if a path is put back) |
| renderer-10 | refusals named the wrong line | fixed | `LimitsAndLines`, `Paragraphs.test_windows_line_endings`. Seen when closing: in a build a lone carriage return never reaches that refusal, because the generator reads documents with Python's universal newlines; it is then a line end, as in CommonMark and on GitHub, so this is not a defect |
| renderer-11 | a document could not mention `{{REPO_URL}}` | fixed: `md.escape` writes the first of two braces as `&#123;`; a directive in a page header stops the build | `CheckTokens.*`, `Directives.test_text_from_a_document_is_never_taken_for_a_directive` |
| accuracy-1 (medium) | sound and graphics statements false for `audio = "alsa"`, which the launcher offers and no document describes | fixed in the site: every such statement limited to `audio = "pulse"` (the default) and `"none"`, the drawings carry "with audio = "pulse"", the alt texts follow; **open for the owner** of the documents (`docs/notes/website.md`, section 7) | three ties to D3 and the README; re-read the scope sentences when the documents change |
| accuracy-2 | "comfortable to play" inside gamescope | fixed: "playable … (pointer capture, full screen)" on both pages | TM D1 tie on both pages; a tie holds a phrase, not a meaning |
| accuracy-3 | a caption said the recovery pages are the only addresses the window opens | fixed: the emergency panel's addresses, and the sign-in links named; **open for the owner**: TM A3's "no others" and the front end's label (then `site/update-screenshots.sh`) | ties to G6 and G3 |
| accuracy-4 | "The graphics card, and nothing else of the hardware" | fixed: "The graphics card's own device nodes" | the D4 ties |
| accuracy-5 | the list of what was not exercised left out fuzzing | fixed: added, marked "among other things", linked to the whole list | one tie over the whole README row |
| accuracy-6 | the page implied that `makepkg` builds the reader's checkout | fixed: the page says only what the README says; **open for the owner**: the PKGBUILD template clones its own unverified copy | none possible for an implication |
| accuracy-7 | "the right column … on its left" is false on a phone | fixed: a sentence true in both layouts | none possible |
| web-1 (medium) | scrollbar thumbs at 2.1 : 1 | fixed: `edge` thumbs (3.67 and 3.56); the contrast table has the rows | `--check` computes every `scrollbar-color` pair (`scrollbar_problems`) |
| web-2 | code blocks of 38 to 60 characters scrolled on a phone without keyboard focus | fixed: every rendered code block takes focus; completed when closing: the three hand-written blocks without it got it, and `--check` fails on any `<pre>` without `tabindex="0"` | `CodeBlocks.test_every_code_block_can_be_reached_with_the_keyboard`, `CheckMarkup` |
| web-3 | the focus ring was cut off in the table of contents beside a document | fixed: `.toc a:focus-visible { outline-offset: -2px }` | the design fixer's probe: no ring cut on nine pages at three widths; no repository test possible |
| web-4 | the workflow's path filter missed files the generator reads, while its comment called the list complete | fixed: no path filter; comment and notes say why | `TheWorkflow.test_every_push_to_main_builds_the_site` |
| web-5 | the notes said no committed file names the owner; the PKGBUILD does | fixed: the sentence is about the website's files and names the places outside it (also in `site/CONTRACT.md` when closing) | a `git grep` with the two names before every commit |
| closing-1 | the security overview said "UDP does not exist in the sandbox" (TM §5 item 7) two panels after "UDP to the launcher's DNS port" (TM J10) | fixed in the site's own words: "UDP reaches nothing from the sandbox but the launcher's DNS responder"; **open for the owner**: the same qualification in item 7 | tie to J10's "The only exits are sockets held by the launcher" |

The requests the fixers left for each other (`site/requests/fix-*.md`) were all answered:
the engine fixer added the scrollbar check, the image-proportion check and the workflow
test; the content fixer followed the drawings' new words and size; the two optional
requests of the engine fixer (the title's anchor on the `h1`; keyboard focus for every
hand-written code block) were done when closing. The request files and the three per-area
hand-over files (`docs/notes/website-fix-*.md`) were folded in here and deleted.

## The close (2026-11-14)

- Changed: `{{TITLE_ID}}` in `site/build.py`, `site/src/document.html` and
  `site/src/emergency.html`; a `--check` rule for `tabindex="0"` on every `<pre>`;
  `tabindex="0"` on three code blocks of `getting-started.html`; the UDP sentence twice on
  `security.html`, with a tie; the generator's description; four new unit tests (356 in
  all); `site/CONTRACT.md` (the names sentence, the code-block rule); these notes and
  `docs/notes/website.md`.
- Every page at 360, 480, 768, 1024 and 1280 px rendered with the task's Firefox recipe
  and looked at, strip by strip (`target/shots/close-<page>-<width>.png`, laid out as
  `close-sheet-<width>-NN.png`); the emergency page with its print rules applied on screen
  at 680 px (`close-print-emergency-680.png`). Nothing sticks out, nothing is clipped, the
  components look the same on every page; what the "Known and accepted" items below
  describe is still there and still accepted.
- The measuring tool at 39 widths, 702 measurements: no problem (see "Last state").
- The four pictures: `site/update-screenshots.sh` against the redesigned interface's
  snapshots reports "unchanged" for all four.
- The action pins, looked up again: unchanged.
- The workflow's build job simulated on `6259e08`: 0 problems (numbers in "Last state").

## Open

For the owner of the repository:

1. **Publish** (`docs/notes/website.md`, section 3): Settings, Pages, Source "GitHub
   Actions"; push `main`. Then look at the published address, at an address that does not
   exist below it (the site's own error page, with its style sheet), and at the
   emergency page on a phone.
2. **The player name in two pictures: decided on 2026-11-14, the pictures stay as they
   are.** `shot-refused.png` and `shot-accounts.png` show the player name of the front
   end's test fixtures (`gui/src/app.rs`, also used by `gui/src/view/tests.rs`), not
   anything read from this machine; it is also the name of one of Minecraft's default
   characters, and the overview says the names are test data. The owner decided to publish
   them with it; nothing is to be done.
3. **After a merge that changes the documents or the front end** (done for the third and
   fourth rounds on 2026-11-14, section "Rounds 3 and 4"): `cargo test -p
   bastion-mc-gui`, `site/update-screenshots.sh`, read the `alt` text and caption of every
   picture it reports "updated" (Appendix B lists what each names), `python3 site/build.py
   --check`, `cargo test --test site`. When `README.md` or `docs/THREAT_MODEL.md` changed,
   go through the claim maps (Appendix B), not only the ties: a tie holds a phrase, not a
   meaning (block 03 of the overview had no tie on the join and said the opposite of the
   fourth round's J13).
4. "Pre-release" and "There are no releases and no binary downloads" stop being true on
   the day a release is made; nothing checks them (`grep -rni -e pre-release -e 'no
   releases' site/src` finds them).

For the owner of the documents (the website may correct their Markdown, not what they say):

5. Done: the two table defects of `docs/THREAT_MODEL.md` were fixed in the document; the two
   `REPAIRS` entries were deleted on 2026-11-14.
6. Done: what `open` network mode does. The README says it now ("Network filtering and the
   credential broker"); the security overview follows.
7. `audio = "alsa"` is described now (README "Sound", row D3). Still open: row D4 says the
   device-ioctl right is limited to the graphics nodes, while with `alsa` the raw sound
   devices are on the sandbox's device list too (`src/sandbox/display.rs`); section 3's
   drawing has "1 audio socket" only; `examples/config.toml` offers `alsa` without the
   warning the README gives.
8. Done: row A3's "no others" and the front end's label were fixed (A3, `gui/src/view/security.rs`);
   `shot-security.png` was refreshed.
9. `packaging/arch/PKGBUILD` is a template that clones its TODO `url` at the default
   branch, without the signed tag and with checksums skipped (accuracy-6). The README says
   so now; whether makepkg should build the local tree or a signed tag is still open.
10. Section 5, item 7 now says "UDP exists only as relayed datagrams to the UDP ports that
    a server rule names"; J10 and E15 describe UDP to the launcher's DNS port as well
    (closing-1, in the new wording). The security overview qualifies the item.
11. The README says that Realms is brokered in an instance without the phone ("Realms";
    and "Settings sync and the brokering of Realms work without it" in "The in-game
    phone"); rows J21 and J22, `docs/PHONE.md` and `src/launch.rs` (`realms_on` requires
    `phone_possible`) say it is not, and that approvals are refused up front. The pages
    follow the threat model.
12. The README says that a burner session records clips and keeps them ("The in-game
    phone"); `docs/PHONE.md` says "no clips (a burner session keeps nothing)", and
    `src/launch.rs` does not record. The pages say nothing about it.
13. The README's drawing of the front end says "(no secrets: window, fonts, GPU)"; section 3
    of the threat model says the event protocol carries one secret, the code of a waiting
    Realms approval. The pages and the drawings say "holds no token" and name the code.

For the owner of the front end:

14. Its Realms approval snapshot (`launch-realms-approval-*.png`) shows the prompt for a
    vanilla instance, which never gets one; with a Fabric instance in the fixture, the
    overview could show the prompt (a `MENU` line in `site/update-screenshots.sh`, a
    `{{SHOT}}` with its `alt` text and caption, and a claim-map row).
15. Its scram line ("Stops running games, deletes refresh tokens and vault files. Revokes
    nothing at Microsoft.", `gui/src/view/security.rs`) does not name the Spotify token,
    the Realms TOTP secret or the phone browser's profile. `shot-security.png` shows it,
    and its `alt` text quotes it.

Known and accepted:

16. A code box is one unbreakable piece for the line breaker, so a bracket or a comma next
    to one can stand alone at the start or the end of a line, and a box that has to wrap
    takes the whole line (the word after it can stand far right). No CSS removes either;
    the alternative broke options and paths inside (see "Decisions").
17. The overview's pictures and the layers drawing print as dark pictures on paper.
18. The drawings label the graphics hole "/dev/dri and its sysfs directory", as section 3
    of the threat model does; row D4 adds "(+ NVIDIA nodes if present)". If the drawings
    change again, that note could go under "/dev/dri" as the audio note goes under "1
    AUDIO SOCKET". They show a launch without the companion mod; the captions name what a
    Fabric profile adds.

## Decisions (worth keeping)

- **Included, not retyped.** The overview includes section 5 of the threat model, the
  README's "Status" and "Requirements" and the threat model's "Out of scope" and "Not
  exercised" paragraphs word for word (`{{SECTION}}`, `{{PARAGRAPH}}`); the security
  overview and getting started include "Not exercised" and "Built and tested on Arch
  Linux". An include follows the document or stops the build with the name of the
  missing heading; a copy would silently go stale. The status table therefore has no
  `tested`/`partly`/`untested` cell classes: they would need a retyped table.
- **Ties.** A hand-written statement that can go stale carries a `{{REQUIRE source
  "phrase"}}` with the source's own words (Appendix A). Hand-written text avoids numbers
  that change with every merge (test counts, sizes); where a number matters it comes in
  through an include.
- **"What it does not defend"** stands directly after "What it does" on the overview, and
  the hero's second button jumps to it. The security overview restates the points (ten
  until the fourth round, sixteen since) in plainer words and ties each one, so a new
  point, a removed one or a new order stops the build.
- **Code is one box**, in running text and in table cells: a command, an option or a path
  moves to the next line whole, and only a piece longer than its line wraps inside its
  box (`overflow-wrap: anywhere`, balanced, one em short of the line). As ordinary inline
  text, a browser broke it after a hyphen or a slash (`--` | `repair`, `/` | `etc`). Counted
  by the design role on four pages at five widths: inline broke 182 pieces, boxes leave 71
  stray marks; a stray mark changes nothing a command says. `nowrap` is not possible:
  some pieces are wider than a column at 360 px.
- **Tables:** four or more columns are stacked records at every width (as a grid, even
  at 1280 px, the threat model's matrix broke about a hundred code pieces per width), with
  the label in a gutter from 62em and the record kept to the measure; three and two
  columns are records below 44em; the generator's `role` attributes keep every one a
  table for a screen reader.
- **One page width.** Header, content and footer share the 76rem column on every page;
  running text, callouts and tables of contents keep to the 46rem measure; code blocks
  and grids use the whole column. An odd last item of a grid takes the whole row.
- **Two drawings** of the layers, the wide one with `hidden`: the style sheet shows exactly
  one; a reader without it gets one. Both carry the same `alt` text. No `loading="lazy"`
  on them.
- **The front end is described by what it does**, not by its widgets, and the pictures
  are said to be test snapshots drawn from fixtures.
- **The footer's host sentence** says what GitHub's documentation says ("Data
  collection": the visitor's IP address is logged for security purposes) and no more.
- **Deployment:** the build job on `ubuntu-26.04` (its `python3` is the 3.14 the
  generator was developed with), not `latest`; the deploy job only for `main`; the
  workflow stops on an empty address before the generator does; the pages decide which
  pictures exist, the screenshot script where each comes from.
- **No path filter** on the workflow (web-4): a document may link to any file of the
  repository and the generator checks every such link, so no fixed list of paths can be
  complete. A unit test keeps the filter out. The price is a run (about ten seconds of
  tests and build) on pushes that change no input.
- **GitHub's renderer, or a refusal** (renderer review): where CommonMark versions
  differ, the converter does what cmark-gfm 0.29.0.gfm.13 does, established from its
  source and from GitHub's answers to probes, never from memory; where it cannot be sure,
  it refuses with file and line.
- **Scope instead of description** (accuracy-1): the site may not describe what no
  document says (`audio = "alsa"`), so it limits each statement that the undocumented
  case would make false to the cases the threat model describes, and says that it claims
  nothing for any other value. (Since the fourth round's documents describe `alsa`, the
  pages describe all three values; the rule stands for the next undocumented case.)
- **Every code block takes keyboard focus** (web-2), whatever its length: whether a block
  scrolls depends on the window, the reader's font size and the depth of a list, which no
  page knows. `--check` requires it.
- **The title's anchor stands on the `h1`** (renderer-8): a link written for GitHub to a
  document's title lands at the top of the page, as there.

## Learned (not obvious)

- **Headless Firefox and the recipe.** It applies a page's Content-Security-Policy to
  `file://` pages too (inline styles and `style=""` are blocked). It takes the picture at
  the load event: a lazy image has not loaded then and shows as its `alt` text, and a
  transition has not run. Without a height it photographs the whole page and keeps 12 px
  for a scrollbar. It never shows keyboard focus (the window has no system focus). It
  sometimes exits with status 11 after the picture is written.
- **The content of a closed `<details>`** still has layout boxes in Firefox: a script that
  measures text must skip it, or it reports the table of contents as drawn over the page.
- **An inline box** (a code box) takes no text decoration from its link (it draws its own
  underline); a break before and after it is always allowed, even next to a bracket; and
  once it wraps, it takes the whole line.
- **Automatic table layout:** a code box with `overflow-wrap: anywhere` adds almost
  nothing to its column's minimum width, so a column of short prose beside long code is
  squeezed to a word per line. Records avoid it; a minimum width for the first cell,
  except in rows with an identifier, fixes three-column grids.
- `<picture>`, `<source>` and `srcset` are refused by `--check`; the choice between the two
  drawings is made by the style sheet.
- `actions/upload-pages-artifact` leaves dot files out of the artifact, which is why the
  generator's marker file `.bastion-site-output` is not published. `build.py` refuses any
  file in `site/src/assets/` whose name starts with a dot, and `--check` refuses a
  published file that no page uses: nothing temporary may be written there.
- `robots.txt` is read only at the root of a host, so on a project site it has no effect
  (the file says so).
- `cargo clippy --workspace --all-targets` answers from its cache when nothing changed;
  touch the file (or run `cargo clippy --test site -v`) to see it checked.
- `python3 -m unittest` without `-B` leaves `site/__pycache__/` (ignored).
- In a working copy where someone saves a file between the two builds of `--check`, it
  reports "X differs between two runs": that is the edit; run it again.
- A scratch copy for trying the ties needs every file of `git ls-files`: the generator
  checks that the documents' own link targets exist.
- Read the clock (`date`) before writing a time into a note.
- **GitHub's Markdown API** names its renderer in the header `x-commonmarker-version`
  (commonmarker 0.23.13, which carries cmark-gfm 0.29.0.gfm.13). Newer CommonMark
  implementations (cmark 0.31, md4c) differ from it in places (emphasis next to
  symbols, a blank line after a rule in a list), so a comparison with them can miss what
  GitHub does.
- The generator reads documents with Python's universal newlines, so a lone carriage
  return reaches the converter as a line end, as CommonMark and GitHub treat it.
- **A full-page picture of a long page** is easier to look at cut into strips laid side by
  side (`target/close-scratch/sheets2.py`, with `vips`): one sheet per width shows every
  page's layout at once, and a strip can be cut out at full size where something looks
  wrong.

- **A page taller than Firefox can draw** (the manual and the threat model at 360 px, about
  70 000 and 130 000 px) gives no picture at all: "Failed to allocate a surface due to
  invalid size". Render it in parts: a copy of the page per slice of the article's
  top-level blocks (2026-11-14, `target/r4site/tools/split.py`).
- **`build.Site` wants a `Path`** as its root; with a string the help text cannot be read
  (`TypeError` in `HelpText`). The snippet in Appendix A passes one.
- **A tie without a meaning check is not enough.** Block 03 of the overview said that a mod
  "can be made to appear on that server as you" with no tie; the fourth round's J13 says
  the join is not tied to that server at all. The build passed that sentence; only reading
  the claim map against the new row found it.

## Tools that were not committed

Everything below lives under `target/` of the team's working copy and goes when that
copy is deleted; nothing in the build needs it. Worth keeping, if anything:

- `target/integrate/`: `shot.sh` (a page at an exact layout width), `render-all.sh`,
  `measure.py` (the measuring tool described above), `printsim.py` (a copy whose style
  sheet applies its print rules on screen).
- `target/design/layers.py` (draws both layer drawings and stops when a text would not fit
  its room), `target/design/sheet.py` (cuts a tall picture into sheets),
  `target/design3/overflow3.py`, `codewrap.py`, `target/design4/tablecode.py`, `focus.py`,
  `target/design/contrast2.py`.
- `target/engine-scratch/` (mutation lists and runners, the comparison with the
  specification examples), `target/deploy-tools/` (workflow checks, the simulated build
  job, the PNG comparison; three need PyYAML), `target/pages-tools/doctext.py` (a
  document's text as `{{REQUIRE}}` compares it).
- From the review and the fixes: `target/site-review-renderer/` (the renderer reviewer's
  cases, scripts and GitHub's answers to probes 1 to 4), `target/fix-engine-scratch/`
  (repro scripts, the cmark-gfm sources read, GitHub's answers to probes 5 to 8, the 33
  mutants), `target/fix-design/` (`layers.py` with the audio note, `probe.py` and
  `focusring.js` for focus rings, `scrollbars.py`, `pixdiff.py`),
  `target/site-fix-content-tools/` (`view.py`, the workflow checks with the path filter
  reversed).
- From the close: `target/close-scratch/` (`probe_cases.py` runs the reviewer's cases
  against the converter, `sheets2.py` lays renders out as sheets).
- From the pass of the third and fourth rounds, in the clone `target/agents/r4-site`:
  `target/r4site/tools/` (`shot.sh`: a page at a layout width, headless Firefox in
  bubblewrap without a network, a throw-away profile; `sheets.sh`: a tall picture as sheets
  of strips, with `vips`; `split.py`: a page in parts; `measure.sh`: the overflow harness,
  a copy of each page without its policy and with a script that measures and paints the
  verdict; `doctext.py`: a document's text as the ties see it, and every tie checked), and
  the renders under `target/r4site/shots/` and `target/r4site/measure-full/`.

## Appendix A: the `{{REQUIRE}}` ties

A tie writes nothing; the build stops when the document no longer says the phrase, and
one run names every phrase that is gone. The phrase is compared with the document's text
as a reader sees it: Markdown marks gone, white space folded, every block in the order of
the document joined by single spaces (so a phrase may run from one list item or table
cell into the next; two tests in `site/test_build.py` hold that). When a build stops,
read the statement above the directive against the document's new text, correct the
statement, then the phrase.

### The ties of the four pages

Every tie stands on its own line directly after the statement (or the list item, or the
panel) that it backs; in the claim maps of Appendix B a statement with a tie is marked
**†**. On 2026-11-14, after the pass of the third and fourth rounds: 336 ties, 64 on the
overview, 67 on getting started, 204 on the security overview, 1 on the error page (142
before that pass: 18, 36, 87, 1). Tied:

- every statement of the kind "not run", "not exercised", "not assessed", "only … was
  tested", "not built", "has not run against the real service";
- every number and version (kernel 6.12, Landlock ABI 8, 9 and 10, Linux 7.2, Java 17 and
  25, the lifetimes of the two tokens, "about 24 hours", the token keeper's 20 seconds and
  60 refreshes, the Web API's 60 calls a minute, the Realms dialer's 8 connections, the
  read date 2026-11-13); never a test count;
- every phrase the page quotes ("none for O") and every setting whose default the page
  states;
- the statements about sound for each value of `audio` (D3, the README's "Sound"), the
  sign-in links of the front end (G3, G6), the UDP sentences (J10, J20);
- the "Stays open" and "Does not stop" items and "What remains" sentences that restate a
  residual of a row (on the overview since 2026-11-14 too: block 03's join, which had no
  tie and said what J13 no longer says);
- the statements about the companion mod, the phone and its services (README "The
  companion mod", "The in-game phone", "The phone's helpers"; TM §3, J19 to J24, H1 to
  H23; `docs/COMPANION.md`);
- **the sixteen points of `security.html#not-defended`**: directives after the list, each
  beginning with the last words of the point (or heading) before it, the last one ending
  with the first words of the heading of section 6; and ties over the parts of points 1,
  6, 7 and 12 that the page restates. So a reworded point, a seventeenth point, a removed
  point or a changed order stops the build. Not tied there, on purpose: the remark about
  the development machine's `docker` group in point 1, and most of the parenthesis on the
  launcher's bounds in point 6, which the page summarises.

When a build stops with "… no longer says '…', which a statement in site/src/pages/PAGE
relies on": find the directive with that phrase in the page, read the statement above it
against the document's new text, correct the statement, then the phrase. To list every
phrase of the four pages that is gone, without building:

    python3 - <<'EOF'
    import re, shlex, sys
    sys.dont_write_bytecode = True
    sys.path.insert(0, "site")
    import build
    from pathlib import Path
    site = build.Site(Path("."), build.PREVIEW_REPO_URL, build.PREVIEW_SITE_URL, absolute_error_page=False)
    for page in build.PAGES:
        text = open("site/src/pages/" + page, encoding="utf-8").read()
        for words in re.findall(r"\{\{(REQUIRE [^{}]*)\}\}", text):
            _, source, phrase = shlex.split(words)
            try:
                site.require("pages/" + page, source, phrase)
            except build.BuildError as problem:
                print(problem)
    EOF

(`Site` takes a `Path`: with a string the help text cannot be read. The tool of the pass of
2026-11-14, `target/r4site/tools/doctext.py` in its clone, does the same and also prints a
document's text as the ties see it.)

**A tie holds a phrase, not a meaning.** A new residual added to a row, or a new row,
stops nothing. After a change to `docs/THREAT_MODEL.md` or `README.md` the claim maps
below are still the list to go through; the ties only make sure that what the pages
already say cannot silently become false.

Many phrases run from one list item, table cell or heading into the next, on purpose (the
sixteen points; a table row's label with its value, as in the three new assets; "Not
built, …: Login-protocol proxy."; the getting-started callout's tie over the README's whole
row "NVIDIA, …" into its second cell, so that an item added to that cell stops the build).
The generator promises that this works (the docstring of `Site.require` in
`site/build.py`): the text is every block in the order of the document, joined by single
spaces and nothing else, and two tests in `site/test_build.py` hold it. An HTML comment is
no part of that text (the README's `<!-- COUNTS -->`), as it is no part of what a reader
sees; a test holds that too.

## Appendix B: the claim maps

The only tie between a hand-written sentence and its source. After any change to
`README.md` or `docs/THREAT_MODEL.md`, go through them: an include follows the document
or stops the build, a tie stops it when its phrase is gone, but a new residual risk or a
changed meaning stops nothing.

### The overview (`site/src/pages/index.html`): every statement and where it comes from

"TM" is `docs/THREAT_MODEL.md` (rows such as J13, sections such as §5); "README" is
`README.md` (sections by their heading; "intro" is its first two paragraphs). Where the
page gives a residual risk, the source of the residual is named too. Statements that are
included word for word are marked "include"; **†** marks a statement with a tie. Brought up
to the documents of the third and fourth rounds on 2026-11-14.

#### Hero and the callout under it

| On the page | Source |
|---|---|
| "A Minecraft launcher for Arch Linux that treats the game as hostile code" (`h1`, title) | README, first line under the title, word for word |
| label "Pre-release · unofficial · Arch Linux · built from source" | "pre-release": README "Status" ("**Pre-release.**"); "unofficial": the trademark statement the footer carries (README "Trademarks and licence"); Arch Linux: README intro; built from source: README "Status" ("it is built from source") |
| "runs the game in a stacked sandbox" | README intro ("It starts the game in a stacked sandbox") |
| "By default the game reaches the network only through the launcher's allowlisting relay" | README intro; "by default" because of the modes `none` and `open`: README "Network filtering and the credential broker", TM J13 and §5.3 |
| "it holds a placeholder instead of your access token" (by default) | README "Network filtering and the credential broker" ("It is given a placeholder"); TM J13 |
| "for people who … assume that a mod, a server or a downloaded world may be malicious" | TM §2 (M, S); README "The sandbox" ("Content you do not trust") |
| "prepared to build the launcher themselves and to sign in through an Azure application of their own" | README "Requirements"; README "Your own OAuth application" |
| second lede: "Fabric instances also get the launcher's own companion mod, with an in-game phone: Spotify, Discord and other sites you list in a browser that runs in a sandbox of its own, clips, approvals of Realms joins and settings sync. The mod is interface only: the launcher decides what each command from the phone may do, and no secret of these services enters the game" | README intro, second paragraph (the list and the last sentence nearly word for word) † (two) |
| callout: "Sign-in has not yet been run against the live Microsoft, Xbox and Mojang services" | README "Status" (first paragraph; the row "Microsoft → Xbox → Minecraft sign-in…") † |
| "neither has anything that needs a signed-in account, such as joining a real multiplayer server" | the same; TM §7 "Not exercised" † |
| "nor anything of the phone's services that needs an account of yours: the Spotify Web API, a signed-in Discord, Realms" | README "Status", first paragraph ("Neither has anything of the fourth round that needs an account of yours…") † |
| "The sign-in chain is implemented and tested against scripted replies and a stand-in Mojang" | README "Status", the sign-in row |
| "What has run with the real game is Mojang's demo mode, without an account, vanilla and as a signed Fabric profile" | README "Status", row "Real Minecraft 26.3 client in the sandbox (demo mode, no account)…"; TM §7 E7 † |
| "and the companion mod with its phone on one version of each of its ten families of Minecraft versions, with stand-in apps behind the phone" | README "Status", row "Companion mod on every Fabric release from 1.20.1 to 26.3 (ten families)…" ("on the representative of each family"; "against the demo service (fake apps, a test switch)") † (two) |
| "There are no releases and no binary downloads" (also in "How to get it") | README "Status" ("There is no release, no signed tag and no binary download"); the footer says the same |

#### What it does, layer by layer

| On the page | Source |
|---|---|
| the drawing (`assets/layers.svg`, `assets/layers-stacked.svg`) | TM §3, the drawing and the paragraphs under it; "only the listed trees": TM J1; "optional", the front end: README "The graphical front end"; the note "with audio = "pulse"" under "1 AUDIO SOCKET": TM D3. Corrected on 2026-11-14: "before it reads anything from the game, it confines itself (Landlock + seccomp, all threads)" after TM §3 ("before it reads anything from the game the launcher confines ITSELF (Landlock + a system-call filter, on all its threads)"), and "holds no token" of the front end after TM §3's last paragraph (the event protocol has no field for a credential and carries one secret, a waiting Realms approval's code) and G1 |
| its `alt` text: launcher with sign-in, verified store, egress relay and DNS responder, credential broker, log relay, launch frame; "before it reads anything from the game, the launcher confines itself, with Landlock and seccomp, on all its threads"; the game "treated as hostile from its first instruction and by default holding a placeholder"; four nested frames; what is writable and read-only; "Three deliberate holes …: one display socket, one audio socket with audio = "pulse", and the graphics device"; the network outside the host; "An optional front end is a separate process that holds no token" | the two drawings as committed (rendered and looked at on 2026-11-14); behind them TM §3 |
| caption: "The trust boundaries on your machine, drawn after section 3 of the threat model. The launcher is the trusted part; the game is not, and runs inside a systemd scope, namespaces, Landlock and a seccomp filter, with three deliberate holes" | TM §3; README "The sandbox" |
| caption: "The audio socket among them is what the default setting `audio = "pulse"` hands over; `audio = "none"` removes it, and `audio = "alsa"` hands over the raw sound devices instead" | TM D3; README "Sound" († in block 02) |
| caption: "The drawing shows a launch without the companion mod: in a Fabric profile the sandbox also gets the mod, read-only, and one socket to the launcher (block 07)" | TM §3 (the drawing's "ro: /bastion/companion" and the holes' "Fabric profiles: 1 socket to the launcher"); TM J2; README "The sandbox" († in block 02) |
| caption: "blocks 01 to 05 below go through it and say what stays open at each. Blocks 06 and 07 are about the account itself and about the phone, which the drawing does not show" | a statement about this page and the drawing |
| "The game's Java process is treated as hostile from its first instruction"; "A malicious or compromised mod …"; "a server or a piece of content …" | TM §3; TM §2, M and S |
| "Each block below says what is in force at one layer and names what stays open there…" | a statement about this page; "where there is one": rows without a test, see "What has been tested" |
| **01** "What the game is started from is fixed before it starts…" | README "Verified downloads and signed profiles"; TM C1, C2, C5 |
| "Every Mojang file is checked against the published hash chain…"; "A mismatch aborts the launch, and nothing is fetched again unless you ask for it with `--repair`" | README "Verified downloads and signed profiles"; TM C1 |
| "A modded instance exists only by virtue of a manifest you signed (Ed25519). At launch the signature, the manifest's serial, every loader library and every jar in `mods/` are verified; one extra, missing or modified file aborts the launch" | README "Verified downloads and signed profiles"; README "Signed profiles" |
| "An older signed manifest is not accepted again, and a manifest is bound to its instance" | TM C3; README "Signed profiles" ("no rollback") |
| "Mods fetched from Modrinth with `bastion-mc profile add` are checked for size and SHA-512 before they reach `mods/`, and they count for nothing until you sign the manifest" | README "Verified downloads and signed profiles" †; TM C8, C2 |
| "Java comes only from root-owned runtimes under `/usr/lib/jvm`; the launcher never downloads one" | TM C5; README "Requirements" |
| "The game is given only the resource and shader packs you recorded…" | TM J2; README "Resource packs and shaders" |
| open: "Mojang publishes SHA-1 hashes only…" | TM C1, residual |
| open: "A loader library that has no published hash is trusted on first use…" | TM C6 and its residual |
| open: "A signature fixes which jars run, by name, size and SHA-256. It says nothing about what they do…" | README "Signed profiles" (`profile lock`); TM C2; TM §2, M and S |
| open: "Modrinth's hash and file come from the same party: a compromised project account, or the registry, serves a bad jar with a matching hash. A build younger than 48 hours is passed over, which is a delay, not detection" | TM C8 and C9, residuals † (two) |
| open: "The one mod that runs without being in your manifest is the launcher's own companion mod, part of the launcher binary (block 07)" | README "Signed profiles" (last sentence) †; README "The companion mod"; TM C2 |
| open: "Other software running as you can add a signing key of its own unless root-owned keys exist…, and the file that remembers a manifest's serial is writable by you" | TM C4, C3 (residuals); README "Signed profiles" |
| link "Threat model 4.4, rows C1 to C12" · "Mods" | TM §4.4; `mods.html` |
| **02** "bubblewrap namespaces, then Landlock, then a seccomp allowlist, inside a systemd scope with memory and task limits" | README "The sandbox" |
| "With the default `audio = "pulse"`, the deliberate holes are one audio socket, the GPU and one display socket…; Setting `audio = "none"` removes the audio socket, and `audio = "alsa"` hands the game the raw sound devices instead" | README "The sandbox", "Sound"; TM §3, D3 † (three) |
| "In a Fabric profile the sandbox also gets one socket to the launcher, for the companion mod" | README "The sandbox" †; TM §3 |
| in force: empty root, synthetic `/etc`; writable `saves/ logs/ config/`; sealed read-only mounts; scratch directory; allowlisted system calls; tripwire; no shell; no terminal, D-Bus, portal or agent sockets; no keyring | README "The sandbox", "Layout (XDG)"; TM J1, J2, J3, J4, J6, J7, J8, J9 |
| "Once the game runs, the resource limits are read back from the kernel: if they are not in force, the launch says so" | README "The sandbox" (Limits) † |
| open: kernel and GPU-driver bugs; scripts in `config/`, worlds in `saves/`, `--burner`; the limits, `saves/`, CPUs; host directory names | TM §5.5, J4, D4; J2 and README "The sandbox"; §5.6; J1 |
| link "Threat model 4.2, rows J1 to J17" | TM §4.2 |
| **03** "In the default, filtered network mode the sandbox's network namespace has no route out. The only exits are sockets held by the launcher" | TM J10; TM J13 † |
| in force: DNS responder, relay to Mojang on 443 with SNI and to allowlisted servers; servers allowlisted, `--join` | TM J10, J11; README "Network filtering and the credential broker" |
| "Voice chat is the only UDP the launcher relays out of the sandbox, and only to a port that a server's rule names, only while a game connection to that server is open and only to the address that connection uses" | README "Network filtering and the credential broker", "Voice chat on a server" † (two); TM J20 |
| placeholder, broker, explicit list; skin, cape and name refused; token never on a command line or in the environment | README "Network filtering and the credential broker"; TM J13, A5, P1, P2 |
| "Realms is refused unless you choose how its joins are approved. Then the broker also answers for Realms, and every join waits for an approval you give outside the game, typed into the phone, before the game gets one connection to that Realm's server" | README "Network filtering and the credential broker" †; TM §5.7 † |
| open: "Code in the game can still ask for what is switched on: the account reads, and one join per game connection it opens to an allowlisted server. That join is not tied to that server: a mod that learns the hash another server is waiting for can authorise a login as you on that other server, once for every game connection it opens" (until 2026-11-14 this item said the opposite, without a tie) | TM J13, residual † (two); §5.3 |
| open: `open` network mode or the broker off: the token in the game; the launch log and the front end say so | TM J13 (residual); §5.3 |
| open: "A server you allowlisted sees whatever the game sends it, and with a voice rule whatever datagrams it sends to that server's voice port…" | TM J10, residual †; §5.8; J20 |
| open: "With Realms on, code in the game can accept an invitation someone sent you, redeem an invite code it knows, agree to the Realms terms in your name and read your Realms list" | TM J21, residual † |
| link "rows J10 to J13 and J20 to J23" · "The token path" | TM §4.2; `token-injection.html` |
| **04** "The launcher is the part that holds the tokens. It gives up what it no longer needs, and it keeps the window out of its own process" | TM §3; README "The graphical front end" |
| "Before it reads anything the game sends, the launcher gives up, on all its threads, writing anywhere but the instance's settings directory, its cache and the directories of the phone's services that are on; program execution; most of the network; and the kernel keyring where the refresh token rests" | README "The launcher's own confinement" † |
| non-dumpable, locked self-wiping memory, `memfd_secret` | TM §3, P6; §7 E14 |
| "The graphical front end is a separate program that holds no token. It starts the launcher and reads its progress events; the protocol between them has no field that could carry a credential, and the launcher binary links none of the front end's dependencies. The one secret it is given is the code of a Realms join that waits for your approval, which is there to be shown to you" | TM §3, last paragraph †; G1; README "The graphical front end" |
| "If the window is closed or the front end dies, the launcher notices and shuts the game down" | README "The graphical front end"; TM G4 |
| open: "All of the self-confinement needs Landlock ABI 9, and its file and network part ABI 8. On an older kernel it is partial, and the launcher says what is missing; `bastion-mc doctor` says…" | README "Requirements" †; README "The launcher's own confinement" |
| open: "With the broker, the launcher holds the access token for the whole session, and with the phone's services also a Spotify access token and, in `totp` mode, the Realms TOTP secret…" | TM §6 (the row "Launcher holds no credential…") †; P6 |
| open: other software running as you; out of scope | TM §5.1, P7; README "Where the refresh token rests"; TM §2 |
| open: whatever can click in the window | TM G1, G6 (residuals) |
| links "row P6" · "J18" · "4.5, the front end" | TM |
| **05** "X11 lets any client log keys…" | README "Display isolation", word for word |
| "On an X11 session the game runs inside gamescope…; gamescope would bridge the clipboard, so the launcher loads a cut-off into it, which reports from inside the compositor; without that report the launcher says that the clipboard is not known to be cut. Without gamescope the launch is refused, unless…" | README "Display isolation" †; TM D1 |
| "On a Wayland session the launcher asks the compositor for a socket that is marked as sandboxed (`wp_security_context_v1`), on which a compositor is expected to withhold…; the protocol only says "should", so the launcher lists what is offered on that socket itself and says so if a privileged interface it knows is still offered. According to the protocol registry's support table, read on 2026-11-13, KWin…" | README "Display isolation" † (two); TM D2 |
| "A sound server that loads modules for its clients is a way out of the sandbox… with `audio = "pulse"`, the launcher's default, it asks your sound server before each launch…" | README "Sound" †; TM D3 |
| open: Mutter, Weston, Wayfire; the session's socket, `require` | README "Display isolation"; TM D2, residual |
| open: sway only; playability inside gamescope | TM D2 and D1 † (two); README "Status" |
| open: a bug in gamescope or Xwayland; `allow_x11` | TM D1, residual; §5.2 |
| open: the ordinary clipboard on Wayland | TM D2, residual |
| open: "With sound enabled, code in the game can record the microphone. Getting a recording out still has to pass the network filter, and where a server rule names a voice port, recorded sound can leave as datagrams to that server. The sound server's answer is checked at launch, not continuously" | TM D3, residual; §5.4 † |
| open: "`audio = "alsa"` hands the game the raw sound devices: playback and capture of every sound card, with no server to ask and no module-loading check, the most access of the three" | README "Sound" †; TM D3 |
| links "Threat model 4.3, rows D1 to D4" · "Manual, display isolation" | TM §4.3; README "Display isolation" |
| **06** "An account is more often lost somewhere else…" | TM §4.6, introduction, word for word |
| profile canary | README "The profile canary"; TM A1 |
| "**Scram.** `bastion-mc scram` stops running games and removes every credential the launcher keeps from this machine in one step: the sign-ins, the Spotify token, the Realms TOTP secret and the phone browser's profile. It says what it could not remove, and it needs no network" | README "Emergency: canary and scram" †; README "If you think the account is compromised"; TM A2 |
| runbook; proofs | README "If you think the account is compromised"; TM A3, A4 |
| open: canary limits; scram revokes nothing, no secure wipe, copied tokens, 24 hours; "Nothing is revoked at Spotify or at the sites the phone's browser was signed in to either"; runbook links; nothing revokes a session | TM A1 †, A2 † (two), A3 †, §5.10 |
| **07** "To Fabric profiles of the ten Minecraft versions it was run with, the launcher adds its own companion mod, for one session at a time, as two read-only files outside `mods/`, which stays exactly what you signed; `force = true` adds it to the other versions from 1.20.1 to 26.3. It brings the in-game phone, a redone title screen and darker menus. The mod runs inside the game and is not a security feature: the launcher does the deciding" | README "The companion mod" † (three) |
| in force: "Every command from the phone is parsed strictly, routed to an app of this launch by its exact name, refused unless its verb is in that app's fixed vocabulary, and limited per app and verb, and in all per second. No argument is ever used as a path, an address, a command line or code" | README "The in-game phone" ("What the game can see and do through the phone") †; TM H1 |
| in force: "What the game cannot get through the phone: a token, a cookie, the browser's profile, the TOTP secret, a path, a web address or a program to run" | README, the same list, word for word †; TM H3 |
| in force: "The services run outside the game's sandbox. The browser, the D-Bus proxy for the Spotify app, the clip recorder and the Realms dialer are started before the launcher confines itself, each in a sandbox of its own; the Spotify token keeper, which holds the Spotify refresh token so that the launcher does not, runs in none. The game reaches none of these programs" | README "The phone's helpers" † (three); TM §3, H19 |
| in force: "The browser shows only the sites you list, each held to its budgets. You sign in to them outside the game, with `bastion-mc browser login <site>`" | README "Discord and the browser"; TM H8, H9, H13 |
| open: "Whatever is shown on the phone or typed into it is visible to every mod in the game. Type no password into the phone" | README "The in-game phone" † (two); TM H3, H4, §5.14 |
| open: "Whatever an app allows, any code in the game can do, at the rate the launcher allows: …" | README, the same sentence, word for word †; TM H2 |
| open: "Code in the game of one instance can change the synced settings of your other instances…" | README "Settings sync" †; TM J24 |
| open: "The services that need an account of yours… have not run against the real services" | README "Status" (first paragraph, and the rows of the fourth round) |
| links "Threat model 4.7, rows H1 to H23" · "The phone" · "The companion mod" | TM §4.7; `phone.html`, `companion.html` |

#### What it does not defend

| On the page | Source |
|---|---|
| label "Read this before relying on it" | TM, the heading of §5 |
| the sixteen numbered items | include: TM §5, complete |
| "The codes in brackets name rows of the threat model, for example P7, reading the game's memory; D1, X11; J13, the access token inside the game; D3, audio; J20, the voice relay; and H8, the phone's browser. §4.6 is its section on the account itself" | TM rows P7, D1, J13, D3, J20, H8; heading of §4.6 |
| "What to do about other software running as you is the subject of the dedicated-account guide" | README "Documentation"; TM P7, residual |
| "Out of scope: …" (now with the services the phone reaches) | include: TM §2, paragraph "Out of scope:" |

#### The front end

| On the page | Source |
|---|---|
| "The launcher is a command-line program" | TM §6, last row ("a plain CLI"); README "The graphical front end" |
| "`bastion-mc-gui` is an optional window around it, of its own design: a dark instrument panel with a ring gauge around the launch control, and no advertising, news, shop or telemetry" | README "The graphical front end", first paragraph † |
| "A rail leads to five pages: the launch, with the instance, the stages of a launch and, once the game starts, which protections that launch runs with; the accounts, with sign-in; the instances; a console with the launcher's, the relay's and the game's output; and security, with the emergency panel" | README "The graphical front end" (the rail; Launch, Accounts, Instances, Console, Security) † |
| "While a Realms join waits for your approval, the launch page shows its prompt" | README, "Launch" † |
| "The phone's own commands are run in a terminal: the window offers none of them" | README "The graphical front end" ("It offers none of the fourth round's commands…") † |
| "These pictures are not captures of a session… its tests draw every state of the window in software from fixed test data. No game was running and nobody was signed in…" | README "Tests" (the snapshots); README "Status", row "Graphical front end"; `gui/src/view/tests.rs` (the snapshot test) and `gui/src/app.rs` (its fixtures) |
| the four `alt` texts and captions | the committed pictures (`shot-security.png` refreshed and looked at on 2026-11-14, its `alt` text with the recovery pages' new line and "no report yet for any of the five"; the other three unchanged since 2026-11-13); what the views are for: README "The graphical front end", "Sound", "Accounts", "Where the refresh token rests", TM G6 |
| caption of `security`: "… then the recovery pages, the only addresses its emergency panel opens. For signing in, the window also opens links to four Microsoft sign-in hosts" | TM G6 †, A3; TM G3 † |

#### What has been tested, and what has not

| On the page | Source |
|---|---|
| "This is the 'Status' section of the manual, word for word. Each row of the table first names what it is about, then says how that was checked, and what was not" | a statement about the include below it; the shape of the README's "Status" table |
| the paragraphs and the table of "Status" | include: README "Status", complete (its `<!-- COUNTS -->` comments are left out, as on GitHub) |
| "**Not exercised:** …" | include: TM §7, paragraph "Not exercised:" |
| "In the threat model's attack surface matrix, the column 'Verified by' names the test or the experiment behind a row; a few rows name none" | TM §4, first line; "a few rows name none": N2, G5, H4, H5 ("—"), H14 (from the design), D4 ("No test of its own"), J17 ("best effort; not measured") |
| "How to run the tests yourself is in the manual" | README "Tests" |

#### How to get it, and where to read on

| On the page | Source |
|---|---|
| "bastion-mc is built from its source on Arch Linux with `cargo`. The `PKGBUILD` in the repository is a template until there is a release: `makepkg` builds whatever its `source=` line points to, not the checkout it lies in" | README "Requirements" (the sentence on the PKGBUILD) † |
| the requirements | include: README "Requirements", complete |
| "Signing in needs an Azure application that you register yourself… which can take days. Until then `bastion-mc launch <id> --demo` starts Mojang's demo mode, with no account and no token" | README "Your own OAuth application" † |
| "Get started: Requirements, building with `cargo`, the decisions about display and sound, your own sign-in application, the first commands, including a demo launch that needs no account, and a signed mod profile with the companion mod" | the sections of `getting-started.html` |
| "Security overview: … what the phone can and cannot do …" | the sections of `security.html` |
| "Manual: the repository's README as a page: a demo launch, each layer of protection, the front end, mods, the companion mod and the phone, Realms, settings sync, accounts, the emergency steps, the configuration and the tests" | the headings of README.md |
| "Threat model", "Token path" | TM, first paragraph; README "Documentation"; TM P1, P2 |
| "Dedicated account: … Neither has been exercised on the development machine" | README "Documentation"; TM P7, residual; TM §7 "Not exercised" † |
| "The phone: the in-game phone and its services: Spotify, Discord and the browser, clips, Realms approvals and settings sync, their settings and limits, and what the game can see" | `docs/PHONE.md`, first paragraph and headings; README "Documentation" |
| "Companion mod: the launcher's own Fabric mod: what it adds on which Minecraft version, its settings, its channel to the launcher, how it is built and what was verified" | `docs/COMPANION.md`, headings |
| "Mods: signed profiles, fetching mods from Modrinth with verified hashes, what Modrinth learns, ViaFabric, and the server rule that lets voice chat through" | `docs/MODS.md`, title and headings |
| "The emergency page is the runbook the launcher carries…" | README "If you think the account is compromised"; TM A2, A3 |

### Getting started, the security overview, the error page

Sources: **R** = `README.md` (section in quotes), **T** = `docs/THREAT_MODEL.md` (section
or row), **H** = the `HELP` constant in `src/cli.rs`, **C** = `docs/COMPANION.md`.
Everything on the three pages is listed; a sentence that is only a link or a heading is
not. **†** = a `{{REQUIRE}}` with the source's words stands next to the statement in the
page; "included" = the generator copies the text from the document. Brought up to the
documents of the third and fourth rounds on 2026-11-14.

#### `getting-started.html`

| Where | What the page says | Source | |
|---|---|---|---|
| lede | pre-release software for Arch Linux; built from source | R first line; R "Status" | |
| lede | the sandbox can be tried in Mojang's demo mode before any account | R "Your own OAuth application" step 4; H (`--demo`) | † (under the steps) |
| lede | sign-in needs your own application; Mojang's approval can take days | R "Your own OAuth application" | † (under `#oauth`) |
| callout | "Built and tested on Arch Linux (…)" (now with the phone's Firefox, ffmpeg and xdg-dbus-proxy) | R "Status" | included |
| callout | sign-in chain and what needs an account: implemented, tested against scripted replies and a stand-in Mojang, not yet run against the live services; nor the phone's services that need an account of yours | R "Status", the sign-in row; R "Status", first paragraph | † (two) |
| callout | not exercised, among other things: NVIDIA, a real Wayland desktop session, Java 8-era versions, the real `age` program and its plugins, coverage-guided fuzzing; the security overview has the threat model's whole list | R "Status", rows "NVIDIA…" (the whole row) and "Display isolation"; T section 7 | † (the whole row, into its second cell; and the Wayland row) |
| Requirements | kernel 6.12 or newer, `landlock` in the LSM list, unprivileged user namespaces, Arch defaults; the launcher's own confinement needs Landlock ABI 8 for files and network and ABI 9 for all of it (partial, and said so, on an older kernel); ABI 10, Linux 7.2, adds UDP rules | R "Requirements" | † (two) |
| Requirements | bubblewrap; the Java release each game version declares; never downloaded, root-owned under `/usr/lib/jvm`; for Java 17 and later only from a jdk package (`jdk17-openjdk`, `jdk21-openjdk`, `jdk25-openjdk`), not a jre package; `jre8-openjdk` for Java 8; 26.3 needs Java 25; `match = "newer"`; the refusal message still suggests the jre package | R "Requirements"; T row C5 | † (six) |
| Requirements | gamescope on X11; for the phone `xdg-dbus-proxy`, Firefox, ffmpeg; systemd user session gives the limits; cargo, a C compiler, a JDK of release 17 or newer (the companion mod is compiled with `--release 17`) | R "Requirements" | † (phone, session, building) |
| Requirements | the three `pacman` lines | R "Requirements", copied (the first line's "see below" reads "see above" here) | |
| Build | build with cargo; `export PATH=…`; `bastion-mc --help` lists every command and option; the launcher is `target/release/bastion-mc`; the commands call it by name, so put the directory on `PATH` or call it by its path | R "Requirements" (the second code block and the sentence before it) | † |
| Build | `--workspace` also builds the front end | R "The graphical front end" (code block) | |
| Build | "The Arch packages are a template": two packages; until there is a release a template that does not build your checkout, and `makepkg` builds whatever its `source=` line points to; on the development machine both built from a tarball of the tree, `check()` passes, the binaries pass the hardening check | R "Requirements" (the PKGBUILD sentence); R "Source map" (`packaging/arch/PKGBUILD`); R "Status", row "Arch packages" | † (two) |
| Build | the settings file does not exist until you create it; `config example`, `config path` | R "Requirements"; H | † |
| Display and sound | a launch can be refused for either reason; `doctor` shows the display isolation | R "Display isolation" ("Without gamescope the launch is refused"; the last paragraph); T row D3, E10 | † |
| Display panel | what X11 lets a client do; gamescope; refused without it unless `allow_x11 = true`; Wayland socket marked as sandboxed; without the protocol (GNOME) the session socket, or `require` refuses; the manual's "Display isolation"; only sway; playability inside gamescope not assessed | R "Display isolation"; R "Status", row "Display isolation"; T row D1 | † (five) |
| Sound panel | module loading by default leads out of the sandbox; with `audio = "pulse"` the launcher asks before each launch and refuses the socket unless the server says no; the four commands; `pactl load-module` stops working; `audio = "none"`, `allow_pulse_module_loading = true`; `audio = "alsa"`: the raw sound devices, playback and capture of every sound card, no server to ask, no module-loading check, the most access of the three; with sound on the game can record the microphone; the manual's "Sound" | R "Sound", copied; T row D3 | † (R three, T two) |
| Sign-in application | no shared client ID, by design; no central anything; your Azure application; the four steps; `config example`; tested against scripted replies only, not yet run against the live services | R first paragraph; R "Your own OAuth application"; R "Status"; T section 7; H | † (five) |
| First commands | the first three steps need no account and work before Mojang's approval | R "Your own OAuth application" step 4; H (`--demo`) | † |
| step 1 | `doctor` | H; R "Requirements" | |
| step 2 | `versions`; `instance new main --minecraft 26.3` creates a vanilla instance | H; R "First run" | |
| step 3 | `launch main --demo`: demo mode, no account, no token; the first launch fetches and checks every file; the launch says what it set up and saw in force; Ctrl-C stops the game; on the development machine run vanilla and as a signed Fabric profile | H; R "First run" (three sentences); R "Status", row "Real Minecraft …" | † (four) |
| step 4 | `login`, `--device`, `status`; needs the approved client ID; not yet run against the live services | R "Signing in and playing"; R "Status"; H | † |
| step 5 | `launch main`, `--join`; servers allowlisted; joining a real server not exercised; the relay tested with a stand-in server and live TLS to Mojang | R "Network filtering and the credential broker"; T section 7 | † (two) |
| Mods | mods run only in a signed profile, a Fabric (or Quilt) instance whose `mods/` a manifest you sign describes; the sequence from a key to a launch | R "Mods" (first paragraph); R "Signed profiles" (the code block, copied) | |
| Mods | `profile add` takes the newest release for the profile's version and loader, with what it requires, and checks size and hashes before anything reaches `mods/`; nothing is signed; at launch the signature and every jar are verified, one extra, missing or modified file aborts | R "profile add and profile check"; R "Signed profiles" | † (three) |
| Mods | a voice-chat mod needs a rule that names the server's voice port, or it stays silent | R "Voice chat on a server" | † |
| Mods | the companion mod added by itself to Fabric profiles of the ten versions it was run with; `force = true` for the others from 1.20.1 to 26.3; press R in a world or on the title screen; the phone shows the apps set up for this launch | R "The companion mod"; R "The in-game phone" | † (three) |
| Mods | out of the box the Spotify desktop app over D-Bus, its album art and settings sync are on; the browser, the Web API, clips and Realms approval off; sign in to sites outside the game with `browser login <site>`; type no password into the phone: every mod can read it | R "Configuration" (the defaults); R "The in-game phone" | † (two) |
| Front end | a rail with five pages and what each does; the Realms prompt, in `code` mode with the code; the phone's commands in a terminal, the window offers none | R "The graphical front end" | † (three) |
| Front end | the two build/run commands; finds `bastion-mc` next to itself, else `/usr/bin`; only the Rust toolchain; the five `pacman` lines; software rendering, `ICED_BACKEND=tiny-skia` | R "The graphical front end" | † (finds …) |
| Front end | separate program: tokens stay in the launcher, no field for one, no shared dependencies; the window never waits; a closed window or dead front end stops the game; a scram is the exception | R "The graphical front end" (the three bullets) | † (the scram) |
| Front end | not covered: the window can be made to click (launch, sign out, scram); other software or any X11 client can read the sign-in code off it, and a Realms approval code while a join waits | T rows G1, G6, G5 (residuals) | † (four) |
| Front end | an `age` identity that asks for a PIN on the terminal cannot be unlocked from the window | R "The graphical front end", last paragraph | † |
| Front end | tests without a display, in software and in iced's emulator against a stand-in launcher; a real demo launch through the backend and the window's own controls; the real window started and closed on a private X11 display; no click in a real window, not on Wayland, the Realms prompt never shown for a real join | R "Status", row "Graphical front end" | † (four) |
| Where to go | security overview, manual (descriptions); emergency: scram removes every credential the launcher keeps, revokes nothing at Microsoft, the runbook is the second step; the phone, the companion mod, mods (descriptions) | the pages' own sections; R "Emergency: canary and scram", "If you think the account is compromised"; the documents' headings | † (two) |

#### `security.html`

| Where | What the page says | Source | |
|---|---|---|---|
| lede | the game is treated as hostile code, from its first instruction, through a mod or through what a server sent; so are the companion mod and its phone, which run inside it | R first line; T section 3 ("and so is the launcher's own companion mod … and with it the in-game phone"); T section 2, M and S | † |
| intro | the threat model names defence and what remains, and for most attacks the test or experiment | T introduction; T section 4 | |
| callout | the cited tests pass on the development machine, an Arch Linux system | T introduction | † |
| What is being protected | the refresh token, the access token, the signing key, files and session, the integrity of what runs (with the companion mod in Fabric profiles); the phone browser's site sessions; the Spotify refresh token (with the Web API; playback state and control, nothing of the library); the Realms TOTP secret (with `totp`) | T section 1 | † (six) |
| Adversaries, M | arbitrary Java and native code in the game from the first instruction | T section 2 | † |
| Adversaries, S | data that triggers code execution; same capability as M; patched logging configuration, lookups off; deserialisation filter, mods with their own filter remain | T section 2; rows J14, J15 | † |
| Adversaries, N | capability and limit; TLS 1.3, fixed hosts, no redirects, no plaintext; pins; what remains | T section 2; rows N1, N2, N3 | † (six) |
| Adversaries, C | hash chain; signature over the exact list of mods; a Modrinth mod installed only if it matches the size and hashes the registry lists; what remains: SHA-1, trust on first use, the hash and the file from the same party, crates.io, other software running as you | T rows C1, C2, C8, C6, C7; C2 to C4 (residuals, in the verified-files panel) | † (four) |
| Adversaries, O | world-readable things; no program arguments, nothing secret in the environment; non-dumpable; "none for O" | T section 2; rows P1, P2, P6 | † |
| Adversaries, W | any content and script in the phone's browser on the hosts its site lists, with an exploit code as you in the browser's sandbox; the browser headless in a sandbox of its own, outside the game's, without home directory, display, D-Bus or sound, out only through the launcher's relay, which answers only for the open site's hosts; what remains: a compromised browser changes its own profile, which persists; its filter is a deny-list, more of the kernel in reach than the game's allowlist | T section 2 (W); rows H9, H11 (defence and residual) | † (three) |
| Adversaries, U | not defended: game memory, keyring, launcher binary, `config.toml`; some things hold; the remedies outside the launcher, neither exercised | T section 5 item 1; rows P6, P7; section 7 "Not exercised"; `docs/MC_VAULT_ACCOUNT.md` | † (four) |
| Out of scope | root, kernel, firmware, physical access, Mojang or Microsoft acting maliciously; the services the phone reaches treated like Mojang's | T section 2 | † (two) |
| Layers, intro | namespaces, Landlock, seccomp allowlist, systemd scope; layers tested alone; the launcher is the trusted part | R "The sandbox"; T rows J7, J4; T section 3 | † (three) |
| Layers, figure | `alt` (the corrected confinement label, "holds no token") and caption: the four frames; the holes; the audio socket with `pulse`, removed with `none`, the raw sound devices with `alsa`; a launch without the companion mod: in a Fabric profile two read-only files and one socket to the launcher, and the phone's helpers beside the launcher, outside the game's sandbox | the two drawings as committed (looked at on 2026-11-14); T section 3, J2, D3; R "Sound", "The sandbox" | † (in `#holes` and under the figure, two) |
| Verified files | Mojang's files against Mojang's hashes, mods against your manifest; hashed from the descriptor that is bound in | T rows C1, C2, C5; R "Verified downloads and signed profiles" | † |
| Verified files, stops | tampered files; unsigned, modified or extra mods; a Modrinth jar that is not the one the registry lists (size and hashes from its answer, bytes from its file server only, nothing installed unless every file matches; nothing counts until you sign); a malicious Java runtime | T rows C1, C2, C8, C5 | † (three) |
| Verified files, does not stop | what a signed mod does (adversary M); a bad jar with a matching hash from a compromised project account or the registry, a build younger than 48 hours passed over, a delay and not detection; the companion mod, the one jar not in your manifest, part of the launcher binary, named with its SHA-256 in every launch log; other software running as you (keys, the serial's file, fs-verity) | T section 2 (M), rows C8, C9, C2 (the companion, residual), C4, C3, C2 | † (eight) |
| Namespaces | what is put into the empty root, in a Fabric profile also the companion mod as two read-only files | T section 3 (drawing); row J2 | † |
| Namespaces, stops / does not stop | home, `/run`, `/var`, `/opt`, real `/etc`; per-launch directory, sealed mounts; own process numbering, no D-Bus, portal or agent socket; host directory names; scripts in `config/`, worlds in `saves/`, `--burner` | T rows J1, J2, J9 (and residuals) | † (two) |
| Landlock | only the listed trees; only the Java runtime and the loader executable; no links, FIFOs, sockets, devices; signals and abstract sockets; TCP to allowlisted ports, with ABI 10 UDP to the launcher's DNS port and, if a server rule names UDP ports, to the launcher's relay sockets on exactly those ports; native code in the JVM remains | T rows J1, J3, J2, J9, J10 | † (two) |
| seccomp | allowlist, everything else answered as not existing; the listed calls; x32; keyring in two layers; no terminal; tripwire; bugs in allowed calls and the graphics interface remain | T rows J4, J5, J7, J8, J6 | † |
| Network | filtered mode is the default; private namespace, no route; exits are launcher-held sockets; names, relay targets, matching server name; DNS never forwarded; private targets refused | T rows J13, J10, J11, J12; section 6 | † |
| Network, stops | voice chat as a way out: UDP only where a server's rule names UDP ports, only to such a port of that server, only while a game connection to it is open, only to that connection's address; without a rule no UDP socket | T row J20 (defence) | † (three) |
| Network, does not stop | covert channels to an allowlisted server, in datagram form with a voice rule; code in the game opening a game connection by itself and keeping it, and sending datagrams within the limits; a request inside TLS naming another host | T rows J10, J20, J12 (residuals) | † (three) |
| Network, the price and the modes | LAN discovery and everything else over UDP do not exist; Realms joins refused unless you choose how they are approved; `none`: a namespace with nothing in it, as a burner session has; `open`: the host's network, no relay, no allowlist, no broker, the token in the game, the host's loopback reachable; nothing of the panel claimed for `open` | T section 5 item 7; R "Network filtering and the credential broker" | † (two) |
| systemd scope | memory ceiling, no swap, task limit, optional CPU quota; from the systemd user session; read back from the kernel once the game runs: if not in force the launch says so and does not show them as on, and `sandbox.cgroup = "require"` stops it; killed inside its own group; `saves/`, scratch and CPUs | R "The sandbox" (Limits); R "Requirements"; T section 5 item 6 | † (two) |
| The launcher | locked self-wiping memory, non-dumpable; before it reads anything the game sends it gives up writing (but the settings directory, its cache, the phone's directories), reading (but `/etc`, `/usr`, `/proc`, the resolver's configuration), execution, most of the network and the keyring, on all threads; all of it needs Landlock ABI 9; on an older kernel it applies what it can and says what is missing, `require` refuses | R "The launcher's own confinement"; T section 3, P6 | † (two) |
| The launcher, stops | attaching or dumping; no token on the command line, in the environment or a core dump; a bug in what it reads from the game or the helpers, read only after it has confined itself; the front end a separate process without tokens | T rows P6, P1, P2, P8, J18, G1 | † |
| The launcher, does not stop | root, kernel code, cold boot; what a launcher taken over still has: the session's token, UDP, TCP to any host on the allowed ports, signals to all your processes; with the phone's services the browser's remote control, a Spotify access token, the TOTP secret in `totp` mode, up to 8 connections through the dialer; the window made to click; the sign-in code read off the window, useless without your credentials, unlike a Realms approval code | T rows P6, J18, H23, J23, G1, G6, G5 (residuals) | † (nine) |
| Holes, intro | the setting `audio` decides the sound opening: `pulse`, the default, a server's socket after asking the server; `none`, no sound hole; `alsa`, the raw sound devices, the most access of the three | T row D3; R "Sound" | † (four) |
| Holes, X11 | what X11 allows; gamescope in a sandbox of its own; no windows or keystrokes; the clipboard: the cut-off loaded into gamescope, the cut claimed only on its report, "not known to be cut" without it; a bug in gamescope or Xwayland; another way of moving a selection, tested for gamescope 3.17 as packaged; `allow_x11` without gamescope, otherwise refused; playability not assessed | R "Display isolation"; T row D1 | † (six) |
| Holes, Wayland | a socket of its own registered as a sandbox; capture, injection and clipboard managers expected to be withheld; the protocol says "should", so the launcher lists the game's socket itself and says so, `require` refuses; the ordinary clipboard; a privileged interface not on the list; compositors without the protocol; sway only, headless | R "Display isolation"; T row D2 | † (six) |
| Holes, sound | `pulse`: Pulse-protocol socket only, never PipeWire's; module loading by default; the probe; `none`: no socket; `alsa`: the raw sound devices, no server, no check, playback and capture of every card; recording, getting it out needs the network, and with a voice rule datagrams to that server; with `alsa` every microphone directly, for machines where that is acceptable; checked at launch only; the override | T row D3 (defence and residual); R "Sound" | † (four) |
| Holes, graphics | `/dev/dri` and NVIDIA nodes, the GPU's own sysfs directory; device ioctls limited to those nodes, and with `alsa` the raw sound devices handed over as well; driver bugs; NVIDIA not exercised | T rows D4, D3; section 7 "Not exercised" | † (three) |
| Broker, before | a handful of Mojang API requests with the token; otherwise the token is in the game | R "Network filtering and the credential broker"; T section 6 | |
| Broker, how | on by default in filtered mode; per-launch placeholder; a per-session authority only in the sandbox; the list and the switches; state changes refused; skin, cape, name always refused; one join per relayed connection, own profile, body rebuilt; replies with the token withheld, and an encoded reply to a credentialed request too; one memory budget for all connections; Realms brokered the same way once approvals are chosen | T row J13; T row J21 | † (five) |
| Broker, buys | nothing in the game changes skin, cape or name or carries the token away; a placeholder in the game's memory | R "Network filtering and the credential broker"; T row P7 | † (P7, in the U panel) |
| Broker, remains | the account reads and server joins; a join spent on a connection to an allowlisted server but not tied to it: a mod that learns another server's hash can authorise a login as you there, once per connection; none without an allowlisted server; a login-protocol proxy not built; the launcher holds the token for the session, with the phone's services a Spotify access token and the TOTP secret, never a refresh token; `open` mode or broker off; not run against the real Mojang API | T row J13 (residual); section 8; section 6; section 7 "Not exercised" | † (seven) |
| Phone, intro | the companion: compiled from source with the launcher, embedded in the binary, two read-only files outside `mods/` for one session; by itself for the ten versions it was run with, with `force = true` for the others from 1.20.1 to 26.3; the phone, the title screen, the dark menus on every one of them | R "The companion mod"; C first paragraph | † (three) |
| Phone, intro | neither is a security feature; the mod enforces nothing, any mod can change, imitate or switch it off; its reports logged as the game's and never acted on; what the phone shows and what you type is visible to every mod, any code can do what the phone can; type nothing secret | R "The companion mod"; T row J19, section 5 item 14 | † (two) |
| Phone, channel | read only after the launcher has confined itself; strict parsing, routing by exact name, a fixed list of verbs, a budget per verb, all connections limited per second; no argument a path, address, URL, command line or code; the phone gets state, pictures, notices, never a token, cookie, profile or TOTP secret; does not stop: any code sending any accepted command at the accepted rate, taking the connection over, reading what is sent, drawing a fake phone or prompt; a bug in the frame reader or dispatcher, in the trusted base, small, generated input, confined | T rows H1, H3, H5 | † (five) |
| Phone, helpers | in the launcher after its confinement, and helpers started before it, each in a sandbox of its own (Firefox, `xdg-dbus-proxy`, ffmpeg, the dialer), the token keeper in none; the game reaches none of them; the keeper holds the Spotify refresh token, at most every 20 s and 60 times a session, hands out access tokens; the browser without home, display, D-Bus, sound; the proxy passes the player interfaces only; the recorder reads the display, writes a buffer, no network; does not stop: the keeper outside any sandbox; a recorder taken over reads its display, on the session's X display the whole session; a program owning Spotify's bus name shows made-up titles | R "The phone's helpers"; T section 3, rows H19, H11, H17, H21 | † (eight) |
| Phone, Spotify | the desktop app over D-Bus through the proxy; the Web API with your own application and `spotify login`; stops: anything beyond the vocabulary (six kinds of `spotify:` address, a listed device, a search only a query value, no token, URL, path or image id; local budgets, 60 calls a minute); does not stop: pause, skip, volume, open, queue, transfer; what you listen to, your devices' names | R "Spotify"; T rows H15, H3 | † (four) |
| Phone, browser | the installed Firefox, headless, in a sandbox, out only through the relay; listed sites, Discord a preset; budgets per site; sign in outside the game with `browser login <site>`; the password manager off; off by default; stops: anything but a listed site or host (lookups and TLS names); the game reading the profile, cookies, tokens, the profile outside the game's sandbox and closed to the confined launcher; does not stop: acting in a signed-in page while the browser runs; other software reading the profile; X11 programs reading the sign-in window | R "Discord and the browser"; T rows H8, H9, H12, H13; section 5 item 15 | † (eight) |
| Phone, clips | off by default; the game's window into a memory buffer; saved into your folder; no sound; no recording with a Wayland display; stops: a path (an id, caps on saves, number and size, only this feature's clips removed); does not stop: deleting this feature's clips, filling the folder to the cap, saving what the game showed; other windows in a clip on the session's X display without a compositing window manager | R "Clips"; T rows H16, H22 | † (five) |
| Phone, Realms | refused unless `[realms] approve`; the broker answers for Realms by a fixed table; a join held until you approve it outside the game (the Realm, the address, in `code` mode a code; in `totp` mode your authenticator's); one connection within two minutes to exactly the server Realms named, through the dialer; a signed-in account, the filtered network with the broker, and the phone; a launch without a phone refuses approvals up front; stops: owner operations; a join you did not approve (503, the pass leads nowhere); the token (the cookie rebuilt, other cookies and headers dropped); does not stop: invitations, invite codes, the terms, your list; in `totp` mode a code spent on another prompt; your reading of the prompt, the code on your terminal and in the front end; never run | T rows J21, J22, J23 | † (seventeen) |
| Phone, settings sync | on by default; the listed kinds of settings, translated between versions; a burner session takes no part; stops: anything but a listed setting of the right shape; does not stop: one instance's game changing the listed settings of your other instances | T row J24; R "Settings sync" | † (four) |
| Phone, closing | Spotify, the browser and clips only with a phone to serve; everything run against stand-ins; the services that need an account of yours not run against the real service | T section 4.7 (introduction); R "Status", first paragraph | † (two) |
| Not defended | the sixteen points | T section 5, items 1 to 16, same order (item 1 without the remark about the development machine's `docker` group; item 6 with the launcher's bounds summarised; item 7's UDP sentence qualified with row J10's DNS responder, see closing-1; item 10's link text "noticing and reacting" is the item's own words) | † (each point, their number and order, and the restated parts of points 1, 6, 7, 12) |
| Not verified | the list | T section 7, the paragraph "Not exercised" | included |
| Account | lost elsewhere more often; cannot prevent, can notice and make the response quick | T section 4.6, introduction | † |
| Canary | pins name, skin, cape; compared at every sign-in before the token is used; by default stops the launch until `canary accept`; visible tampering only; the pin is a file of the user | T row A1; R "The profile canary" | † (three) |
| Scram | first stops what could still store a credential (running launches with the phone's helpers, pending sign-ins, the token keeper); then every refresh token in the keyring and every vault file, in a full run also the Spotify token and the TOTP secret; every run deletes the phone browser's profile, unless a `browser login` window holds it; `COULD NOT` and exit status 1; no network; revokes nothing at Microsoft or Mojang, no secure wipe, says so; copied token, session token up to about 24 hours; nothing revoked at Spotify or the sites; the removal of the two keys never run against a real keyring | T row A2 | † (nine) |
| Runbook | steps and exact addresses built in; links can go stale; only the existence of the request form checked; a compromised machine can show a different text | T row A3 | † (three) |
| Read further | what the six documents are | the documents' own titles and first paragraphs | |

#### `404.html`

| What the page says | Source | |
|---|---|---|
| nothing is published at this address; the list of pages (since 2026-11-14 with mods, the companion mod and the in-game phone) | the site's own page list (`PAGES`, `DOCUMENTS` in `site/build.py`) | |
| one line about each page | the same descriptions as above; T introduction and section 4 for the threat model; the three documents' first paragraphs | |
| the launcher carries the same text: `bastion-mc wiki scram` | R "Documentation" ("the emergency text the launcher carries"); H | † |

### What the four pictures' texts name

When `site/update-screenshots.sh` reports "updated" for one of them, open the new picture
and read its `alt` text and caption in `site/src/pages/index.html` against it. They name
what is visible today: the instance "vanilla, Minecraft 26.3", the five steps of the
sequence, the dial reading RUNNING with the time above Stop, the five readouts marked IN
FORCE and the line "broker: the game holds a placeholder, the token stays in the
launcher" (`launch`); the refusal message about the audio socket with its three choices,
"The launch did not complete." and "no launch yet" (`refused`); three cards alt, default
and work-2 with their token stores, and the form for a new label (`accounts`); the
emergency panel with Scram and its line "deletes refresh tokens and vault files", the
recovery pages with Open and Copy under the line that besides them the window opens only
the sign-in pages a sign-in asks for, the readout of the last launch with no report for
any of the five, and the start of the runbook (`security`, refreshed on 2026-11-14).

## Appendix C: for whoever writes or changes a page

### What a page source may contain (read from `site/build.py`)

- A page source is a header, a line `---`, and the body. Header lines are `key: value`;
  `title` and `description` are required, `nav` (`start`, `security`, `manual`,
  `threat-model`, `emergency`: which navigation entry is current; any other word is an
  error since 168482e), `class` (the body class) and `footer` (`footer` or
  `footer-minimal`) are optional. The title is written into `<title>` as it is, so it
  carries its own ` | bastion-mc`. A header takes no directive: `{{` or `}}` in it stops
  the build.
- Directives in the body: `{{ROOT}}`, `{{REPO_URL}}`, `{{SITE_URL}}`, `{{INCLUDE file}}`
  (a file under `site/src/` without directives), `{{SECTION source "heading prefix"}}` and
  `{{PARAGRAPH source "text prefix"}}` (a piece of a rendered document, word for word; the
  build fails if the piece is no longer there), `{{REQUIRE source "phrase"}}` (writes
  nothing; the build fails if the document no longer says the phrase), `{{SHOT name "alt
  text"}}` (`assets/shot-NAME.png`). Any other `{{…}}` is an error, so page text cannot
  contain two opening braces.
- In a `{{REQUIRE}}` phrase a double quote is written `\"`; the phrase must not contain
  braces; `$` and an apostrophe are fine. The phrase is compared with the document's text
  as a reader sees it (no Markdown marks, white space folded; a table row's cells and a
  list's items follow each other separated by one space).
- `{{ROOT}}` is empty on every page except `404.html`, where it is the site address when
  `--site-url` is given: GitHub Pages serves that page for missing addresses at any depth.
  Every link in `404.html` therefore starts with `{{ROOT}}`; the shell and the footer do
  the same for the navigation, the style sheet and the mark.
- `--check` on a hand-written page: no HTML comments in the output (so none in a page
  source), every end tag written out, no `style` attribute, no `<a>` without `href` or with
  `target`, no block element inside `<p>`, one `<h1>` which is the first heading, no
  skipped heading level, internal links without `rel`, outbound links `https` with
  `rel="noopener noreferrer"`, every fragment must exist in the target page, every `img`
  with `alt` and its intrinsic `width` and `height` in its picture's proportions, every
  `<pre>` with `tabindex="0"`.
- The two document shells write `<h1 id="{{TITLE_ID}}">`: the anchor GitHub gives the
  title (`#bastion-mc` on the manual, `#bastion-mc-threat-model`,
  `#secure-token-injection`, `#keeping-other-programs-of-yours-away-from-the-game-ptrace_scope-and-a-dedicated-account`,
  `#emergency-runbook-you-think-the-account-is-compromised`).
- The command check reads the text of every `<code>` and `<pre>` element: each
  `bastion-mc` (also behind a path such as `target/release/`) followed by words is an
  invocation, and its sub-command and options must be the help text's for that command.
  Shell comments are skipped.
  `bastion-mc-gui` is not matched.
- Fragments that exist in `threat-model.html`: the heading anchors (GitHub's rule, for
  example `#5-not-defended--read-this-before-relying-on-it`) and one per table row,
  `#row-p7`, `#row-j13`, `#row-d1` … (`row-` and the row's label in lower case); the
  adversaries are `#row-n`, `#row-m`, `#row-s`, `#row-o`, `#row-u`, `#row-c`.

### Ids other pages may link to

`index.html`: `#layers`, `#display` (block 05), `#not-defended`, `#front-end`, `#status`,
`#get-it`, `#read-on`.
`getting-started.html`: `#requirements`, `#build`, `#display-and-sound`, `#oauth`,
`#first-commands`, `#mods` (since 2026-11-14), `#front-end`, `#next`.
`security.html`: `#assets`, `#adversaries`, `#layers`, `#verified-files` and
`#the-launcher` (two panels in `#layers`), `#holes`, `#broker`, `#phone` and `#realms` (a
panel in `#phone`; both since 2026-11-14), `#not-defended`, `#not-verified`, `#account`,
`#further`.

### What these pages depend on in other pages (a change there fails `--check`, by design)

- `manual.html#status`, `manual.html#display-isolation`, `manual.html#sound`,
  `manual.html#tests` (README headings "Status", "Display isolation", "Sound", "Tests"; the
  README's "The display" became "Display isolation" in the fourth round).
- `threat-model.html`: the heading anchors `#1-assets`, `#2-adversaries`,
  `#3-trust-boundaries`, `#4-attack-surface-matrix`,
  `#42-malicious-mod--in-jvm-code-execution-adversaries-m-s`,
  `#43-desktop-integration-the-deliberate-holes`,
  `#44-network-and-supply-chain-adversaries-n-c`, `#45-graphical-front-end`,
  `#46-the-account-itself-after-a-compromise-elsewhere`,
  `#47-the-in-game-phone-and-its-services`,
  `#5-not-defended--read-this-before-relying-on-it`,
  `#6-where-the-implementation-deviates-from-the-written-requirements`,
  `#7-experiments-behind-the-claims-development-machine`,
  `#8-hardening-what-was-built-what-is-not`, and 66 rows: `#row-a1` to `#row-a3`,
  `#row-c1` to `#row-c9` and `#row-c12`, `#row-d1` to `#row-d4`, `#row-g1` `#row-g5`
  `#row-g6`, `#row-h1` `#row-h2` `#row-h3` `#row-h5` `#row-h8` `#row-h9` `#row-h11`
  `#row-h12` `#row-h13` `#row-h15` `#row-h16` `#row-h17` `#row-h19` `#row-h21` `#row-h22`
  `#row-h23`, `#row-j1` to `#row-j15` and `#row-j18` to `#row-j24`, `#row-n1` to
  `#row-n3`, `#row-p1` `#row-p2` `#row-p6` `#row-p7` `#row-p8`.
- `companion.html`, `phone.html`, `mods.html` (the pages themselves, from the overview,
  getting started, the security overview, the error page and the footers).
- `assets/layers.svg` (960×606) and `assets/layers-stacked.svg` (328×1453 since
  2026-11-14, one line taller): the sizes are written in `index.html` and `security.html`;
  if a drawing's size changes, change them there too (`--check` fails on an `img` without
  its picture's proportions).
- The paragraphs "Built and tested on Arch Linux…" (README) and "Not exercised:…" (threat
  model), included word for word, and the 336 phrases under `{{REQUIRE}}` (Appendix A).
