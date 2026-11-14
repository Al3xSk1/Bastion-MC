# Website: what a page and the style sheet agree on

This file began as the working agreement of the team that built the site (five roles side
by side in one working copy, 2026-11-13). The team has handed in; what stays true for
anyone who writes a page or changes the style sheet is below. What was done and verified,
what is open, and the claim maps that tie every hand-written sentence to its source are in
`docs/notes/website-progress.md`; the notes for the owner of the repository are in
`docs/notes/website.md`.

## The generator is the interface

`site/build.py` defines what a page source may contain (its directives and placeholders
such as `{{ROOT}}`, `{{REPO_URL}}`, `{{SITE_URL}}`, `{{SECTION}}`, `{{PARAGRAPH}}`,
`{{REQUIRE}}`, `{{SHOT}}`), which files it expects under `site/src/`, and what `--check`
enforces; its docstring lists them, and a unit test holds that list to the directives
there are. Read it before writing a page or a shell; do not guess. If the generator lacks
something a page needs, change the generator and its tests, not the page around it.

Pages (fixed): `index.html` (overview), `getting-started.html`, `security.html`,
`404.html`, and the rendered documents `manual.html` (README.md), `threat-model.html`,
`token-injection.html`, `dedicated-account.html`, `mods.html` (docs/MODS.md),
`companion.html` (docs/COMPANION.md), `phone.html` (docs/PHONE.md), `emergency.html` (the
runbook). Navigation (fixed): Get started, Security, Manual, Threat model, Emergency; the
footers list every page.

## The class vocabulary

Pages are plain semantic HTML first (`h1`–`h3`, `p`, `ul`/`ol`, `table`, `pre`/`code`,
`figure`), which the style sheet renders well with no class at all — the rendered
documents have none. On top of that, `site/src/assets/site.css` implements exactly these,
and pages use only these:

| Class | On | Meaning |
|---|---|---|
| `hero` | `section` | the top of the landing page; contains `h1`, `p.lede`, `p.actions` |
| `lede` | `p` | the statement under a page's `h1`: one paragraph (the overview's hero has a second, on the companion mod and the phone) |
| `actions` | `p` | a row of `a.button` links |
| `button`, `button primary` | `a` | a link drawn as a control; `primary` is the main one |
| `section` | `section` | a block of a page; starts with an `h2` |
| `label` | `p`, `span` | small-caps technical label (for example above a heading) |
| `readout` | `span`, `code` | a monospace value |
| `grid two`, `grid three` | `div` | children laid out in two or three columns, one column on a phone; a last child that would stand alone in its row takes the whole row |
| `panel` | `div`, `article` | a framed block with the site's corner treatment; first child usually `h3` |
| `panel caution`, `panel danger`, `panel quiet` | same | amber, red, and de-emphasised variants |
| `callout note`, `callout warning`, `callout danger` | `div`, `aside` | a statement the reader must not miss |
| `defended`, `not-defended` | `ul` | lists whose markers say "in force" and "not defended" |
| `status` | `table` | the status table; cells may carry `tested`, `partly`, `untested` |
| `steps` | `ol` | numbered procedure with prominent numerals |
| `shots` | `div` | a gallery of `figure.shot` (screenshot + `figcaption`) |
| `diagram` | `figure` | holds the layers diagram (`img` from `assets/`) and its caption |
| `doc` | `article` | body of a rendered document (set by the document shell) |
| `toc` | `nav` | a table of contents; its first child may be a `p.label` ("On this page") |
| `table-wrap` | `div` | horizontal scroll container the generator puts around tables |
| `skip`, `site-header`, `bar`, `brand`, `site-nav`, `site-footer`, `footer-nav`, `legal` | shell | used by `template.html` and the footers |

Used only by the shells: `masthead`, `chip`, `doc-layout`, `doc-head`, `doc-source`,
`toc-fold`, `toc-rail`, `runbook`, `runbook-lead`, `runbook-note`, `device-check`. Written
by the generator and styled in `site.css`: `heading`, `anchor`, `pre.diagram`, `cols-N`,
`align-*`, `a.shot`, `a.tap`, `tap-what`, `tap-where`, `nav-emergency`, `page-document`,
`page-emergency`.

A page that needs something that is not in this table adds it to `site.css` and to this
table in the same commit. No inline styles and no `style` elements anywhere: the
Content-Security-Policy forbids them, and so does `--check`.

## Rules for every page

- No JavaScript, and no request to any other host.
- Every link between pages is relative, so that the site works below a sub-path and from
  the file system; `404.html`, the shells and the footers write `{{ROOT}}` before every
  same-site address (GitHub serves the error page at any depth).
- No file of the website contains the owner's or the repository's name: `{{REPO_URL}}`
  and `{{SITE_URL}}` stand for them, and the workflow fills them in.
- Every `<pre>` carries `tabindex="0"`: a code block wider than its column scrolls inside
  its own box, and a reader on the keyboard has to reach what is hidden (`--check` fails
  on one without it; the converter writes it on every block of a document).
- Every claim is backed by `README.md`, `docs/THREAT_MODEL.md` or another rendered
  document: a statement that can go stale carries a `{{REQUIRE}}` tie with the source's
  words, and every hand-written statement has a row in the claim maps of
  `docs/notes/website-progress.md`. No superlatives, no numbers that change with every
  merge (test counts least of all), and what is not verified is said.
- Every footer carries the trademark statement (not an official Minecraft product, not
  approved by or associated with Mojang or Microsoft) and the sentence about what the host
  sees.
- The website does not change the documents it renders; a defect found in one goes to the
  document's owner (`docs/notes/website.md`, section 7).
- To try something, build into a directory of your own
  (`python3 site/build.py --out target/site-NAME --check`); never put files into
  `site/dist/` by hand.
