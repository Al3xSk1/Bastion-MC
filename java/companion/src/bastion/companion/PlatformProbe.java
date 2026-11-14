package bastion.companion;

import java.util.Locale;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Core;
import bastion.companion.platform.HomeAction;
import bastion.companion.platform.Host;
import bastion.companion.platform.Keys;
import bastion.companion.platform.Link;
import bastion.companion.platform.Overlay;
import bastion.companion.platform.Pixels;
import bastion.companion.platform.Platform;
import bastion.companion.platform.Platforms;
import bastion.companion.platform.Texture;

/**
 * The self-test's part for the platform, the same on every family (it uses the platform API
 * only): which family runs, what is on screen once the game has asked for its title screen,
 * an overlay opened, drawn with every primitive (text measured, a texture made from pixels
 * and one from a picture the launcher sent) and closed again, the HUD drawn in a world, and
 * the line to the launcher in both directions. Every result is one {@code selftest: …} line
 * for {@code tests/companion_game.rs}.
 */
final class PlatformProbe {
    private static final long TITLE_TIMEOUT_MILLIS = 240_000;
    private static final long STEP_TIMEOUT_MILLIS = 30_000;

    private final Consumer<String> say;

    PlatformProbe(Consumer<String> say) {
        this.say = say;
    }

    private interface Step {
        void run() throws Throwable;
    }

    private static Platform platform() {
        return Platforms.current();
    }

    /** Runs {@code step} on the render thread and waits for it. */
    private void onRenderThread(Step step) throws Throwable {
        CountDownLatch done = new CountDownLatch(1);
        Throwable[] failure = new Throwable[1];
        platform().runOnRenderThread(() -> {
            try {
                step.run();
            } catch (Throwable thrown) {
                failure[0] = thrown;
            } finally {
                done.countDown();
            }
        });
        if (!done.await(STEP_TIMEOUT_MILLIS, TimeUnit.MILLISECONDS)) {
            throw new IllegalStateException("the render thread did not run a step in time");
        }
        if (failure[0] != null) {
            throw failure[0];
        }
    }

    private static String name(Object object) {
        return object == null ? "none" : object.getClass().getName();
    }

    /** Waits until the game has asked for its title screen (it has finished loading then). */
    boolean awaitTitle() throws InterruptedException {
        long deadline = System.currentTimeMillis() + TITLE_TIMEOUT_MILLIS;
        while (System.currentTimeMillis() < deadline) {
            Core core = Core.get();
            if (core != null && core.titles() > 0 && core.frames() > 20) {
                return true;
            }
            Thread.sleep(250);
        }
        return false;
    }

    /** At the title (or home) screen. */
    void title() throws Throwable {
        Platform platform = platform();
        say.accept("platform family=" + platform.family() + " version=" + platform.minecraftVersion() + " live="
                + Platforms.live());
        Core core = Core.get();
        if (core == null) {
            return;
        }
        Thread.sleep(1000);
        String[] seen = new String[1];
        onRenderThread(() -> {
            Object screen = core.glue().screen();
            Host host = core.glue().hostOf(screen);
            seen[0] = String.format(Locale.ROOT,
                    "platform screen=%s home=%b demo=%b gui_scale=%.2f phone_key=%s singleplayer=%s demo_available=%b",
                    name(screen), host != null && host.home(), platform.demo(), platform.guiScale(),
                    platform.phoneKeyName(), platform.label(HomeAction.SINGLEPLAYER).replace(' ', '_'),
                    platform.available(HomeAction.PLAY_DEMO));
        });
        say.accept(seen[0]);
        overlay(core);
        menuBackground(core);
    }

    /**
     * A screen of the game's own (its options, opened as the home screen's button does): the
     * background behind it is the menu background painter's (when one is set), through the
     * family's background hook.
     */
    private void menuBackground(Core core) throws Throwable {
        Platform platform = platform();
        String[] seen = new String[1];
        long before = core.backgrounds();
        onRenderThread(() -> {
            seen[0] = name(core.glue().screen());
            platform.perform(HomeAction.OPTIONS);
        });
        Thread.sleep(1000);
        String[] options = new String[1];
        onRenderThread(() -> options[0] = name(core.glue().screen()));
        long painted = core.backgrounds() - before;
        // Back to where it was (the home screen).
        onRenderThread(() -> core.glue().setScreen(core.glue().newTitleScreen()));
        Thread.sleep(300);
        String[] back = new String[1];
        onRenderThread(() -> back[0] = name(core.glue().screen()));
        say.accept("platform options screen=" + options[0] + " backgrounds_painted=" + painted + " back=" + back[0]);
    }

    /** An overlay that draws with every primitive and remembers what it measured. */
    private static final class Probe implements Overlay {
        volatile int frames;
        volatile int width;
        volatile int height;
        volatile int textWidth;
        volatile int lineHeight;
        volatile int opened;
        volatile int closedCalls;
        volatile Texture texture;
        volatile Texture picture;
        volatile Pixels incoming;

        @Override
        public void opened(int w, int h) {
            opened++;
            width = w;
            height = h;
        }

        @Override
        public void render(Canvas canvas, int mouseX, int mouseY, float partialTick) {
            Platform platform = Platforms.current();
            if (texture == null) {
                int[] argb = new int[16 * 16];
                for (int i = 0; i < argb.length; i++) {
                    argb[i] = ((i / 16 + i % 16) & 1) == 0 ? 0xff20a060 : 0xff102030;
                }
                texture = platform.texture(Pixels.of(16, 16, argb));
            }
            Pixels pending = incoming;
            if (pending != null) {
                incoming = null;
                if (picture == null) {
                    picture = platform.texture(pending);
                } else {
                    picture.replace(pending);
                }
            }
            canvas.fill(0, 0, canvas.width(), canvas.height(), 0xc0101418);
            canvas.roundRect(20, 20, 120, 60, 8, 0xff2a2f3a);
            canvas.outline(20, 20, 120, 60, 0xff6a7f9a);
            canvas.clip(20, 20, 140, 80);
            canvas.push();
            canvas.translate(26, 26);
            canvas.scale(1.0f);
            canvas.text("Bastion phone §k test", 0, 0, 0xffffffff, true);
            canvas.pop();
            canvas.unclip();
            if (texture != null) {
                canvas.image(texture, 26, 40, 32, 32);
            }
            Texture shown = picture;
            if (shown != null) {
                canvas.image(shown, 70, 40, 48, 32);
            }
            textWidth = canvas.textWidth("Bastion");
            lineHeight = canvas.lineHeight();
            frames++;
        }

        @Override
        public boolean keyPressed(int key, int modifiers) {
            return key != Keys.UNKNOWN;
        }

        @Override
        public void closed() {
            closedCalls++;
        }
    }

    private final Probe probe = new Probe();

    private void overlay(Core core) throws Throwable {
        Platform platform = platform();
        String[] before = new String[1];
        onRenderThread(() -> {
            before[0] = name(core.glue().screen());
            platform.open(probe);
        });
        Link.Picture art = Platforms.link().picture("spotify", 0);
        if (art != null) {
            probe.incoming = art.pixels;
        }
        Thread.sleep(1500);
        boolean[] open = new boolean[1];
        onRenderThread(() -> open[0] = platform.isOpen(probe));
        say.accept(String.format(Locale.ROOT,
                "platform overlay open=%b frames=%d size=%dx%d text_width=%d line_height=%d texture=%b picture=%b opened=%d",
                open[0], probe.frames, probe.width, probe.height, probe.textWidth, probe.lineHeight, probe.texture != null,
                probe.picture != null, probe.opened));
        String[] after = new String[1];
        onRenderThread(() -> {
            platform.close(probe);
            open[0] = platform.isOpen(probe);
            after[0] = name(core.glue().screen());
        });
        say.accept("platform overlay closed=" + !open[0] + " closed_calls=" + probe.closedCalls + " screen_before="
                + before[0] + " screen_after=" + after[0]);
    }

    /** The line to the launcher: the phone state, then (with the demo) both directions. */
    void link() throws Throwable {
        Link link = Platforms.link();
        long deadline = System.currentTimeMillis() + 15_000;
        while ((!link.connected() || link.state("phone") == null) && System.currentTimeMillis() < deadline) {
            Thread.sleep(100);
        }
        Link.State phone = link.state("phone");
        say.accept("link connected=" + link.connected() + " phone=" + (phone == null ? "none" : phone.raw));
        if (phone == null || !phone.raw.contains("\"settings\"")) {
            return;
        }
        // Commands out, answers in: a notice, a state, and pictures of two formats.
        while (link.nextNotice() != null) {
            continue;
        }
        boolean sent = link.send("settings sync-now") & link.send("browser open discord")
                & link.send("browser size 200 120") & link.send("browser frames on") & link.send("browser text -- hi phone")
                & link.send("browser mouse down 50 60 left");
        // Outside the grammar: refused here, never sent. (A well-formed command the app will
        // refuse — a site that is not configured — is the launcher's to refuse.)
        boolean refused = !link.send("Browser open discord") && !link.send("x\ny z") && !link.send("browser");
        Link.Notice notice = null;
        Link.Picture page = null;
        deadline = System.currentTimeMillis() + 15_000;
        while ((notice == null || page == null || page.pixels.width() != 200) && System.currentTimeMillis() < deadline) {
            Link.Notice next = link.nextNotice();
            if (next != null && next.app.equals("settings")) {
                notice = next;
            }
            page = link.picture("browser", 0);
            Thread.sleep(100);
        }
        Link.Picture art = link.picture("spotify", 0);
        Link.State browser = link.state("browser");
        say.accept(String.format(Locale.ROOT,
                "link sent=%b refused_locally=%b notice=%s page=%s art=%s browser_open=%s",
                sent, refused, notice == null ? "none" : notice.text.replace(' ', '_'),
                page == null ? "none" : page.pixels.width() + "x" + page.pixels.height(),
                art == null ? "none" : art.pixels.width() + "x" + art.pixels.height(),
                browser == null ? "none" : String.valueOf(browser.json.get("open"))));
        if (page != null) {
            // The picture the launcher sent, as a texture in an overlay.
            probe.incoming = page.pixels;
            Platform platform = platform();
            onRenderThread(() -> platform.open(probe));
            Thread.sleep(800);
            int frames = probe.frames;
            onRenderThread(() -> platform.close(probe));
            say.accept("link picture_drawn=" + (probe.picture != null) + " frames=" + frames);
        }
    }

    /**
     * Into the demo world through the platform's own action (what the title screen's button
     * does), then the HUD: drawn in the world, every frame.
     */
    void world() throws Throwable {
        Platform platform = platform();
        Core core = Core.get();
        if (core == null) {
            return;
        }
        onRenderThread(() -> platform.perform(HomeAction.PLAY_DEMO));
        long deadline = System.currentTimeMillis() + 240_000;
        long playingSince = -1;
        while (System.currentTimeMillis() < deadline) {
            boolean[] state = new boolean[2];
            onRenderThread(() -> {
                state[0] = platform.inWorld();
                Object screen = core.glue().screen();
                if (state[0] && screen != null && core.glue().hostOf(screen) == null
                        && !screen.getClass().getName().toLowerCase(Locale.ROOT).contains("load")) {
                    // The demo greets a new player with a screen of its own: closed, as a player would.
                    core.glue().setScreen(null);
                }
                state[1] = platform.playing();
            });
            if (state[1]) {
                if (playingSince < 0) {
                    playingSince = System.currentTimeMillis();
                } else if (System.currentTimeMillis() - playingSince > 4000) {
                    break;
                }
            } else {
                playingSince = -1;
            }
            Thread.sleep(250);
        }
        boolean inWorld = platform.inWorld();
        long before = core.hudFrames();
        Thread.sleep(1000);
        long hud = core.hudFrames() - before;
        say.accept("platform world entered=" + inWorld + " playing=" + platform.playing() + " hud_frames_in_1s=" + hud);
    }

    /**
     * While playing, waits a few seconds for the phone key: the launcher's test may have a
     * helper really press it (on a display of its own). Reports whether a screen of this mod's
     * (the phone) opened meanwhile, and closes it again.
     */
    void phoneKey() throws Throwable {
        Platform platform = platform();
        Core core = Core.get();
        if (core == null || !platform.playing()) {
            say.accept("platform phone_key opened=false not_playing=true");
            return;
        }
        say.accept("platform watching phone_key");
        long deadline = System.currentTimeMillis() + 6000;
        String[] opened = new String[1];
        while (opened[0] == null && System.currentTimeMillis() < deadline) {
            onRenderThread(() -> {
                Object screen = core.glue().screen();
                Host host = core.glue().hostOf(screen);
                if (host != null && !host.home()) {
                    opened[0] = name(screen);
                }
            });
            Thread.sleep(100);
        }
        if (opened[0] != null) {
            onRenderThread(() -> core.glue().setScreen(null));
        }
        say.accept("platform phone_key opened=" + (opened[0] != null) + " screen=" + (opened[0] == null ? "none" : opened[0]));
    }
}
