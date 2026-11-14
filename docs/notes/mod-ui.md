# mod-ui: the phone, the home screen and the dark theme

Package mod-ui of round 4 (docs/notes/round4-contract.md). Part 1 is text for the README,
part 2 for the threat model, part 3 says how it is built, part 4 what was verified (by running,
against what) and part 5 what was not and what is open. The hand-over file is
`docs/notes/mod-ui-progress.md`.

Everything here is the companion mod's *interface*: it draws, takes the player's input and
turns it into command lines for the launcher. It enforces nothing. The launcher decides what
each command may do (round4-contract.md, invariant 2), and every secret stays with the
launcher (invariant 1).

---------------------------------------------------------------------------------------

## 1. For the README

### The phone

In a Fabric instance the companion mod gives you a phone. Press **R** in a world and it slides
up at the right of the screen, like a phone you pull out of your pocket; press **R** again (or
Escape on its home screen) to put it away. The game keeps running behind it — it is not a
pause menu, and in single player the world does not stop — and the mouse is free while the
phone is out.

What is on it depends on what the launcher set up for this game: only the apps the launcher
announced appear on its home screen.

| App | What you can do |
|---|---|
| **Spotify** | see what is playing (with the album art), play/pause, next, previous, seek on the bar, volume, shuffle, repeat; search and play or queue a result; pick the device that plays |
| **Discord** | Discord, in the launcher's separate browser, at the site called `discord` in the launcher's config |
| **Browser** | the sites listed in the launcher's config; the page is drawn into the phone, clicks and typing go to the page; turn the phone sideways (the wide-screen button) to read |
| **Clips** | keep the last 15 s … the whole buffer of the game as a video, see your clips with thumbnails, rename or delete one |
| **Realms** | when the launcher asks for a code before joining a Realm: type the code the launcher shows (or the one from your authenticator app), see how long it is valid and how many tries are left |
| **Settings** | whether your game settings are kept in step across your instances, when that last happened, *sync now* |

The status bar shows the time, how many notices arrived (a short line from the launcher: a
clip saved, a sign-in approved …) and whether the phone is connected to the launcher. When a
notice arrives while the phone is away, it peeks in at the right edge for a few seconds; a
Realms sign-in waiting for its code stays there until it is answered or expires.

Everything works with the mouse and with the keyboard alone: arrow keys or Tab move the
highlight, Enter or Space uses it, Escape or Backspace goes back one step (and from the
phone's home screen puts it away). While a text field has the cursor — a search, a code, a new
name — keys are letters: **R** types an "r" there instead of putting the phone away. Clicking
a web page gives it the keyboard (a cyan frame says so); Escape takes it back.

**Why R:** no Minecraft version from 1.20.1 to 26.3 binds it by default (checked against
26.3's own key list, which includes all F3 combinations, and against the defaults of the 1.20
and 1.21 versions), it is not one of Simple Voice Chat's keys (M, N, H, V), and it is in the
same place on QWERTY, QWERTZ and AZERTY keyboards. Another key: `phone_key` below.

### The home screen

The title screen is replaced by a dark one of the launcher's own design: the game's name and
version on the left with what the title screen offers — **Singleplayer**, **Multiplayer**,
**Minecraft Realms**, **Options**, **Mods** (when a mod list such as Mod Menu is installed) and
**Quit**; in demo mode **Play Demo World** instead of the first three — and below them
**Language**, **Accessibility** and **Classic title screen** (the game's own, once). On the
right, a sketch of the phone with the key that pulls it out. The captions are the game's own,
in your language; every button does exactly what the title screen's button does. The
copyright line opens the credits, as on the original.

### Dark mode

The phone and the home screen use one dark theme — near-black gunmetal, one cyan accent,
amber for caution, red for danger — the same family of colours as the launcher's window. The
game's own menus (options, world list, pause menu …) get a darker background too: flat
gunmetal outside a world, a dark veil over the world inside one. Only the background changes;
the menus themselves are the game's.

### Settings

In `~/.config/bastion-mc/config.toml`, and per instance under `[companion]` in its
`instance.toml`, like the companion's other keys:

```toml
[companion]
phone = true                  # the in-game phone
phone_key = "key.keyboard.r"  # Minecraft's name for the key (as in options.txt); "r" works too
home = true                   # the redone title screen
dark_menus = true             # darker backgrounds behind the game's own menus
```

All three are on by default. They need the companion mod (Fabric instances; see its notes)
and a Minecraft version the mod has an adapter for (1.20.1 to 26.3).

---------------------------------------------------------------------------------------

## 2. For the threat model

### What the phone is, in the model

The phone is code inside the game. Everything inside the game is hostile by assumption, so the
phone is treated exactly like any other mod: **it can be read, drawn over, imitated, or
replaced by any other mod in the same JVM**, and nothing it shows or sends carries any
authority. The rows below say what follows from that and what keeps it from mattering.

| # | Who / what | Can do through the phone | Defence | Residual |
|---|---|---|---|---|
| P1 | Another mod imitates the phone's commands (connects to the link, or calls the phone's code) | send any command line the phone could send | the launcher parses each line strictly, rate-limits it per app and verb and lets it do only its app's fixed vocabulary (mod-platform's dispatcher, each service) — the phone adds no capability: every command it can send, any mod can send | whatever an app's vocabulary allows is available to all code in the game: playing Spotify, opening a *configured* site, typing into the open page, saving or deleting clips, sending a Realms code, syncing settings |
| P2 | Another mod reads what the phone shows | read app states, notices, album art, browser pages, clip thumbnails | the launcher sends the phone only what it is meant to show; no token, cookie, profile or TOTP secret is ever in a state, a picture or a notice (invariant 1; the phone has no field for one) | what the phone shows is visible to all code in the game: what you listen to, the open page as pictures (including a Discord conversation on screen), clip names, the clip folder's path, Realms world names |
| P3 | Another mod reads what you type into the phone | read a search, text typed into a web page, a clip name, **a Realms code** | none possible inside the game: keystrokes reach every mod. A Realms code is short-lived and answers one question the launcher asked (realms-sync's notes) | text typed into a page through the phone (a Discord message, a password typed into a site!) is readable by game code. **Do not type passwords into the in-game browser**; sign in with `bastion-mc browser login <site>` outside the game (browser package) |
| P4 | Another mod draws a fake phone or a fake prompt | ask you for a Realms code, show a fake notice, a fake "connected" | none inside the game. The decision about a Realms join is the launcher's, and its own window/terminal shows what it is approving (realms-sync) | a fake prompt can collect a code you type; it is worth only the pending question the launcher asked. A fake notice can say anything |
| P5 | Hostile launcher messages (a bug in a service) | crash or freeze the phone, draw over the game, hide UI | every state is parsed by the platform's bounded JSON reader and read by `AppStates` with defaults, bounds and cleaning (no control, direction or `§` characters drawn); text is shortened to its box, everything is clipped to the phone; an exception closes the phone screen and is logged once (platform) | the launcher is trusted; this only limits the damage of its bugs |
| P6 | The home screen | everything the title screen's buttons do | each button calls `Platform.perform` with the same action as the game's button; nothing else | a mod can replace or imitate the home screen like any screen |

### What changes in existing rows

* **The companion mod's footprint** (row J18 of round 3): the mod now draws screens and
  reads input; it still holds no secret and names no network, process, native or file-writing
  API (`companion::tests::the_mod_names_no_api_for_networking_processes_native_code_or_writing_files`
  scans the jar with the UI in it, and passes).
* **Not defended (§5):** "The in-game phone is interface only. What it shows and what you type
  into it is visible to every mod in the game; type nothing secret into it."

---------------------------------------------------------------------------------------

## 3. How it works

### Packages (all under `java/companion/src/bastion/companion/`)

| Package | What |
|---|---|
| `ui/` | `Ui` (entry point: the three features), `Rect`/`Column` (layout by cutting rectangles), `Gfx` (fitted labels, tracked capitals, panels, pairs), `Icons` (pixel glyphs written as bitmaps in code), `Kit` (buttons, rows, tabs, sliders, fields, paragraphs), `Hits` (clickable places, keyboard focus by direction), `TextField`, `Scroller`, `Text` (cleaning launcher text), `Clock` (time source; tests move it) |
| `phone/` | `PhoneFeature`, `Phone` (the overlay: geometry, slide, status bar, home grid, banner, HUD toast, input rules), `Store` (the link's listener: states, textures, notices), `Send` (every command line), `AppStates` (bounded readers of each app's JSON), the apps `SpotifyApp`, `BrowserApp` (also Discord), `ClipsApp`, `RealmsApp`, `SettingsApp` |
| `home/` | `HomeFeature`, `HomeScreen` |
| `theme/` | `Theme` (palette), `DarkMenus` (feature and menu background painter) |

**Built only on the platform API** (`bastion.companion.platform`, mod-platform's): `Platform`,
`Overlay`, `Canvas`, `HudLayer`, `MenuBackground`, `Link`, `Pixels`/`Texture`, `Keys`,
`Commands`, `Json`, `HomeAction`. No game class is named anywhere in these packages, so the
same bytecode runs on every family the platform has an adapter for.

### Rules that keep things inside their panels

* Layout cuts rectangles out of rectangles (`Rect`, `Column`); nothing is placed by adding
  offsets to a parent's corner and hoping.
* Every piece of text goes through `Gfx.label`/`Gfx.caps`/`Gfx.pair`/`Kit.paragraph`: shortened
  with "..." to the width of its box (never cut by a clip), not drawn at all where the box is
  lower than a line, centred in the box's height. Two labels in one row never meet
  (`Gfx.pair`, or two lines where they would).
* The phone clips everything to its screen; lists clip to themselves and register clickable
  rows only where they are visible.
* The phone's size follows the screen (`Phone.body`): at most the screen less its margins;
  portrait 82 % of the height (220–320 GUI units) and 0.66 of that wide (120–196); sideways
  1.75 : 1 for reading. In a world, where it would stand over the hotbar's right end, it stands
  on the hotbar row instead if that leaves it at least 180 high (hearts, food and hotbar stay
  visible). The harness proves the rules on 13 window sizes (part 4).

### Input rules

`Phone.keyPressed`: Escape and Backspace go back (a focused field loses the focus first; an
app goes back within itself — Spotify to its first tab, a clip back to the list, the browser
out of page focus, then out of landscape); with a field focused every key is the field's
(returning true, so the platform does not treat the phone key as such); arrows and Tab move
the focus, Enter and Space use it. The mouse acts on press (like the game's buttons); sliders
send on release (seek) or at most four times a second while dragged (volume).

The browser's page: a click maps the phone position to the page picture's pixel
(`BrowserApp.toPage`) and sends `mouse down`/`mouse up` with that position; movement is sent
only while a button is held (at most 15/s); the wheel in batches (100 px per notch, one
`scroll` per 100 ms); typing in batches (one `text -- …` per 100 ms, at most 400 bytes);
Enter, Backspace, Tab, Delete, Home, End, the arrows, Page Up/Down and Ctrl+A/C/V/X/Z as
`key <name>`. The viewport (`size`) is the page area in real pixels, widened to at least 360
(portrait) or 640 (sideways) CSS pixels so that sites lay themselves out for a screen they
know, at most 1280 × 1280; sent when it has stayed the same for 0.3 s. `frames on` while the
page shows, `frames off` when it does not.

---------------------------------------------------------------------------------------

## 4. What was verified, and how

### Without the game — part of `cargo test` (`tests/phone_ui.rs`)

`tests/java/PhoneUiCheck.java` plays the game (a platform with screens, keys, a mouse and a
clock it moves by hand) and the launcher (app states, pictures, notices), compiled against the
embedded jar. It drives every app the way a player would and checks:

* **Commands:** every line the phone sent is parsed by the launcher's own
  `phone::Command::parse` and matched against the contract's vocabulary (verb; number, kind
  and shape of arguments — digits, `on|off`, Spotify URI kinds, the browser's key names …;
  text exactly where text belongs); 34 app/verb pairs must each occur. 681 lines in the last
  run.
* **Scripted behaviour (126 checks):** e.g. play/pause sends `pause` while playing and `play`
  while paused; seek is sent once, on release, at the right millisecond; volume is
  rate-limited while dragged and its last value sent on release; results without a valid URI
  and devices with an id outside the grammar send nothing; a click on the page lands on the
  page pixel under it (computed independently in the test); typing "hello, R" into the page is
  one `text` command and does not put the phone away; Escape steps back in the documented
  order; the phone key puts the phone away and brings it back where it was; a notice peeks in
  while the phone is away and goes again; a waiting Realms sign-in peeks in; only digits enter
  the code field; delete asks twice; keyboard-only use opens apps; the home screen's buttons
  perform exactly the title screen's actions, and demo mode offers the demo world instead of
  single/multiplayer/Realms; the notice banner shows the notice's text on every size from 240
  rows; out of a world the phone has a backdrop over the whole screen; in a world it stands on
  the hotbar row where it would cover the hotbar's right end.
* **Layout:** every draw call of 5 504 frames (853 044 operations) on 13 window sizes from
  200×150 to 2560×1080 GUI units, with ordinary, overlong (150-character fields, 64 results,
  40 sites, 64 clips), wide-script (CJK) and offline states: nothing outside its clip (text
  never cut by a clip — only shortened), nothing outside the phone's body or text outside its
  screen, nothing off the screen, no text over text, no rectangle inside out, clips and
  transforms balanced. 0 violations. Mutation check: with `Gfx.label` drawing text
  unshortened, 13 131 violations.
* **Hostile input:** 27 hostile messages (not JSON, wrong types, NaN-like and huge numbers,
  lone surrogates, control and direction characters, `§`, 40-deep nesting, URIs with paths,
  ids with spaces) to every app, 300 seeded mutations of well-formed states, pictures from 1×1
  to 2048×1 in all 34 slots, 100 notices: no exception, notices bounded at 20, textures reused
  or released.

Defects the harness found, fixed: Realms' two facts drawn over each other in narrow rows;
inside-out fills from `Canvas.outline` on 1-unit rectangles (guarded in `Gfx.frame`; reported
to mod-platform, R3); Escape stuck in the browser's sideways mode after the page closed;
Spotify's two time labels able to meet.

### Looked at, without the game

A preview renderer (not in the repository; `target-ui/scratch/preview`) draws every screen
with Java2D and the game's own bitmap font, read from the local 26.3 jar, at 480×270 ×2,
320×240 ×3, 640×360 ×2 and 854×480 ×2. Looked at; the changes it led to are in the hand-over
file (wider phone, pictogram tabs, shorter captions …).

### In the real game

**Setup.** Arch Linux (kernel 7.2.14), OpenJDK 27.0.1, Fabric Loader 0.20.1; my own private,
invisible display `:82` (headless sway with a rootless Xwayland, `tests/support/private-x11.sh`);
the launcher's own display choice there, a nested gamescope 3.17.0; demo mode (no account);
mod-platform's **demo service** for every app (`BASTION_PHONE_DEMO=1`: fake state, pictures,
every received command written to the launch log). One signed Fabric profile per family
representative in my data root `target/e2e-ui` — 1.20.1, 1.20.4, 1.21.1, 1.21.4, 1.21.5,
1.21.8, 1.21.10, 26.1.2, 26.2, 26.3 — without mods; the companion forced onto the versions not
yet on the launcher's tested list. Driver: `tests/support/phone-shots.sh` (launch, XTEST keys
and a click through `tests/support/x11-input.py`, screenshots of the game's window with
`ffmpeg -f x11grab -window_id`, SIGINT at the end). Every screenshot was looked at.

| Round | Window (GUI size, scale) | Families | Result |
|---|---|---|---|
| r1–r4 | 854×480 (427×240, 2) | 26.3 | first runs; findings below fixed; the page click checked by eye: the demo's cross under the pointer |
| sweep 1 | 854×480 (427×240, 2) | all ten | hello `features: phone, home, darkmenus` on all ten; full walk-through on nine (1.20.1: my driver waited for an atlas 1.20.1 does not have); some screenshots lagged the game (fixed in the driver: wait until the capture follows the step) |
| sweep 2 | 1600×900 (533×300, 3) | all ten | **all ten complete**: each family's demo log shows the same 21–22 commands, among them `spotify search -- rock` (typed, R a letter in the field), `realms code -- 424242`, `browser text -- …` per character, `browser size 432 555`, `browser mouse down/up 217 289 left` — the cross the demo draws there sits under the pointer; no stale screenshot |
| world | 854×480 (427×240, 2) | 26.3, 1.21.4 | the phone over the running demo world (no blur, no dimming), standing on the hotbar row (hearts, food and hotbar visible); Spotify; *save 30 s* → the banner "CLIPS / saved the last 30 seco…"; put away → the toast (a waiting Realms sign-in); the pause menu with the dark veil |
| world | 854×480, `guiScale:1` (854×480, 1) | 26.3 | the same at the smallest GUI scale: phone with a three-column grid and the notices list, home screen, page (viewport 360×524) |
| final | 854×480 (427×240, 2) | 26.3, 1.20.1 | on the final build: the browser turned sideways in the game (`browser size 748 326`, the page redrawn wide, the typed text kept); the two-line banner ("REALMS / approved (demo)"); the Realms empty text wrapped; the home hint in its short wording ("PRESS R / PHONE") where the long one does not fit; 22–23 commands at the demo |

Seen on every family in the sweeps: the home screen in place of the title screen (demo: *Play
Demo World* / *Options* / *Quit*, *Language*, *Accessibility Settings*, *Classic title screen*,
the copyright line), keyboard focus on it, the phone pulled out with R over it (the dark
background behind), its home screen with all six apps (the demo's five and Discord), Spotify
(album art, controls, search results with queue buttons), Discord's page with the typed text,
the browser's site list, clips with thumbnails, the Realms prompt (countdown, tries) and its
approval, settings, the game's own Options with the dark background (on 1.20.x in place of the
dirt texture).

**Found in the real game and fixed:** nothing behind the phone over the home screen (now the
dark menu background); the edition line and the phone hint cut on narrow rails (now a shorter
wording is chosen); in a world the phone covered the hotbar's right end at small GUI sizes (now
it stands on the hotbar row where that leaves it at least 180 high); the notice banner was too
low for its second line, so the notice's text was never drawn (fixed; the harness now requires
the text); the Realms empty-state line was cut (wraps now). **Found and fixed by mod-platform on
request:** no typed characters on 26.3 (SDL text input; R4).

---------------------------------------------------------------------------------------

## 5. Not verified, and open

* **Real services.** Every in-game check ran against mod-platform's demo service. The media,
  browser and realms-sync packages' real services were not run behind this phone here; the
  phone follows the contract's vocabularies (checked against the launcher's own parser and the
  contract in `tests/phone_ui.rs`), so what they send in those shapes is what it shows.
* **Mouse drags in the game.** Seek and volume (drag on a slider) and dragging on a web page are
  covered by the harness only; in the game the driver clicked and typed.
* **The non-demo home screen** (Singleplayer, Multiplayer, Realms) and the **Mods** button were
  seen only with the stand-in platform: no account here (demo mode only) and no Mod Menu.
* **Wayland:** not run (only the X11 path with the nested compositor).
* **GUI scales:** the game's auto scale at two window sizes and scale 1 on one family; the
  layout rules are proven on thirteen GUI sizes by the harness, not in the game.
* **A person at the keyboard.** The keys were XTEST events on a US layout; nobody has held the
  phone in a real session — how it *feels* (the slide, where it stands) is a judgement still to
  be made.
* **Clock and time zone:** the status bar shows the game JVM's time zone, which inside the
  sandbox may be UTC.
* **Fuzzing.** No new Rust parser of untrusted input in this package; the phone's readers of
  launcher states (`AppStates`, on the platform's `Json`) are exercised with 27 hostile and 300
  mutated messages in `tests/phone_ui.rs` (mod-platform fuzzes the JSON reader and the frame
  reader themselves).

