# Website: notes for the user and whoever maintains the site

Written on 2026-11-13 by the deploy role of the website team, brought up to date on
2026-11-14 by the integrator who made the parts one site, by the fixers who acted on the
review of the site, by the engineer who closed the website after them (section 10 is the
short account of the review), by the engineer who brought the site up to the documents of
the third and fourth rounds after it was merged (section 11), and last, the same day, by
the documents pass of the fifth round (Minecraft: Bedrock Edition, section 12).
Everything here comes from what was run in the
working copy. "Verified" means run or looked at on the development machine. What could not
be verified there is listed in section 8, not left out. What was done and verified in
detail, what is still open, and the claim maps that tie every hand-written sentence to its
source are in the hand-over file, `docs/notes/website-progress.md`.

**State: built, never published.** The site builds from the repository's own documents and
passes its own checks. The deployment workflow has never run: publishing takes two steps
that only the owner of the repository can take (section 3).

No file of the website names the owner of the repository or contains the repository's
address: the pages write `{{REPO_URL}}` and `{{SITE_URL}}`, which the workflow fills in,
and where this file needs them it writes `OWNER` and `REPOSITORY`. Outside the website,
`packaging/arch/PKGBUILD` names both in its `url` line (marked TODO), and its `pkgbase` and
one line of `.gitignore` (the directory makepkg clones into) carry the repository's name as
GitHub spells it. Whether those stay is the owner's decision.

## 1. What exists

A static site of thirteen pages: no JavaScript, one style sheet, no request to any other host.

| Page | Made from |
|---|---|
| `index.html` (overview) | `site/src/pages/index.html`, with section 5 of the threat model, the README's "Status" and "Requirements" and two paragraphs of the threat model included word for word |
| `getting-started.html`, `security.html`, `404.html` | `site/src/pages/` |
| `manual.html` | `README.md` |
| `threat-model.html` | `docs/THREAT_MODEL.md` |
| `token-injection.html` | `docs/TOKEN_INJECTION.md` |
| `dedicated-account.html` | `docs/MC_VAULT_ACCOUNT.md` |
| `mods.html` | `docs/MODS.md` (since 2026-11-14, section 11) |
| `companion.html` | `docs/COMPANION.md` (since 2026-11-14) |
| `phone.html` | `docs/PHONE.md` (since 2026-11-14) |
| `bedrock.html` | `docs/BEDROCK.md` (since 2026-11-14, section 12) |
| `emergency.html` | `docs/SCRAM_RUNBOOK.md`, the device warning moved to the top |

`docs/README.md`, the index of the documents, is not rendered: most of its lines point to
the engineering notes under `docs/notes/`, which the site does not publish, so on the site
nearly every link of it would lead into the repository; its section on the screenshots
shows them as Markdown images, which the converter refuses on purpose (the site's pictures
are installed by `site/update-screenshots.sh` and checked by `--check`, not taken from a
document); and the site has its own index of what it publishes (the footer of every page,
the error page, "Where to read on" on the overview). The manual's link to it therefore
leads to the repository, where GitHub shows it with its pictures.

Also written: `sitemap.xml`, `robots.txt`, and `assets/`: the style sheet, the mark, the
drawing of the sandbox's layers in two forms (wide and stacked) and four pictures of the
front end.

| Part | Files | Role |
|---|---|---|
| The generator and its checks; the strict Markdown converter; the tests | `site/build.py`, `site/md.py`, `site/test_md.py`, `site/test_build.py`, `tests/site.rs` | engine (the generator and the converter were begun by two engineers before the team) |
| The look: style sheet, mark, the drawing of the layers (`layers.svg`, `layers-stacked.svg`), page shells, footers | `site/src/assets/site.css`, `site/src/assets/*.svg`, `site/src/template.html`, `site/src/document.html`, `site/src/emergency.html`, `site/src/partials/` | design |
| The overview | `site/src/pages/index.html` | landing |
| Getting started, the security overview, the error page | `site/src/pages/getting-started.html`, `security.html`, `404.html` | pages |
| Deployment, the pictures of the front end, this file | `.github/workflows/pages.yml`, `site/update-screenshots.sh`, `site/src/assets/shot-*.png`, `site/src/robots.txt`, three lines in `.gitignore` | deploy |

After the five roles had handed in, an integrator went over the whole: every page at
360, 480, 768, 1024 and 1280 px, with the changes that made the parts one site (all in
`site/src/assets/site.css`, plus a label in the tables of contents of two pages; listed in
the hand-over file). `site/CONTRACT.md` is what a page source and the style sheet agree
on: the pages, the navigation and the class vocabulary. The generator does not read it.

## 2. Preview on this machine

    python3 site/build.py                                   # writes site/dist/ (ignored by git)
    python3 -m http.server --bind 127.0.0.1 -d site/dist    # then open http://127.0.0.1:8000/

Verified in a copy of the committed tree: both commands work as written (the server was
given another port number there, so as not to collide with anyone); it listens on the
loopback address only.

- `site/dist/index.html` can also be opened straight from the file system: every reference
  between pages is relative.
- Python's server answers an unknown address with its own error page. To see the site's,
  open `/404.html`.
- `python3 site/build.py --check` also verifies the output: links and fragments, the
  Content-Security-Policy and the referrer policy on every page, nothing loaded from
  another origin, outbound links, the emergency links against
  `crates/proto/src/runbook.rs`, commands against the help text in `src/cli.rs`, a thumb of
  3 : 1 for every scrollbar the style sheet colours, the proportions of every `img`
  against its picture, keyboard focus (`tabindex="0"`) on every code block, and two runs
  byte-identical. `--out DIR` writes somewhere else. The generator empties an output
  directory only if it wrote it itself, so never put files into `site/dist/` by hand.
- A preview build writes `https://repository.invalid` where a page links to a file of the
  repository and `https://site.invalid/` into canonical links, the sitemap and
  `robots.txt`: reserved names that never resolve. A deployable build needs both
  addresses, as the workflow passes them:

      python3 site/build.py --repo-url https://github.com/OWNER/REPOSITORY \
          --site-url https://OWNER.github.io/REPOSITORY/ --check

- The tests: `python3 -I -B -m unittest discover -s site -p "test_*.py"` (the converter's
  and the generator's unit tests; the run says how many there are), and `cargo test
  --test site` (five tests: the real site built both ways with `--check`, the emergency page
  against the compiled `runbook::LINKS`, the help text the generator reads against what
  `bastion-mc --help` prints, and the unit tests again).

## 3. Publishing: the owner's steps

Nobody on the team pushed anything, changed a repository setting or ran the workflow.

1. **Switch Pages on.** On GitHub: the repository, **Settings**, then **Pages** (in the
   sidebar section "Code, planning, and automation"). Under "Build and deployment", set
   **Source** to **GitHub Actions**. GitHub then suggests workflow templates: ignore them,
   the repository brings its own. (Wording of GitHub's documentation, read 2026-11-13.)
2. **Push `main`** with the website's files. The push contains
   `.github/workflows/pages.yml`, and GitHub accepts a workflow file only from a
   credential that may write workflows: a fine-grained personal access token needs the
   repository permission **Workflows** with write access in addition to **Contents** with
   write access; a classic token needs the `workflow` scope. A push over SSH with a key of
   the account carries no such scopes.
3. **Watch the run.** The push starts the workflow "Website" (tab **Actions**): the job
   "Build and check", then "Deploy". The Deploy job shows the address of the site, and so
   does Settings, Pages. For a project site it is `https://OWNER.github.io/REPOSITORY/`.

If the push came before step 1, the run fails in its step "Configure Pages", which reads
the site's address from the Pages settings and does not switch Pages on. Do step 1, then
start the workflow by hand: Actions, "Website", "Run workflow", branch `main`.

Afterwards every push to `main` builds, checks and publishes again (section 4 says why
there is no path filter). A build that fails its checks publishes nothing; the site stays
as it was.

Worth a look once, after the first deployment: Settings, Environments, `github-pages`,
"Deployment branches and tags" should allow `main` only (GitHub's documentation recommends
such a rule; the workflow itself deploys from `main` only).

What to look at after the first deployment, because it cannot be seen from here: the
address above in a browser; any address that does not exist below it (the site's own error
page must appear, with its style sheet); the emergency page on a phone.

## 4. What the workflow does

`.github/workflows/pages.yml` is the only workflow of the repository.

- **Triggers:** every push to `main`, and a manual start. Started by hand on another
  branch it builds and checks that branch and publishes nothing. There is no path filter,
  on purpose: besides `site/`, the eight documents, `crates/proto/src/runbook.rs` and
  `src/cli.rs`, the generator checks that every file a document links to exists (when the
  site was brought up to the fourth round's documents, 2026-11-14: `LICENSE`, `TODO.md`,
  `examples/config.toml`, `docs/README.md`, the three pictures under `docs/images/`,
  `crates/proto/src/lib.rs`, `fuzz/README.md`, `java/BastionBootstrap.java`,
  `src/egress/broker.rs`, `src/mojang/args.rs`, `src/realms/table.rs` and
  `src/sandbox/mod.rs`), and a document may link to any file. Until the review the workflow had a filter that left
  those files out while its comment called the list complete (finding web-4): a push that
  renamed or deleted one of them did not start the workflow, and the published page kept a
  link that GitHub no longer served. A run on unchanged inputs builds the same pages and
  publishes them again. On the development machine the unit tests take about 13 seconds
  (7 before the three documents and their tests were added) and the build with `--check`
  under one; the runner's times are not known (section 8).
- **Permissions:** the token starts with `contents: read`. The build job adds
  `pages: read` (its "Configure Pages" step asks the API for the site's address). Only the
  deploy job has `pages: write` and `id-token: write`, and it runs no code of the
  repository.
- **Concurrency:** one run per branch at a time; a deployment in progress is never
  cancelled; of the runs waiting behind it only the newest is kept.
- **Build job**, on the image `ubuntu-26.04` (its `python3` is 3.14; nothing is
  installed): check out without leaving credentials in the working copy; run the unit
  tests of the converter and the generator; read the site's address; run
  `site/build.py --check` with the repository's address and the site's address, which
  reach it as arguments through environment variables (no expression is pasted into the
  script, and an empty address stops the run); upload `site/dist/`. The generator accepts
  only an `https` address for the site: with a custom domain on which HTTPS is not
  enforced, the build stops and says so.
- **Deploy job:** publishes the uploaded artifact to the environment `github-pages`.

Every action is pinned to the commit of a release. Looked up through GitHub's public API
on 2026-11-13, twice, by two engineers: each tag is the action's latest release, each is a
lightweight tag (so there is no tag object to dereference), and `commits/TAG` names the
same commit. Looked up once more on 2026-11-14, when the website was closed
(`releases/latest` and `git/ref/tags/TAG` of each): the same four releases are still the
latest, with the same commits.

| Action | Release (published) | Commit |
|---|---|---|
| `actions/checkout` | v7.0.1 (2026-07-20) | `3d3c42e5aac5ba805825da76410c181273ba90b1` |
| `actions/configure-pages` | v6.0.0 (2026-03-25) | `45bfe0192ca1faeb007ade9deae92b16b8254a0d` |
| `actions/upload-pages-artifact` | v5.0.0 (2026-04-10) | `fc324d3547104276b827a68afc52ff2a11cc49c9` |
| `actions/deploy-pages` | v5.0.1 (2026-09-01) | `368f82528645a54fb793d4d04e342629a3f51346` |

`upload-pages-artifact` itself calls `actions/upload-artifact` at
`bbbca2ddaa5d8feaa63e36b76fdaad77386f024f` (v7.0.0, resolved the same way). How to move a
pin to a newer release is written in the header of the workflow file.

Verified here, without GitHub: the file is valid against the public JSON schema for
workflows. The build job's own script texts were read out of the file and run on the
committed tree (taken with `git archive`, so without `target/` and without untracked
files) in an empty environment; the result was packed with the `tar` call of
`upload-pages-artifact`, unpacked below `/example-repo/`, served on the loopback address
and crawled: all 19 files answered, every reference of every page resolved below the
sub-path and none started at the root of the host, `404.html` has no relative reference
(GitHub serves it for missing addresses at any depth), and the generator's marker file is
not in the artifact. After the trigger change of 2026-11-14 the file was validated against
the same schema again, and the deploy role's structural checks were run again with the
check for the path filter reversed: both pass. The simulation ran without a problem on
`2ea3350` (the integrator's hand-in: 312 unit tests, an artifact of 19 files and 1.26 MB,
541 internal references followed), on `cc7248a` (after the content and design fixes: 542)
and on `6259e08`, the closing commit of the site (356 unit tests, 19 files, 542 internal
references followed).

## 5. The look: palette and contrast

The colours are the custom properties at the top of `site/src/assets/site.css`: near-black
gunmetal surfaces, one cyan accent (in force, selected, the way forward), amber (caution:
weaker than it could be, not yet verified) and red (danger: not defended, destructive, the
emergency). Colour never carries a meaning alone: every state also has a word on the page,
and the drawn marks differ in shape. The ratios below were computed with the WCAG 2
formula (relative luminance) by the design role on 2026-11-13 from the committed file.
WCAG AA asks for 4.5 : 1 for text and 3 : 1 for large text, for marks and for the parts of
a control that have to be seen. The integrator changed no colour. A scrollbar whose
colours the style sheet sets is part of the page, not the browser's own control, so its
thumb needs 3 : 1 against its track (WCAG 1.4.11). The review of 2026-11-14 found that pair
missing from the table and below 3 : 1: the thumbs were `line-hi` (2.12 : 1 on `well`,
2.06 : 1 on `void`; finding web-1). The design fixer made them `edge` (commit `c073a03`);
the two rows are in the table now, computed again by the content fixer from the committed
tokens.

Every text/background and mark/background pair the style sheet and the drawings use; the
"Needs" column is 4.5 for text, 3 for large text, interface edges and marks.

| Text or mark | On | Ratio | Needs | Where |
|---|---|---|---|---|
| `text` #dbe6ec | `void` #0a0d10 | 15.35 | 4.5 | body text |
| `text` #dbe6ec | `plate` #10161b | 14.35 | 4.5 | text in panels, tap targets, the folded table of contents |
| `text` #dbe6ec | `well` #05080a | 15.82 | 4.5 | code blocks |
| `text` #dbe6ec | `deck` #070a0c | 15.64 | 4.5 | footer navigation |
| `text` #dbe6ec | `accent-tint` #0e1d21 | 13.60 | 4.5 | note callout; a table row that is the link target |
| `text` #dbe6ec | `amber-tint` #1d1a14 | 13.67 | 4.5 | warning callout |
| `text` #dbe6ec | `red-tint` #1e1416 | 14.18 | 4.5 | danger callout, the device warning |
| `bright` #f2f7f9 | `void` #0a0d10 | 18.04 | 4.5 | headings, strong |
| `bright` #f2f7f9 | `plate` #10161b | 16.87 | 4.5 | headings in panels, buttons |
| `bright` #f2f7f9 | `plate-hi` #16202a | 15.26 | 4.5 | inline code; hovered button |
| `bright` #f2f7f9 | `red-tint` #1e1416 | 16.67 | 4.5 | strong and the heading in the device warning |
| `bright` #f2f7f9 | `deck` #070a0c | 18.39 | 4.5 | the name in the header |
| `dim` #9bb0bd | `void` #0a0d10 | 8.66 | 4.5 | lede, captions, quotations, table headers |
| `dim` #9bb0bd | `plate` #10161b | 8.10 | 4.5 | summary line of a panel, quiet panels, address under a tap target |
| `dim` #9bb0bd | `plate-hi` #16202a | 7.33 | 4.5 | address under a hovered tap target |
| `dim` #9bb0bd | `deck` #070a0c | 8.83 | 4.5 | header navigation, footer text |
| `dim` #9bb0bd | `well` #05080a | 8.93 | 4.5 | caption of the diagram |
| `faint` #738a9a | `void` #0a0d10 | 5.41 | 4.5 | labels, list markers, heading anchors, cell labels of stacked tables |
| `faint` #738a9a | `plate` #10161b | 5.06 | 4.5 | labels in panels |
| `faint` #738a9a | `deck` #070a0c | 5.51 | 4.5 | legal lines of the footer |
| `faint` #738a9a | `accent-tint` #0e1d21 | 4.79 | 4.5 | cell labels in a stacked row that is the link target |
| `accent` #3fd9e6 | `void` #0a0d10 | 11.39 | 4.5 | links, step numerals, read-outs, row identifiers |
| `accent` #3fd9e6 | `plate` #10161b | 10.65 | 4.5 | links in panels, the title of a tap target |
| `accent` #3fd9e6 | `plate-hi` #16202a | 9.64 | 4.5 | code inside a link; hovered tap target |
| `accent` #3fd9e6 | `deck` #070a0c | 11.61 | 4.5 | current navigation entry, footer links |
| `accent` #3fd9e6 | `well` #05080a | 11.74 | 4.5 | links in the caption of the diagram |
| `accent` #3fd9e6 | `accent-tint` #0e1d21 | 10.09 | 4.5 | label and links of the note callout |
| `accent` #3fd9e6 | `amber-tint` #1d1a14 | 10.15 | 4.5 | links in the warning callout |
| `accent` #3fd9e6 | `red-tint` #1e1416 | 10.52 | 4.5 | links in the danger callout |
| `hover` #8eeaf2 | `void` #0a0d10 | 14.10 | 4.5 | hovered link |
| `hover` #8eeaf2 | `red-tint` #1e1416 | 13.03 | 4.5 | hovered link in the danger callout |
| `amber` #f5b041 | `deck` #070a0c | 10.56 | 4.5 | the pre-release chip |
| `amber` #f5b041 | `amber-tint` #1d1a14 | 9.23 | 4.5 | label of the warning callout |
| `amber` #f5b041 | `void` #0a0d10 | 10.36 | 4.5 | status "partly" |
| `red` #ff5f56 | `deck` #070a0c | 6.64 | 4.5 | the Emergency entry of the navigation |
| `red` #ff5f56 | `red-tint` #1e1416 | 6.02 | 4.5 | label of the danger callout |
| `red` #ff5f56 | `void` #0a0d10 | 6.52 | 4.5 | status "not yet run" |
| `red-hover` #ff8d86 | `red-tint` #1e1416 | 8.06 | 4.5 | hovered Emergency entry |
| `void` #0a0d10 | `accent` #3fd9e6 | 11.39 | 4.5 | primary button |
| `void` #0a0d10 | `hover` #8eeaf2 | 14.10 | 4.5 | hovered primary button |
| `white` #ffffff | `selection` #1d545b | 8.50 | 4.5 | selected text |
| `edge` #556c7e | `void` #0a0d10 | 3.56 | 3.0 | border of a button, of the folded table of contents; thumb of the scrollbar of a wide table and of the table-of-contents rail (their track is `void`) |
| `edge` #556c7e | `well` #05080a | 3.67 | 3.0 | thumb of the scrollbar of a code block (its track is `well`) |
| `edge` #556c7e | `plate` #10161b | 3.33 | 3.0 | border of a button (its own fill) |
| `accent` #3fd9e6 | `void` #0a0d10 | 11.39 | 3.0 | focus outline, marks of a defended item, band of a note |
| `accent-edge` #2a8790 | `void` #0a0d10 | 4.61 | 3.0 | border of the note callout and of a tap target |
| `amber-edge` #976f2d | `void` #0a0d10 | 4.29 | 3.0 | border of the warning callout, of a caution panel, of the chip |
| `red-edge` #c24a44 | `void` #0a0d10 | 4.04 | 3.0 | border of the danger callout and of a danger panel |
| `red` #ff5f56 | `plate` #10161b | 6.09 | 3.0 | mark of a not-defended item in a panel |
| `amber` #f5b041 | `void` #0a0d10 | 10.36 | 3.0 | band of the warning callout |

The scroll shadow is the 28 % cyan band at the edge of a code block or a table that has
more to scroll to (`rgba(63, 217, 230, 0.28)`), blended with what it lies on. Every token
in `:root` of the committed `site.css` has the value the computation used (compared by a
script, no difference); the one other colour on screen is the selection's white.

| Text or mark | On | Ratio | Needs | Where |
|---|---|---|---|---|
| `text` #dbe6ec | shadow over `well` #154348 | 8.58 | 4.5 | code passing under the edge of a scrolling block |
| `dim` #9bb0bd | shadow over `void` #19464c | 4.62 | 4.5 | a table header passing under the edge of a scrolling table |
| `accent` #3fd9e6 | shadow over `void` #19464c | 6.07 | 4.5 | a row identifier under the same edge |
| `faint` #738a9a | shadow over `void` #19464c | 2.88 | 4.5 | never shown: cell labels exist only in records, and records do not scroll |

The two drawings (colours checked against the files: the same hex values, the four frames
filled #070b0e to #0d141a, the lightest taken):

| Text or mark | On | Ratio | Needs | Where |
|---|---|---|---|---|
| `text` #dbe6ec | `red-tint` #1e1416 | 14.18 | 4.5 | values in the game's frame |
| `bright` #f2f7f9 | `red-tint` #1e1416 | 16.67 | 4.5 | the sentence in the game's frame |
| `dim` #9bb0bd | `red-tint` #1e1416 | 8.00 | 4.5 | labels and notes in the game's frame |
| `red` #ff5f56 | `red-tint` #1e1416 | 6.02 | 4.5 | the label "The game" |
| `bright` #f2f7f9 | `plate` #10161b | 16.87 | 4.5 | names of the launcher's parts |
| `dim` #9bb0bd | `plate` #10161b | 8.10 | 4.5 | notes and values in the launcher |
| `text` #dbe6ec | `plate` #10161b | 14.35 | 4.5 | the sentence under the launcher's parts |
| `accent` #3fd9e6 | `plate` #10161b | 10.65 | 4.5 | the label "Launcher" |
| `accent` #3fd9e6 | ring #0d141a | 10.85 | 4.5 | labels of the four frames |
| `dim` #9bb0bd | ring #0d141a | 8.25 | 4.5 | values of the four frames |
| `dim` #9bb0bd | `deck` #070a0c | 8.83 | 4.5 | the label "Host", notes under the holes |
| `dim` #9bb0bd | `well` #05080a | 8.93 | 4.5 | the network line's note, the front end |
| `amber` #f5b041 | `deck` #070a0c | 10.56 | 4.5 | labels of the holes |
| `accent-edge` #2a8790 | `deck` #070a0c | 4.70 | 3.0 | the frames' lines |
| `red-edge` #c24a44 | ring #0d141a | 3.85 | 3.0 | the game's frame |
| `amber` #f5b041 | ring #0d141a | 9.87 | 3.0 | the holes' lines |
| `line-hi` #33485a | `well` #05080a | 2.12 | none | the host's frame (decoration: its label says the same) |

On paper (the print rules): text #000, secondary #222 (15.91), labels #333 (12.63), all on
#fff.

## 6. What has to be refreshed, and when

| When | What | How |
|---|---|---|
| The front end changes (its look, a text on a page the pictures show, its test fixtures) | The four pictures of the overview (`launch`, `refused`, `accounts`, `security`) and what the page says about them | `cargo test -p bastion-mc-gui`, then `site/update-screenshots.sh` (it reads `target/gui-snapshots/`; another directory can be given as its argument; it copies the files as they are and installs all of them or none). Look at every picture it reports as "updated" and read that picture's `alt` text and caption in `site/src/pages/index.html` against it. Then `python3 site/build.py --check`. Last done 2026-11-14 on the merged front end (section 11). |
| `README.md` or a document under `docs/` changes | The nine rendered pages follow by themselves. The pieces that hand-written pages include word for word (`{{SECTION …}}`, `{{PARAGRAPH …}}`) follow too, or stop the build and name the heading or paragraph that is gone: then the document was restructured, so change the prefix in the directive and read the sentences around it again. A hand-written sentence follows nothing, unless it carries a `{{REQUIRE source "phrase"}}`, which stops the build when the phrase has left the document. | `python3 site/build.py --check`. Then the claim maps in `docs/notes/website-progress.md` (Appendix B): they name the source of every hand-written statement. First the callout under the overview's title and the "Stays open" lists, against the new "Status" table and the new "Not exercised" paragraph. |
| A release is made, or binaries are offered | "Pre-release" (the chip next to the name and the footer of every page; the overview, the security overview and getting started say it too) and "There are no releases and no binary downloads" (both footers, and twice on the overview) | By hand; nothing checks these. `grep -rni -e pre-release -e 'no releases' site/src` finds them. |
| Section 3 of the threat model changes (its drawing, the launcher's parts, the holes) | The two drawings `site/src/assets/layers.svg` and `layers-stacked.svg`, their `alt` texts (on the overview and the security overview) and captions | The drawings are plain SVG with comments and can be edited by hand; the script that drew them is not committed (see section 9). They show a launch without the companion mod: the companion channel, the phone's dispatcher and helpers and the Fabric profile's socket to the launcher are left to the captions, which say so. Their words were corrected by hand on 2026-11-14 (section 11); `--check` holds the `img` sizes in the pages to the drawings' proportions. |
| Section 5 of the threat model changes | `security.html#not-defended` restates its nineteen points by hand (the overview includes them word for word) | Read that section of `site/src/pages/security.html` again; its ties fail on a new, removed or reworded point. |
| What the documents say about sound changes (`audio`: row D3, the README's "Sound") | The sound statements of the overview (blocks 02 and 05), getting started (the sound panel), the security overview (`#holes`, the sound and graphics panels) and both drawings' captions describe all three values, `pulse`, `none` and `alsa`, since the documents describe `alsa` (2026-11-14) | `grep -n 'audio = ' site/src/pages/*.html`; read each against the new text. |
| README.md's `<!-- COUNTS -->` markers are filled in or move | Nothing: the converter leaves HTML comments out, as GitHub does, and the hand-written pages carry no test counts | — |
| A new user document under `docs/` that the README links to | Whether it becomes a page: `DOCUMENTS` in `site/build.py`, both footers, `404.html`, `site/CONTRACT.md`, the page list in `tests/site.rs` and the small site in `site/test_build.py` | As `docs/MODS.md`, `docs/COMPANION.md` and `docs/PHONE.md` were added (section 11), and `docs/BEDROCK.md` after them (section 12). Until then the manual's link leads to the repository. A comment on lines of its own (`<!-- … -->`) is a raw HTML block, which the converter refuses: take it out or make it part of a paragraph. |
| The help text in `src/cli.rs` changes | Commands shown on hand-written pages | `--check` fails and names the command. |
| `runbook::LINKS` or `docs/SCRAM_RUNBOOK.md` changes | The emergency page | `--check` fails unless the page links to exactly the launcher's list. |
| An action publishes a release | The four pins | The header of `.github/workflows/pages.yml`. |
| GitHub retires the image `ubuntu-26.04` | `runs-on` in both jobs | Choose an image whose `python3` the generator was run with. |

The pictures are not captures of a session. They are test snapshots of the front end,
drawn in software from fixtures (the player name, the account labels and the instance are
test data), and the overview says so above the gallery. The ones committed are the merged
front end's, 1280×800, as its tests drew them on 2026-11-14 (section 11: three unchanged,
`shot-security.png` updated). The player name of the fixtures, which `shot-refused.png` and
`shot-accounts.png` show, is the name of one of Minecraft's default characters; the owner
decided on 2026-11-14 to publish the pictures with it. The files hold nothing but the
picture (no text, time or EXIF chunk); the script and the generator both refuse a picture
that does.

## 7. Found in the documents the site renders, and elsewhere (for their owners)

The website does not change what the documents under `docs/` and `README.md` say. Since
2026-11-14 it may correct their Markdown where the converter shows that it is wrong, and
nothing else (section 11 needed no such correction). What the site found, and what became
of it:

**Settled since the site was built** (each was in this section before the documents of the
third and fourth rounds were merged):

- **The two table mistakes in `docs/THREAT_MODEL.md`** (section 4.3 lacked its column
  "Verified by"; a blank line after row E10 split the table of section 7) were fixed in the
  document. The generator no longer repairs anything: both entries of `REPAIRS` were
  deleted, and the build prints no `note:`.
- **`open` network mode** is described now: the README's "Network filtering and the
  credential broker" says what `open` and `none` give the game, and the security overview
  says it too.
- **`audio = "alsa"`** is described now, in the README's "Sound" and in row D3 (defence and
  residual); the pages describe all three values instead of excluding `alsa`.
- **Row A3 and the front end's label** no longer say "only" of the whole window: A3 names
  the sign-in pages a sign-in asks for, the emergency panel's label in
  `gui/src/view/security.rs` says "besides them the window opens only the sign-in pages a
  sign-in asks for", and `shot-security.png` shows the new label.
- **The PKGBUILD** is described as what it is: the README's "Requirements" says that it
  does not build the checkout and that `makepkg` builds whatever its `source=` line points
  to. Whether it should build the local tree or a signed tag is still the owner's decision.

**Settled by the documents of the fourth round** (read again against the current documents
and code on 2026-11-14, section 12):

- **Realms without a phone**: the README says, as rows J21 and J22 do, that a launch without a
  phone does not broker Realms (and, since the fifth round, that a Bedrock session approves
  outside the game without one).
- **Clips in a burner session**: the README says that a burner session records no clips.
- **UDP in section 5, item 7** names the launcher's DNS responder (row J10) as well.
- **`audio = "alsa"`**: row D4 and section 3's drawing name `/dev/snd`, and
  `examples/config.toml` says what `alsa` hands over.
- **The front end and its secret**: the README's drawing says "no tokens", and section 3 of
  the threat model names the one secret (the Realms code; since the fifth round also what you
  answer a Bedrock prompt with).
- **The front end's scram line** names what scram removes since the fourth round.

**Still open, for the owner of the documents:**

- **Not every row of the attack surface matrix names a test.** In the column "Verified by",
  N2, G5, H4 and H5 have a dash, H14 says "from the design and Firefox's documented
  behaviour; not tested separately", D4 "No test of its own; real launches render through
  it" and J17 "best effort; not measured"; since the fifth round B3 is a finding read in the
  client's manifest, B39 has a dash (not the launcher's to defend) and B15 names the network
  rows' tests. The overview says "where there is one" and "a few rows name none".

**Still open, for the owner of the front end:**

- **Its Realms approval snapshot pairs the prompt with a vanilla instance**
  (`launch-realms-approval-*.png`: "vanilla, Minecraft 26.3"), a state the launcher never
  reports for a Java launch, since a Java launch without a phone refuses Realms approvals up
  front. The overview therefore shows no picture of the prompt; with a Fabric instance in that
  fixture it could (a `MENU` line in `site/update-screenshots.sh` and a `{{SHOT}}` on the
  overview).

## 8. Not verified

- **The workflow has never run.** Nothing on the development machine can run it. The first
  run after the owner's two steps is its first test. Where a problem would show: the step
  "Configure Pages" (Pages not switched on, or the token's permission), "Run the
  generator's tests" and "Build the site and check it" (the runner's Python), "Deploy".
- **`pages: read` for the build job** comes from GitHub's table of permissions
  (`GET /repos/{owner}/{repo}/pages` is listed under "Pages: read"), not from a run. If
  "Configure Pages" fails with "Resource not accessible by integration" although Pages is
  switched on, that reading was wrong: give the build job `pages: write`, which is what
  GitHub's own starter workflow gives the job that runs this step.
- **The runner's Python.** The image has Python 3.14.4 according to its description; the
  generator and its tests were only ever run with 3.14.9, and there is no other Python on
  the development machine. Between the two, the 3.14 changelog lists changes in
  `html.parser` (3.14.7: how an abruptly closed empty comment is read),
  `xml.etree.ElementTree` (3.14.6, 3.14.8) and `argparse` messages (3.14.5). By reading,
  neither the generator nor its tests reach any of them (they parse only the generator's
  own escaped output), but that is reading, not running. The files parse as Python 3.8
  and later, which says nothing about behaviour.
- **GitHub Pages itself:** that it serves `404.html` for missing addresses below the
  sub-path, and the headers it sends, are GitHub's documented behaviour, not something
  seen here.
- **Browsers.** Every page was looked at only as a rendering of headless Firefox 159:
  never in another browser, never on a phone, never with a screen reader. What was looked
  at: every page at 360, 480, 768, 1024 and 1280 px (the integrator, after the roles'
  own passes at 360 to 1920 px, and again when the website was closed); the print rules
  applied on screen at about the width of an A4 column (the runbook, the overview, the
  threat model); the pages without the style sheet; keyboard focus through a class
  standing in for `:focus-visible`, because the headless window never has focus (the
  design fixer's probe also showed that no ring is cut off by a box around it). Sideways
  scrolling was measured, not judged by eye: on all nine pages at 39 widths from 360 to
  1920 px, with and without a scrollbar, nothing is wider than its window, sticks out of
  its panel or cell, is clipped or is drawn over other text. Some rules are recent CSS
  (`text-wrap: balance`, `:has()`, labels floated in a `flow-root` cell); another engine
  may wrap differently. When the site was brought up to the fourth round's documents
  (section 11), the changed pages were looked at again at 360, 768 and 1280 px, and all
  twelve pages were measured at 44 widths from 320 to 1920 px with a harness of that pass
  that measures less than the integrator's tool did: only whether anything is wider than
  the window outside the boxes that scroll by design, not whether something sticks out of
  its panel, is clipped or is drawn over other text (the pictures were looked at for that).
- **Paper:** real pagination and the `break-inside` rules were not seen; there is no
  print-to-file here.
- **GitHub's renderer beyond what was asked.** Where CommonMark versions differ, the
  converter follows cmark-gfm 0.29.0.gfm.13, the version GitHub's Markdown API names; its
  source was read, never run, and GitHub's API answered eight probes. The anchor rule is
  taken to be "letters, marks, decimal digits, connector punctuation, `-` and space", which
  every probe agrees with; characters newer than GitHub's Unicode version would differ.
  That github.com shows a README exactly as the API renders it was not compared.
- **The documents are trusted text.** The converter's nesting limit and linear paths make
  an accident fail quickly; they were not reviewed against an adversary writing the
  documents.

## 9. For whoever maintains the site

- The website is `site/`, `.github/workflows/pages.yml`, `tests/site.rs` and the three
  lines at the end of `.gitignore` (`/site/dist/`, `/site/__pycache__/`,
  `/site/.update-screenshots.*/`).
- The generator fails closed. After merging other branches run
  `python3 site/build.py --check`. The build stops (with file and line where there is one)
  on Markdown the converter does not implement or that GitHub would read differently; on a
  heading or paragraph that a page includes and that is gone; on a `{{REQUIRE}}` phrase
  that has left its document; on a `{{` or `}}` that is not part of a whole directive, and
  on any directive in a page header; on a heading whose GitHub anchor a table row or the
  page around the document already has, a heading with raw HTML or without a letter or
  digit, and block quotes, lists, emphasis or links nested more than 32 deep. Among the
  Markdown it refuses because GitHub would show something else: a line inside a code block
  that begins like the block's own fence (a fence that lost its closing line), a table row
  indented by four or more spaces, a row of dashes that GitHub does not read as a table,
  and `:--` without pipes below a line of text. A document may show `{{` as text (it is
  published as `&#123;{`, which reads the same). An HTML comment inside text
  (`<!-- COUNTS -->` in the README) is left out, as GitHub leaves it out; other raw HTML is
  shown as the text it is.
- `--check` then fails on a link or fragment that no longer resolves, a command that the
  help text no longer has, an emergency page that differs from `runbook::LINKS`, a
  relative reference on the deployed `404.html` (GitHub serves it at any depth), a link to
  the site's own address (inside the site every link is relative), SVG animation or
  `<feImage>` in a drawing, a scrollbar thumb below 3 : 1 against its track, an `img`
  whose `width` and `height` do not have its picture's proportions, and a `<pre>` without
  `tabindex="0"`.
- When the front end changes, refresh the pictures (section 6, first row). In order, once
  everything is merged:

      cargo test -p bastion-mc-gui     # writes target/gui-snapshots/
      site/update-screenshots.sh       # then look at every picture it calls "updated"
      python3 site/build.py --check
      cargo test --test site
- Every push to `main` starts the workflow. A push that renames or deletes a file some
  document links to fails the build job, which names the link, and publishes nothing;
  `cargo test --test site` catches the same thing before a push. Do not add a path filter
  back (section 4 says why).
- `docs/notes/website-progress.md` holds the claim maps (Appendix B): the only tie between
  a hand-written sentence and its source. Keep them current when a page changes.
- `robots.txt` and its `Sitemap:` line have no effect while the site is a project site:
  crawlers read that file only at the root of a host. The file says so itself.
- Not committed, and living under `target/` of the team's working copy: the script that
  drew the two layer drawings (`target/fix-design/layers.py`, with the audio note), the
  tools that measured the pages (`target/integrate/measure.py` and the roles' earlier
  ones), the focus-ring probe (`target/fix-design/probe.py`), the simulation of the build
  job (`target/deploy-tools/`) and the reviewers' cases and GitHub's answers to the probes
  (`target/site-review-renderer/`, `target/fix-engine-scratch/github/`). Nothing in the
  build needs them; the hand-over file lists them. Take what is worth keeping before that
  working copy is deleted. The pass of section 11 worked in a clone of its own
  (`target/agents/r4-site`) and left its tools under `target/r4site/tools/` there:
  `shot.sh` (a page at a layout width, in bubblewrap without a network), `split.py` (a page
  too tall for one picture in parts), `sheets.sh` (a tall picture as sheets of strips),
  `measure.sh` (the overflow harness) and `doctext.py` (a document's text as `{{REQUIRE}}`
  sees it, and every tie of every page checked at once).

## 10. The review of the site, and what was done (2026-11-14)

After the integration the site was reviewed through three lenses, each by its own
reviewer: the **renderer** (does the converter show every document as GitHub does, or
refuse), **accuracy** (does every sentence say only what `README.md` and
`docs/THREAT_MODEL.md` say) and the **web** (accessibility, layout, the workflow, these
notes). Three fixers then worked side by side, one per area (engine, design, content),
and one engineer closed the website after them. Every finding was reproduced before it
was fixed; the hand-over file has the table with each finding, its fix and its test.

- **Renderer, 11 findings, all held and fixed** in `site/md.py` and `site/build.py`, with
  tests. The one of high severity: a tab after a closing code fence (allowed by CommonMark
  and by GitHub) paired the fences wrongly, so whole sections of a document became code
  while `--check` still passed. The others: emphasis next to symbols, tables recognised or
  ended differently, a backslash before a pipe in a cell, a rule after list items, Unicode
  white space, character references before a line end, heading anchors (the title's,
  letters GitHub keeps, clashes with table rows), deep nesting and slow paths, refusals
  that named the wrong line, and documents that could not mention `{{REPO_URL}}`. Where
  CommonMark versions differ the converter now follows GitHub's own renderer (section 8).
  On the five real documents the output did not change, apart from keyboard focus on three
  code blocks and the title's anchor on each page's `h1`.
- **Accuracy, 7 findings, all held and fixed** in the pages: the sound and graphics
  statements were true only for the default `audio = "pulse"` (the launcher also offers
  `audio = "alsa"`, which no document describes; section 7); "comfortable to play" inside
  gamescope where the documents say playability is not assessed; a caption that called the
  recovery pages the only addresses the window opens (it also opens Microsoft sign-in
  links); a heading that claimed "nothing else of the hardware"; a list of what was not
  exercised that left out fuzzing; a sentence that implied `makepkg` builds the reader's
  checkout (the PKGBUILD clones its own copy; section 7); and a sentence about the status
  table's columns that was false on a phone. The pages gained ten `{{REQUIRE}}` ties.
- **Web, 5 findings, all held and fixed:** scrollbar thumbs at 2.12 : 1 and 2.06 : 1 (now
  3.67 : 1 and 3.56 : 1, section 5, and `--check` computes every pair); code blocks that scroll on a phone without
  keyboard focus (every code block takes focus now, and `--check` requires it); the focus
  ring cut off in the table of contents beside a document (drawn inside the entry now);
  workflow triggers that missed files the generator reads (no path filter now, and a test
  keeps it so); and a sentence in these notes that said no committed file names the owner
  (the PKGBUILD and `.gitignore` do; see the top of this file).
- **Found when closing:** the security overview said "UDP does not exist in the sandbox"
  two panels after "UDP to the launcher's DNS port" (both from the threat model; section
  7); it now says that UDP reaches nothing from the sandbox but the launcher's DNS
  responder. The rest of the closing pass found nothing more: the review's own cases run
  against the final converter give GitHub's answers or a refusal, every page was looked at
  again at five widths and measured at 39, and the four action pins are still the latest
  releases.
- **Still open** then (section 7 and the hand-over file): what only the owner of the
  documents, the PKGBUILD or the front end could change (`audio = "alsa"`, row A3 and the
  front end's label, the PKGBUILD template, the two table defects, `open` network mode, the
  UDP wording in item 7), and the owner's two publishing steps. Section 11 says which of
  these the documents of the fourth round settled.

## 11. Brought up to the documents of the third and fourth rounds (2026-11-14)

The site was built and reviewed against the documents of the second pass. It was then
merged into a tree with two later rounds of features and documents: the redesigned front
end, the companion mod, the UDP relay for voice chat and verified mods from Modrinth (third
round); the in-game phone with its services, Realms with approvals, settings sync and the
companion on every Fabric release from 1.20.1 to 26.3 (fourth round). After the merge the
build stopped on 15 `{{REQUIRE}}` phrases that had left `README.md` or the threat model,
and `--check` named 9 problems that followed from it. One engineer brought the site up to
the documents, in a clone of its own; the hand-over file has the details and the new claim
maps.

- **The 15 ties.** Each statement was read against the document's current text. Twelve had
  become false or outdated and were rewritten: the launcher's own confinement needs
  Landlock ABI 8 (files and network) and 9 (all of it), not kernel 7.0 (on three pages);
  the PKGBUILD is a template that does not build the checkout (getting started no longer
  offers `makepkg` as a second way to build it); the front end's window was started on a
  private X11 display, not run on the session; the join the broker honours once per game
  connection is not tied to that server, so a mod that learns another server's hash can
  authorise a login as you there (the security overview, and block 03 of the overview,
  which had no tie and said the opposite); scram also removes the Spotify token and the
  Realms TOTP secret and deletes the phone browser's profile; and the not-defended list's
  points 1 (the age vault), 3 (a join good for any server), 4 (recorded sound as voice
  datagrams), 6 (limits read back, the launcher's own bounds, the helpers' limits) and 7
  (Realms with approval, the voice relay), with its five new points 11 to 16. Three were
  only re-quoted: NVIDIA not exercised, the broker's join and withheld replies (the
  withheld encoded reply added), and the words between points 1 and 2.
- **Beyond the ties,** every hand-written page was read against the README and the threat
  model as they are now. The overview has a block 07 on the companion mod and the phone,
  and its blocks name the mod catalog, the voice relay, Realms with its approval, the
  limits read back, the Wayland listing and the clipboard cut-off's report; the front end
  is described as the README describes it now. Getting started has Java from jdk packages,
  the phone's packages, the settings file, and a section on a signed mod profile, the
  companion mod and the phone. The security overview has the three new assets, adversary
  W, the network modes `none` and `open`, the voice relay in the Landlock and network
  panels, `audio = "alsa"` (described now, not excluded), what a launcher taken over still
  holds, the front end's Realms code, and a section on the companion mod and the phone:
  the channel and its dispatcher, the helpers outside the game's sandbox, Spotify, the
  browser, clips, Realms and settings sync, each with what it stops and what it does not.
  The pages carry 336 ties now (142 before), and no test count.
- **Three documents are pages** (section 1): `docs/MODS.md`, `docs/COMPANION.md` and
  `docs/PHONE.md`, so the manual's and the other documents' links to them land on the site.
  They convert as they are; no Markdown had to be corrected. `docs/README.md` is not
  rendered (section 1 says why). The navigation keeps its five entries: a sixth would add
  a row to the header on a phone, and the overview, getting started, the security overview,
  the manual, the error page and the footer of every page link the three pages.
- **The converter** leaves an HTML comment inside text out. The README marks the numbers
  the integrator fills in with `<!-- COUNTS -->`; GitHub shows nothing of them, the site
  showed them as text (eleven times in the status table that the overview includes word
  for word). Only a comment by the rule of GitHub's renderer is left out; anything else
  that begins with `<!--` stays text, and a comment in a heading is refused. With tests.
- **No repair.** The threat model's two table mistakes were fixed in the document; both
  `REPAIRS` entries were deleted (the functions stay for the next one), and a test holds
  that nothing is repaired and no `note:` is printed.
- **The layer drawings** said what section 3 no longer says: "once the game runs, the
  launcher confines itself (Landlock, all threads)", where section 3 now has the launcher
  confine itself before it reads anything from the game, with Landlock and a system-call
  filter; and that the front end "holds no secrets", where the event protocol now carries
  one, the code of a waiting Realms approval. Those words were corrected by hand ("before
  it reads anything from the game, it confines itself (Landlock + seccomp, all threads)";
  "holds no token"); the stacked drawing grew by one line, to 328×1453. The frames of
  section 3 did not change, so nothing was redrawn. The drawings still show a launch
  without the companion mod (no companion channel, dispatcher or helpers, and no socket to
  the launcher); both captions say so.
- **The pictures.** `cargo test -p bastion-mc-gui` (126 tests passed: 120 unit tests, 4
  emulator tests, and the 2 live tests, which are opt-in and returned at once), then
  `site/update-screenshots.sh`: `launch`, `refused` and `accounts` unchanged,
  `shot-security.png` updated (its recovery pages' line now names the sign-in pages); its
  `alt` text was read against it and follows. The snapshot of the Realms prompt was not
  added (section 7). The player name of the fixtures stays: the owner decided so.
- **Layout.** A three-column grid gives a last item that would stand alone in its row the
  whole row (the seven adversaries of the security overview). The changed pages were looked
  at at 360, 768 and 1280 px (headless Firefox 159.0.1 in bubblewrap without a network, a
  throw-away profile; the manual and the threat model at 360 px in parts, since a single
  picture of them is taller than Firefox can draw); all twelve pages measured at 44 widths
  from 320 to 1920 px, 528 measurements: nothing wider than its window.
- **Verified:** `python3 -I site/build.py --check` passes (12 pages, 916 internal links, 78
  outbound links, two runs byte-identical, no `note:`), and so does the deployable build
  with `--repo-url` and `--site-url`; `python3 -I -B -m unittest discover -s site -p
  "test_*.py"`: 361 tests (139 of the converter, 222 of the generator), OK; `cargo test
  --test site`: 5 passed; `cargo fmt --all --check`: no change. Not repeated: the
  simulation of the workflow's build job and the lookup of the action pins (the deploy
  role's tools were not in this clone, and nothing was fetched).
- **Still open:** section 7 (for the owner of the documents: Realms without a phone and
  clips in a burner session in the README, item 7's UDP wording, `alsa` and row D4, the
  front end's one secret, rows without a test; for the owner of the front end: the Realms
  prompt's fixture and the scram line), the PKGBUILD decision, and the owner's two
  publishing steps (section 3).

## 12. Minecraft: Bedrock Edition on the site (2026-11-14)

The fifth round added Minecraft: Bedrock Edition (the Android build, run by the user's
installed `mcpelauncher-client` in the launcher's sandbox) and, in its documents pass, the
README's section on it, the threat model's section 4.8 (rows B1 to B39) with additions to
sections 1, 2, 3, 5, 6, 7 and 8, the runbook's Bedrock sentences and the user guide
`docs/BEDROCK.md`. The site was brought along in the same pass; the hand-over file's section
"Round 5" has the detail and Appendix B the new claim maps.

- **A page more**: `bedrock.html`, from `docs/BEDROCK.md`, in the footers, on the error page
  and in "Where to read on" on the overview, getting started and the security overview. The
  navigation keeps its five entries. The guide's HTML comment at the top (who wrote and
  checked it) was taken out of the document: the converter refuses a raw HTML block, which a
  comment on lines of its own is. The guide converts as it is otherwise.
- **The hand-written pages**: the overview has a third lede, a sentence in the callout (Bedrock
  has not run with the real game at all) and block 08, with what is in force and what stays
  open (above all: the game holds your Xbox sign-in for the session); getting started a
  sentence in the callout and a section with the three commands, the licence switch and the
  sign-in; the security overview the new asset, what S, M and C mean in a Bedrock session, a
  section on Bedrock with what it stops and what it does not, and section 5's points 17 to 19.
  The pages carry 386 ties now (336 before). The commands on them passed the check against
  the help text.
- **What the documents' changes broke** (five ties): block 03's "Voice chat is the only UDP
  the launcher relays out of the sandbox" — the README now says "out of a Java game's
  sandbox", because a Bedrock session relays RakNet to the servers you list; the page says
  the same now; two quotes of the Status table on getting started (re-quoted: the packages
  built on the fifth round's tree, five emulator tests); and the chain of the not-defended
  list, at point 7 (its Bedrock sentence) and after point 16.
- **Section 7, brought up to date**: six of its items were settled by the documents of the
  fourth round (TODO.md, the fourth round's "Integration") and are listed as settled now,
  after reading them again against the current documents and code; still open are the rows
  without a test (B3, B15 and B39 added) and the Realms prompt's fixture.
- **Verified**: `python3 -I site/build.py --check` passes (13 pages, 1082 internal links, 86
  outbound links, two runs byte-identical), and so does the deployable build;
  `python3 -I -B -m unittest discover -s site -p "test_*.py"`: 361 tests, OK; `cargo test
  --test site`: 5 passed. **Not done**: the new and changed pages were not rendered in a
  browser or measured at the 44 widths of section 11 (the tools of the earlier passes are not
  in this clone); the drawings were not changed — they show a Java launch, and a Bedrock
  session's sandbox is described in words only.
