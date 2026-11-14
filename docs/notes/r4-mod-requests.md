# Requests between mod-platform and mod-ui (clone `r4-mod`)

Both engineers append here; nobody rewrites the other's entries. Each entry: who asks, what,
why, and a status line the other side updates (`open` / `accepted` / `done in <commit>` /
`changed: …`). Read before every commit.

---------------------------------------------------------------------------------------

## R1 (mod-ui → mod-platform): what the UI needs beyond your draft Platform API

Status: **accepted** (mod-platform, 2026-11-14) — API v1 committed with everything below
except where the answer says otherwise; see "Answer to R1" after this entry.

I read your draft in `platform/` (`Platform`, `Canvas`, `Overlay`, `HudLayer`, `HomeAction`,
`Keys`; uncommitted when I read it) and build on it as it is — `Overlay` for the phone and
the home screen, `Keys` numbering, `setPhoneKey`, `setHome`/`available`/`perform`,
`setHud`, `texture`, `runOnRenderThread`. My code reaches the platform through one class of
mine (`ui/Bridge.java`), so later renames cost one file. What I still need, in order of
importance:

1. **The phone link** (protocol 2), e.g. on `Platform` or a `Link` of its own:
   ```java
   void setPhoneListener(PhoneListener listener);   // null: none
   boolean send(String commandLine);                // one COMMAND frame; false: not sent
   interface PhoneListener {                        // on the render thread, please
       void link(boolean connected);                // hello sent and answered / connection lost
       void state(String app, String json);         // STATE: the raw JSON; the UI parses it
       void notice(String app, String text);        // NOTICE
       void frame(String app, int slot, Pixels pixels); // FRAME, already decoded
   }
   ```
   Who decodes FRAME images (PNG, JPEG, RGBA8) into your `Pixels`? I assume you (the
   channel is yours, and `javax.imageio` vs the game's `NativeImage` is a platform choice);
   decoding off the render thread keeps browser frames (up to 15 per second) cheap. If you
   want me to decode, say so and tell me what `Pixels` is.
2. **Dark menus:** a hook for the background behind *vanilla* screens:
   ```java
   void setMenuBackground(MenuBackground painter);  // null: vanilla's
   interface MenuBackground {
       /** Instead of vanilla's background; return false to have vanilla's drawn after all. */
       boolean paint(Canvas canvas, boolean inWorld);
   }
   ```
   Where your adapters can reach more (list backgrounds, header/footer separators), a second
   method for those would be welcome; the screen background alone is enough to start.
3. **The GUI scale:** `double guiScale()` (real pixels per GUI unit), on `Canvas` or
   `Platform`. The browser's viewport (`browser size <w> <h>`) is the page area in real
   pixels, and images look sharp only when drawn at their pixel size.
4. **`HomeAction`:** add `MODS` (the mod list, only `available` when a mod-list mod such as
   Mod Menu is installed — the brief's "mods if present") and `CREDITS` (what the copyright
   line of vanilla's title screen opens). Optional but nice: `String label(HomeAction)`, the
   game's own caption in the player's language (else I use English captions).
5. **Text:** please draw `Canvas.text` literally — no `§` formatting codes interpreted (a
   track title containing `§k` must not turn into noise). I strip `§` from everything the
   launcher sends as well.
6. **Start-up:** an entry point for the UI. Proposal: after the platform is up, the start-up
   calls `bastion.companion.ui.Ui.features()` and starts the three `Feature`s it returns
   like the others (ids `phone`, `home`, `darkmenus`; a failure switches only that one off
   with the usual diagnostic). Each `resolve` gets the platform from `Platforms` — tell me
   the accessor's name (or pass it in `Config`/`Game`, as you prefer).
7. **The demo service:** for the real-game checks I need, in demo mode, a `phone` state
   announcing all five apps, and states that exercise every screen: Spotify now playing with
   album art (slot 0), results after `search`, devices after `devices`; browser `sites`
   including one named `discord`, a page frame (slot 0) at the size `size` asked for that
   shows where the last `mouse down` landed and what `text` typed (so the mapping shows in a
   screenshot); clips with a few entries and thumbnails after `list` / `thumb`; a Realms
   prompt with a running `expires_in` and `attempts_left`; settings with `sync`,
   `last_sync`, `keys`, `instances`. And every command it receives in the launch log,
   verbatim, so a test can compare what the phone sent with the grammar.
8. **A fake** for tests without Minecraft: I write my own recording fake of `Platform` and
   `Canvas` in `tests/java/` (I need every draw call with its bounds, to prove that nothing is
   drawn outside its panel, its phone or the screen). If you write one, tell me where.

## R2 (mod-ui → mod-platform): configuration under `[companion]`

Status: **accepted** (mod-platform, 2026-11-14): exactly these keys, defaults, file lines
and feature ids; in `src/companion/mod.rs` with my next launcher-side commit.

You own `src/config.rs`'s companion part and the settings file; I need these keys (merged
per instance like the others), rendered into the settings file as shown:

| TOML (`[companion]`) | default | settings file | what |
|---|---|---|---|
| `phone` | `true` | `phone=true` | the in-game phone |
| `phone_key` | `"key.keyboard.r"` | `phone.key=key.keyboard.r` | pulls it out / puts it away; same validation as `zoom_key` (`key_name`) |
| `home` | `true` | `home=true` | the redone title screen |
| `dark_menus` | `true` | `darkmenus=true` | darker backgrounds behind vanilla menus |

Feature ids for the hello: `phone`, `home`, `darkmenus` (they are also the flag names, as
for `zoom`, `hud`, `fullbright`).

Why **R** for the phone: no vanilla binding uses it on any version from 1.20.1 to 26.3
(checked against 26.3's options.txt, which lists every binding including the F3
combinations — those use A B C D G H I L N P S T V X 1–4 — and against the defaults of
1.20.x/1.21.x: W A S D Space Shift Ctrl Q E F T / Tab L C X P G (1.21.6+) O (26.x) 1–9
F1–F5 F11 and the hard-wired F3 combinations A B C D F G H I L N P Q S T). It is also not
one of Simple Voice Chat's (M N H V, read from its 2.6.27 jar; that mod is in this
project's own voice-chat instances), and it is in the same place on QWERTY, QWERTZ and
AZERTY. With a text field focused on the phone the key types a letter instead (my overlay
then returns true from `keyPressed`, so `setPhoneKey`'s callback does not fire).

### Answer to R1 (mod-platform, 2026-11-14)

All in package `bastion.companion.platform` (committed with this answer):

1. **Link**: `Platforms.link()` returns a `Link` (never null; offline it says
   `connected() == false`). Both styles: polling (`state(app)`, `picture(app, slot)`,
   `nextNotice()`, `changes()`; `State` carries `raw` JSON and the parsed `json`) and your
   listener: `link.setListener(Link.Listener)` with exactly your four methods
   (`link(boolean)`, `state(app, json)`, `notice(app, text)`, `frame(app, slot, Pixels)`),
   called on the render thread once per frame in arrival order; on being set the listener
   gets the connection state and the newest state of every app. `send(line)` checks the
   grammar (`Commands.valid`) and returns false when it would be refused or the link is down.
   **I decode** FRAME images (PNG/JPEG via the JDK's image readers on bytes in memory,
   RGBA8 by hand) into `Pixels` on the channel's reader thread, with the limits in `Pixels`.
2. **Dark menus**: `platform.setMenuBackground(MenuBackground)`, `boolean paint(Canvas,
   boolean inWorld)` (false: the game's background after all). The screen background only,
   to start; list backgrounds and separators later if the families allow it cheaply.
3. **GUI scale**: `platform.guiScale()` (real pixels per GUI unit).
4. **HomeAction**: `MODS` (available only where Mod Menu is installed) and `CREDITS` added;
   `platform.label(HomeAction)` gives the game's caption in the player's language (the
   translation keys are in `HomeLabels.key`, English fallbacks in `HomeLabels.english`).
5. **Text** is drawn literally (no `§` codes): documented on `Canvas.text`; the adapters draw
   through the game's literal path (`FormattedCharSequence.forward`), not the formatting one.
6. **Start-up**: as you proposed. The start-up calls `bastion.companion.ui.Ui.features()`
   and starts the returned `Feature`s like its own; the platform is `Platforms.current()`,
   the link `Platforms.link()` (both set before any feature starts). My placeholder
   `ui/Ui.java` returns an empty array — it is yours.
7. **Demo service**: will do as listed (every command logged verbatim in the launch log,
   rate-limited like everything else); I will say here when it is in.
8. **Fake**: I wrote one too, `tests/java/FakePlatform.java` (default package; draw calls as
   lines with their bounds, `FakeLink` with listener delivery on `frame()`); use it, extend
   it, or ignore it — if you write your own, name it differently so both compile.

## R3 (mod-ui → mod-platform): `Canvas.outline` on rectangles one unit thin

Status: **done** (mod-platform, 2026-11-14): `Canvas.outline` fills the whole rectangle when
it is less than three units wide or high (nothing inside, no side inside out).

`Canvas.outline(x, y, w, h, c)` with `h == 1` (or `w == 1`) fills the side edges inside out:
`fill(x, y + 1, x + 1, y + h - 1, c)` becomes `fill(x, y + 1, x + 1, y, …)`. The game's
`fill` swaps reversed corners, so it draws a stray unit rather than nothing. My harness
(`tests/phone_ui.rs`) caught it from my side; `ui/Gfx.frame` now guards it, but the base
method could too (a plain fill when `w < 2 || h < 2`).

## R4 (mod-ui → mod-platform): no typed characters on 26.x — SDL text input is never started

Status: **done** (mod-platform, 2026-11-14), as proposed: on 26.3 (the one SDL family — 26.1
and 26.2 still read input through GLFW, `InputConstants.KEY_A = 65` there, and deliver
characters) every host screen calls `minecraft.textInputManager().startTextInput(screen)` in
`init()` and `stopTextInput(screen)` in `removed()` (`Base.textInput`, overridden in
`platform/v26_3/FamilyGlue`). Not yet seen typing in the real game from my side (my runs press
no keys); your `phone-shots.sh` run is the check.

Seen in the real game (26.3, demo service, `tests/support/phone-shots.sh` on `:82`): key
presses reach the phone (`keyPressed`: R, Tab, Enter, Escape all work), but **no `charTyped`
ever arrives** — the Spotify search field, the Realms code and the browser page stay empty
(and the demo log shows no `search`, `code` or `text` command). In 26.x SDL3 delivers text
events only while text input is started for the window: the game does that through
`Minecraft.onTextInputFocusChange(GuiEventListener, boolean)` →
`TextInputManager.startTextInput(Object owner)` → `SDL_StartTextInput` when one of its own
`EditBox`es takes the focus (javap on 26.3). Our host screen has no `EditBox`, so nothing
starts it.

Proposal (the simplest that works for every overlay): on the SDL families, start text input
when a host screen is shown (`added`/`init`) and stop it when it is removed
(`textInputManager().startTextInput(this)` / `stopTextInput(this)`, or
`minecraft.onTextInputFocusChange(this, true/false)`). Keys keep arriving as keys either way; my
overlays already ignore characters they do not want. (GLFW families deliver characters
without this; 1.20.1–1.21.11 need nothing.)

## R5 (mod-ui → mod-platform): replies to commands do not seem to reach the phone

Status: **withdrawn** (mod-ui, 2026-11-14): not reproduced. Two further runs with a debug
line in my `Store` showed every reply arriving (browser states after `open`, the page frames
after `size`/`mouse`, the clip thumbnails after `list`); the first debug run's budget had been
used up by the periodic Spotify/Realms states. The first observation stays unexplained; I
will say here if it comes back.

Same run: the demo logs `received browser open discord` and `received clips thumb demo0..2`,
but the phone keeps showing "Opening Discord…" and the clip thumbnails never appear (3 s and
more later). What arrives at hello (the app list, Spotify's state with its album art, the
clips list) is shown correctly. So states and pictures sent *after* hello, in reply to a
command, appear not to arrive — or not to be delivered to the listener. I am adding a debug
line on my side to tell which; if you see it at once in `channel.rs`/`dispatch.rs` (the writer
after hello?), say so here.

## U1 (mod-platform → mod-ui): `tests/phone_ui.rs` ran out of heap

Status: **done** (mod-ui, 2026-11-14): two causes, both in the check, not in the phone. The fake
texture had started keeping each picture's pixels (for my preview renderer; it now does so only
when the preview asks), and the hostile-pictures step sent 1280×1280 pictures (6.5 MB of
pixels each) to all 34 clip slots — now to three. `PhoneUiCheck` passes in `-Xmx128m` (the
test gives it 256 MB). The phone itself closes a texture it replaces with one of another size
(`Store.frame`), as the check's "textures are reused or released" asserts. `closed()` at the
next frame: fine for the phone (it resets its state there).

In a full `cargo test --workspace --no-fail-fast` on this clone (tree as of commit f221639 plus
your uncommitted files at the time), `tests/phone_ui.rs` failed: `PhoneUiCheck` ended in
`java.lang.OutOfMemoryError: Java heap space` at `Pixels.filled` (your `PhoneUiCheck.java:835`)
under `-Xmx256m`, after the browser steps. A full-size `Pixels` is 16 MB (2048×2048 ints);
something keeps them (textures never closed in the fake? one per frame?). Everything else in
the workspace passed. Also for you: overlays that the *game* replaces (a death screen, a
disconnect) are now closed — `Overlay.closed()` called once — at the start of the next frame
(`Core.settleReplaced`), not at once.
