# Bastion OS — P6: Phone apps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three apps the owner asked for on 2026-11-16, in the in-game phone:
- **Wiki:** the Minecraft Wiki at minecraft.wiki, and nothing else.
- **Waypoints:** named places per world, with distance and direction.
- **Clock:** the time, timers, a stopwatch, a break reminder, and the game's own day and night.

The fourth app, the screenshot manager, is in P5.

**Architecture:**
- The apps are classes in the companion mod (`java/companion/src/bastion/companion/phone/`), on the existing `App` base, with the phone's dark style.
- Waypoints and Clock run inside the game only. They need nothing from the launcher and add no trust.
- Wiki uses the phone's existing browser (Firefox in bastion's browser sandbox, outside the game) with a built-in site whose hosts are only the wiki's own. It asks the launcher through the existing channel, as BrowserApp does.

**Tech Stack:** Java (the companion mod, built by `build.rs` through `src/companion/jar.rs`), Rust (the browser's site list, the phone dispatch), the existing phone channel.

**Spec:** The owner's request of 2026-11-16, and index Decision 14 in `docs/superpowers/plans/2026-11-15-bastion-os.md`. Read the index first: Global Constraints and Decision 14. Also read `docs/notes/companion-mod.md` (how the mod is built, injected and tested; the tested-version list; the feature registry; the channel), `docs/PHONE.md`, and `java/companion/src/bastion/companion/phone/{App,Phone,PhoneFeature,Send,Store,BrowserApp}.java`.

## Global Constraints

The index's Global Constraints apply. In addition:
- **No secrets, no network of their own:** the apps hold no secret and open no network connection. Wiki's pages come through the launcher's browser service, as the Browser app's do.
- **Every supported Minecraft version:** the apps work on every version in the companion's tested-version list (1.20.1–26.x). An app that cannot work on one version is hidden there, and the notes say why.
- **What game data is:** the waypoint and timer stores live in the instance (`config/bastion-companion/…`). They are game data, so the launcher never reads them and nothing outside the game depends on them.
- **Fair play on servers:** when a server reduces debug info (`reducedDebugInfo`), Waypoints hides coordinates, distances and the HUD arrow, as F3 does.
- **The time zone stays private:** the game does not learn the time zone unless `[sandbox] expose_timezone = true`. Without it, the Clock shows UTC and says so. Timers, the stopwatch and the break reminder do not need the time zone.
- **Layout:** every app is laid out in the phone's screen sizes with the existing phone tests' approach, in light and dark.

## Review Focus

1. **A server that reduces debug info.** Waypoints must not become a way around it. Pinned by Task 2 `with_reduced_debug_info_waypoints_hide_coordinates_and_the_arrow`.
2. **A link on a wiki page that leads off the wiki.** It is not followed, and the page says so. Pinned by Task 1 `a_link_outside_the_wiki_is_blocked_with_a_note`.
3. **A huge or corrupt waypoint store.** It may have been written by anything that can write the instance. It is read with limits (at most 500 waypoints per world, names of at most 32 characters, a 256 KiB file), and a corrupt file is kept aside, not crashed on. Pinned by Task 2 `a_corrupt_or_oversized_store_is_set_aside_and_the_app_starts_empty`.
4. **The time zone hidden from the game.** The Clock never guesses local time. Pinned by Task 3 `without_the_time_zone_the_clock_shows_utc_and_says_so`.
5. **Timers across a world change, a server switch or a restart of the game.** They keep running against real time and fire once. Pinned by Task 3 `a_timer_survives_a_world_change_and_fires_once`.

---

### Task 1: Wiki — minecraft.wiki and nothing else

**Files:**
- Create: `java/companion/src/bastion/companion/phone/WikiApp.java`
- Modify:
  - the browser's built-in sites (`src/browser/`), adding a site `wiki` with the wiki's hosts. The hosts are minecraft.wiki and the hosts its pages need; derive them from the site and record the sources in the notes.
  - the phone dispatch, if the app needs a verb of its own (`src/phone/dispatch.rs`);
  - the phone's app list.
- Test: Rust unit tests for the site; the companion's tests

**Interfaces:**
- Produces:
  - **The site:** a built-in browser site `wiki`, start page `https://minecraft.wiki/`, enabled by default when the phone is on (shown by the config manager like the other sites).
  - **The app:**
    - a search field that opens `https://minecraft.wiki/?search=<query>` (URL-encoded);
    - the last 20 pages, stored game-side;
    - a link to a host outside the site is not followed, with a one-line note.

- [ ] **Step 1: Write the failing tests:**
  - `the_wiki_site_allows_only_the_wiki_hosts`
  - `a_wiki_search_opens_the_search_page_for_the_query` (spaces, `&`, non-ASCII)
  - `a_link_outside_the_wiki_is_blocked_with_a_note`
  - companion: `the_wiki_app_searches_and_lists_recent_pages`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib browser && cargo test --test companion` (or the companion tests' actual entry point, as the notes describe). Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, and the opt-in browser test against the stand-in sites with `BASTION_BROWSER_TESTS=1` on your private display. Expected: PASS.
- [ ] **Step 5: Commit.** `phone: a Wiki app for minecraft.wiki, with nothing else reachable`

### Task 2: Waypoints

**Files:**
- Create: `java/companion/src/bastion/companion/phone/WaypointsApp.java`, `java/companion/src/bastion/companion/waypoints/{Waypoints.java,WaypointStore.java,Bearing.java}`, a small HUD element for the active waypoint
- Modify: the phone's app list and the feature registry (`PhoneFeature.java`)
- Test: the companion's tests (store, bearing, rules)

**Interfaces:**
- Produces:
  - **Waypoints per world.** A world is a single-player save's folder name, or the server's address. A waypoint has a name, position, dimension and colour.
  - **Actions:**
    - add one at the current position, with an editable name;
    - rename, delete, or set active.
  - **The list** is sorted by distance, with an arrow showing the direction relative to where the player faces.
  - **The active waypoint** shows a small HUD arrow and its distance.
  - **The last death position** is kept automatically as a waypoint named "Last death", replaced at each death.
  - **The store** is `config/bastion-companion/waypoints.json` in the instance, read with the limits of Review Focus 3.

- [ ] **Step 1: Write the failing tests:**
  - `a_waypoint_is_added_at_the_current_position_and_listed_by_distance`
  - `the_bearing_points_the_right_way_in_each_quadrant`
  - `waypoints_are_kept_per_world_and_dimension`
  - `the_last_death_is_kept_and_replaced`
  - `with_reduced_debug_info_waypoints_hide_coordinates_and_the_arrow`
  - `a_corrupt_or_oversized_store_is_set_aside_and_the_app_starts_empty`
- [ ] **Step 2: Run them; expect failure.** Run the companion tests. Expected: FAIL.
- [ ] **Step 3: Implement** with the companion's existing per-version adapters for player position, facing, dimension and death events. Hide what a version cannot provide.
- [ ] **Step 4: Run them; expect them to pass.** Run the companion tests, and the opt-in real-game companion test (`tests/companion_game.rs`) on your private display if the notes say it covers phone apps. Expected: PASS.
- [ ] **Step 5: Commit.** `phone: Waypoints per world with distance, direction, a HUD arrow and the last death, honouring reduced debug info`

### Task 3: Clock

**Files:**
- Create: `java/companion/src/bastion/companion/phone/ClockApp.java`, `java/companion/src/bastion/companion/clock/{Timers.java,TimerStore.java}`
- Modify: the phone's app list; the phone's notification path (the existing one the other apps use)
- Test: the companion's tests

**Interfaces:**
- Produces:
  - **The time:** local time when the sandbox exposes the time zone. Otherwise UTC, labelled "UTC — your time zone stays hidden from the game", with a pointer to bastion-mc's setting `sandbox.expose_timezone`. The game cannot change that setting.
  - **Timers:** named countdowns, with several running at once. They fire as a phone notification with a sound.
  - **A stopwatch.**
  - **A break reminder:** every N minutes, off by default.
  - **The game's time:** day number, time of day, minutes to sunset or sunrise, and the moon phase.
  - **The store:** timers are kept game-side against real time (`config/bastion-companion/timers.json`, with limits).

- [ ] **Step 1: Write the failing tests:**
  - `without_the_time_zone_the_clock_shows_utc_and_says_so`
  - `with_the_time_zone_the_clock_shows_local_time`
  - `a_timer_survives_a_world_change_and_fires_once`
  - `the_break_reminder_is_off_by_default_and_repeats_when_on`
  - `the_game_time_shows_the_day_the_time_and_the_moon_phase`
- [ ] **Step 2: Run them; expect failure.** Run the companion tests. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `phone: a Clock with timers, a stopwatch, a break reminder and the game's day and night, keeping the time zone private`

### Task 4: The notes

**Files:**
- Create: `docs/notes/os-p6.md`

- [ ] **Step 1: Write the notes.**
  - The apps, their stores and limits.
  - The wiki's host list and its sources.
  - Which Minecraft versions hide which parts, and why.
  - Every `Ruling:` this package made.
  - The user-facing text for docs/PHONE.md and the website's phone page.
- [ ] **Step 2: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Expected: 0 failed.
- [ ] **Step 3: Commit.** `docs: notes on the Wiki, Waypoints and Clock phone apps`
