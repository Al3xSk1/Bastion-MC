package bastion.companion;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executor;
import java.util.concurrent.TimeUnit;
import java.util.function.Function;

/**
 * A scripted run of every feature inside the real game, for the launcher's integration test
 * (`tests/companion_game.rs`). It starts only when the launcher's settings file says so,
 * which no configuration file can make it do: the switch exists in the launcher's test
 * interface alone.
 *
 * It waits (on a thread of its own, sleeping between looks) until the game has finished
 * loading, then has each active feature exercise itself on the render thread and logs what
 * was measured as `selftest: …` lines; the test asserts on those numbers. In `world` mode it
 * then enters Mojang's demo world the way the title screen's button does and measures what
 * only a running world shows: the camera's field of view with the zoom key down and up,
 * how far the same mouse movement turns the player in both states, a notch of the wheel
 * through the game's own scroll handling, the HUD being drawn, and the light map's
 * brightness. Up to there the key's state is scripted. Last, it watches the camera for a
 * few seconds with nothing scripted — while playing, and with the chat open — which is
 * when the test can have the key really pressed.
 *
 * Unlike the features, this class uses plain reflection and assumes the game it was
 * written against; where an assumption fails it reports the failure and stops.
 */
final class SelfTest {
    private static final long LOAD_TIMEOUT_MILLIS = 240_000;
    private static final long WORLD_TIMEOUT_MILLIS = 240_000;
    private static final long STEP_TIMEOUT_MILLIS = 30_000;
    private static final long SETTLE_MILLIS = 4_000;
    private static final long WATCH_MILLIS = 5_000;
    private static final String DEMO_WORLD = "Demo_World";

    private final String mode;
    private final ClassLoader loader = SelfTest.class.getClassLoader();
    private Object minecraft;
    private Client client;

    private SelfTest(String mode) {
        this.mode = mode;
    }

    static void start(Config config) {
        String mode = config.text("selftest", "");
        if (!mode.equals("title") && !mode.equals("world")) {
            return;
        }
        Thread thread = new Thread(() -> new SelfTest(mode).run(), "bastion-companion-selftest");
        thread.setDaemon(true);
        thread.start();
    }

    private static void say(String text) {
        Log.info("selftest: " + text);
    }

    private void run() {
        try {
            // The platform, on every family: what is on screen, an overlay, the launcher link.
            PlatformProbe platform = new PlatformProbe(SelfTest::say);
            boolean live = bastion.companion.platform.Platforms.live();
            if (live) {
                if (!platform.awaitTitle()) {
                    say("failed: the game did not ask for its title screen in time");
                    return;
                }
                platform.title();
                platform.link();
            }
            // The features written for 26.3's names, measured through those names.
            if (!live || "v26_3".equals(bastion.companion.platform.Platforms.current().family())) {
                minecraft = loaded();
                if (minecraft == null) {
                    say("failed: the game did not finish loading in time");
                    return;
                }
                client = new Client(new Game(loader));
                onRenderThread(this::title);
                if (mode.equals("world")) {
                    world();
                }
            } else {
                for (Feature feature : Hooks.ALL) {
                    say(feature.id() + " not on this family");
                }
                if (mode.equals("world")) {
                    platform.world();
                }
            }
            if (live && mode.equals("world") && minecraft != null) {
                say("platform world hud_frames=" + bastion.companion.platform.Core.get().hudFrames());
            }
            if (live && mode.equals("world")) {
                platform.phoneKey();
            }
            say("done mode=" + mode);
        } catch (Throwable failure) {
            say("failed: " + Log.describe(failure));
        }
    }

    /** The client object once the game has finished loading; null on timeout. */
    private Object loaded() throws Exception {
        Class<?> type = Class.forName("net.minecraft.client.Minecraft", false, loader);
        Method instance = type.getMethod("getInstance");
        Method finished = type.getMethod("isGameLoadFinished");
        long deadline = System.currentTimeMillis() + LOAD_TIMEOUT_MILLIS;
        while (System.currentTimeMillis() < deadline) {
            Object found = instance.invoke(null);
            if (found != null && (Boolean) finished.invoke(found)) {
                return found;
            }
            Thread.sleep(250);
        }
        return null;
    }

    private interface Step {
        void run() throws Throwable;
    }

    /** Runs `step` on the game's render thread (the client object is its task queue) and waits. */
    private void onRenderThread(Step step) throws Throwable {
        CountDownLatch finished = new CountDownLatch(1);
        Throwable[] failure = new Throwable[1];
        ((Executor) minecraft).execute(() -> {
            try {
                step.run();
            } catch (Throwable thrown) {
                failure[0] = thrown;
            } finally {
                finished.countDown();
            }
        });
        if (!finished.await(STEP_TIMEOUT_MILLIS, TimeUnit.MILLISECONDS)) {
            throw new IllegalStateException("the render thread did not run a step in time");
        }
        if (failure[0] != null) {
            throw failure[0];
        }
    }

    private static String name(Object object) {
        return object == null ? "none" : object.getClass().getName();
    }

    // ------------------------------------------------------------ at the title screen

    private void title() throws Throwable {
        say("loaded screen=" + name(client.screen(minecraft)) + " player=" + name(client.player(minecraft)));
        for (Feature feature : Hooks.ALL) {
            if (!feature.live()) {
                say(feature.id() + " off");
                continue;
            }
            List<String> measured = new ArrayList<>();
            try {
                feature.probe(minecraft, measured);
            } catch (Throwable failure) {
                measured.add(feature.id() + " probe failed: " + Log.describe(failure));
            }
            for (String line : measured) {
                say(line);
            }
        }
    }

    // ------------------------------------------------------------ in a world

    private static Object call(Object target, String method, Object... arguments) throws Exception {
        for (Method candidate : target.getClass().getMethods()) {
            if (candidate.getName().equals(method) && candidate.getParameterCount() == arguments.length) {
                return candidate.invoke(target, arguments);
            }
        }
        throw new NoSuchMethodException(target.getClass().getName() + "." + method);
    }

    private static Object field(Object target, String name) throws Exception {
        return target.getClass().getField(name).get(target);
    }

    private Object constant(String type, String name) throws Exception {
        return Class.forName(type, true, loader).getField(name).get(null);
    }

    /**
     * Waits until the player has been in the world, with nothing in front of it, for
     * {@link #SETTLE_MILLIS} without interruption. Loading screens come and go, and the demo
     * greets a new player with a screen of its own a moment *after* the world has appeared;
     * a screen that is not a loading screen is closed, as a player would close it. False on
     * timeout.
     */
    private boolean settledInWorld() throws Throwable {
        long deadline = System.currentTimeMillis() + WORLD_TIMEOUT_MILLIS;
        long playingSince = -1;
        int closed = 0;
        while (true) {
            long now = System.currentTimeMillis();
            if (now > deadline) {
                return false;
            }
            boolean[] playing = new boolean[1];
            String[] screen = new String[1];
            onRenderThread(() -> {
                Object open = client.screen(minecraft);
                if (client.player(minecraft) != null && open != null && !open.getClass().getName().contains("Loading")) {
                    screen[0] = open.getClass().getName();
                    call(field(minecraft, "gui"), "setScreen", new Object[] {null});
                }
                playing[0] = client.playing();
            });
            if (screen[0] != null && closed++ < 8) {
                say("world closed_screen=" + screen[0]);
            }
            if (!playing[0] || screen[0] != null) {
                playingSince = -1;
            } else if (playingSince < 0) {
                playingSince = now;
            } else if (now - playingSince >= SETTLE_MILLIS) {
                return true;
            }
            Thread.sleep(250);
        }
    }

    private void world() throws Throwable {
        onRenderThread(this::enterDemoWorld);
        if (!settledInWorld()) {
            say("world failed: not in the world in time");
            return;
        }

        Object renderer = field(minecraft, "gameRenderer");
        Object camera = call(renderer, "mainCamera");
        Object mouse = field(minecraft, "mouseHandler");
        Object player = client.player(minecraft);
        boolean[] flags = new boolean[2];
        onRenderThread(() -> {
            flags[0] = (Boolean) call(minecraft, "isWindowActive");
            flags[1] = (Boolean) call(mouse, "isMouseGrabbed");
        });
        say("world entered window_active=" + flags[0] + " mouse_grabbed=" + flags[1] + " screen="
                + name(client.screen(minecraft)));

        if (Hooks.ZOOM.live()) {
            float idle = (Float) call(camera, "getFov");
            float plainTurn = turned(player, mouse);
            Hooks.ZOOM.testKey = Boolean.TRUE;
            Thread.sleep(1000);
            float zoomed = (Float) call(camera, "getFov");
            float zoomedTurn = turned(player, mouse);
            Hooks.ZOOM.testKey = Boolean.FALSE;
            Thread.sleep(1000);
            float after = (Float) call(camera, "getFov");
            Hooks.ZOOM.testKey = null;
            say(String.format(Locale.ROOT,
                    "world zoom fov_idle=%.3f fov_zoomed=%.3f fov_after=%.3f turn_plain=%.4f turn_zoomed=%.4f"
                            + " zoom_live=%b",
                    idle, zoomed, after, plainTurn, zoomedTurn, Hooks.ZOOM.live()));

            // The wheel, through the game's own scroll handling (the method the callback
            // sits in): with the key held it changes the factor and the hotbar stays where
            // it is; with the key up the hotbar moves.
            long window = (Long) call(call(minecraft, "getWindow"), "handle");
            Object inventory = call(player, "getInventory");
            int[] slot = new int[3];
            double[] factor = new double[2];
            Hooks.ZOOM.testKey = Boolean.TRUE;
            Thread.sleep(500);
            onRenderThread(() -> {
                slot[0] = (Integer) call(inventory, "getSelectedSlot");
                factor[0] = Hooks.ZOOM.factor();
                call(mouse, "onScroll", window, 0.0, 1.0);
                slot[1] = (Integer) call(inventory, "getSelectedSlot");
                factor[1] = Hooks.ZOOM.factor();
            });
            Hooks.ZOOM.testKey = Boolean.FALSE;
            Thread.sleep(500);
            onRenderThread(() -> {
                call(mouse, "onScroll", window, 0.0, 1.0);
                slot[2] = (Integer) call(inventory, "getSelectedSlot");
            });
            Hooks.ZOOM.testKey = null;
            say(String.format(Locale.ROOT,
                    "world wheel slot_before=%d slot_after_notch_while_zoomed=%d factor_before=%.3f factor_after=%.3f"
                            + " slot_after_notch_released=%d",
                    slot[0], slot[1], factor[0], factor[1], slot[2]));
        }
        if (Hooks.HUD.live()) {
            long before = Hooks.HUD.drawn();
            Thread.sleep(1000);
            long frames = Hooks.HUD.drawn() - before;
            String[][] lines = new String[1][];
            onRenderThread(() -> lines[0] = Hooks.HUD.describe(minecraft));
            say("world hud frames_drawn_in_1s=" + frames + " lines=[" + String.join(" | ", lines[0]) + "] direction="
                    + call(player, "getDirection") + " hud_live=" + Hooks.HUD.live());

            // When the readout must not be there: the HUD hidden (what F1 does), the game's
            // debug overlay up (what F3 does), and — position and facing only — reduced
            // debug information (the player's option here; a server can demand the same).
            Object hud = field(field(minecraft, "gui"), "hud");
            Object debugEntries = field(minecraft, "debugEntries");
            Object reducedInfo = call(field(minecraft, "options"), "reducedDebugInfo");
            onRenderThread(() -> call(hud, "toggle"));
            long whileHidden = framesDrawn();
            onRenderThread(() -> {
                call(hud, "toggle");
                call(debugEntries, "setOverlayVisible", true);
            });
            long whileDebugging = framesDrawn();
            onRenderThread(() -> call(debugEntries, "setOverlayVisible", false));
            long afterwards = framesDrawn();
            onRenderThread(() -> {
                call(reducedInfo, "set", true);
                try {
                    lines[0] = Hooks.HUD.describe(minecraft);
                } finally {
                    call(reducedInfo, "set", false);
                }
            });
            say("world hud_conditions frames_while_hud_hidden=" + whileHidden + " frames_while_debug_overlay="
                    + whileDebugging + " frames_afterwards=" + afterwards + " hud_live=" + Hooks.HUD.live()
                    + " lines_with_reduced_debug_info=[" + String.join(" | ", lines[0]) + "]");
        }
        if (Hooks.FULLBRIGHT.live()) {
            float[] brightness = new float[1];
            double[] option = new double[1];
            onRenderThread(() -> {
                Object state = field(call(renderer, "gameRenderState"), "lightmapRenderState");
                brightness[0] = (Float) field(state, "brightness");
                option[0] = Hooks.FULLBRIGHT.optionGamma(minecraft);
            });
            say(String.format(Locale.ROOT,
                    "world fullbright gamma=%.3f option=%.3f brightness=%.3f applied=%d fullbright_live=%b",
                    Hooks.FULLBRIGHT.gamma(), option[0], brightness[0], Hooks.FULLBRIGHT.applied(),
                    Hooks.FULLBRIGHT.live()));
        }
        if (Hooks.ZOOM.live()) {
            // From here on nothing is scripted: the mod reads the game's own keyboard state.
            // The launcher's test may have a helper really press the zoom key (on a display
            // of its own) while the camera is watched — first while playing, then with the
            // chat open, where the same key is a letter and must not zoom.
            watch(camera, "playing");
            Object chat = Class.forName("net.minecraft.client.gui.screens.ChatScreen", true, loader)
                    .getConstructor(String.class, boolean.class).newInstance("", false);
            onRenderThread(() -> call(field(minecraft, "gui"), "setScreen", chat));
            watch(camera, "chat");
            String[] typed = new String[1];
            onRenderThread(() -> {
                Field input = chat.getClass().getDeclaredField("input");
                input.setAccessible(true);
                typed[0] = String.valueOf(call(input.get(chat), "getValue"));
                call(field(minecraft, "gui"), "setScreen", new Object[] {null});
            });
            say("world chat typed=[" + Log.clip(typed[0]) + "]");
        }
    }

    /** Frames in which the readout was drawn during 700 ms, starting 300 ms from now. */
    private static long framesDrawn() throws InterruptedException {
        Thread.sleep(300);
        long before = Hooks.HUD.drawn();
        Thread.sleep(700);
        return Hooks.HUD.drawn() - before;
    }

    /** Reports the smallest field of view the camera had during {@link #WATCH_MILLIS}, and the last. */
    private void watch(Object camera, String state) throws Throwable {
        say("world watching state=" + state + " screen=" + name(client.screen(minecraft)) + " millis=" + WATCH_MILLIS);
        float smallest = Float.MAX_VALUE;
        float last = 0.0f;
        long end = System.currentTimeMillis() + WATCH_MILLIS;
        while (System.currentTimeMillis() < end) {
            last = (Float) call(camera, "getFov");
            smallest = Math.min(smallest, last);
            Thread.sleep(20);
        }
        say(String.format(Locale.ROOT, "world watched state=%s fov_min=%.3f fov_end=%.3f", state, smallest, last));
    }

    /**
     * What the title screen's "Play Demo World" button does: open the demo world, creating
     * it first if this instance has none.
     */
    private void enterDemoWorld() throws Throwable {
        Object options = field(minecraft, "options");
        // The test's window is not necessarily focused; a pause screen would stop the world.
        options.getClass().getField("pauseOnLostFocus").setBoolean(options, false);
        Object flows = call(minecraft, "createWorldOpenFlows");
        boolean exists;
        try (AutoCloseable access = (AutoCloseable) call(call(minecraft, "getLevelSource"), "createAccess", DEMO_WORLD)) {
            exists = (Boolean) call(access, "hasWorldData");
        }
        say("world entering existing=" + exists);
        if (exists) {
            Runnable cancelled = () -> say("world failed: opening the demo world was cancelled");
            call(flows, "openWorld", DEMO_WORLD, cancelled);
            return;
        }
        Method dimensions = Class.forName("net.minecraft.world.level.levelgen.presets.WorldPresets", true, loader)
                .getMethod("createNormalWorldDimensions", Class.forName("net.minecraft.core.HolderLookup$Provider", false, loader));
        Function<Object, Object> normal = registries -> {
            try {
                return dimensions.invoke(null, registries);
            } catch (ReflectiveOperationException e) {
                throw new IllegalStateException(e);
            }
        };
        call(flows, "createFreshLevel", DEMO_WORLD,
                constant("net.minecraft.server.MinecraftServer", "DEMO_SETTINGS"),
                constant("net.minecraft.world.level.levelgen.WorldOptions", "DEMO_OPTIONS"), normal,
                client.screen(minecraft));
    }

    /**
     * Degrees the player turns when the game is handed 200 units of horizontal mouse
     * movement — through its own mouse handling, a few frames later. 0 when the game does
     * not turn the player at all (the window is not focused or the mouse not captured).
     */
    private float turned(Object player, Object mouse) throws Throwable {
        float[] yaw = new float[2];
        onRenderThread(() -> {
            yaw[0] = (Float) call(player, "getYRot");
            Hooks.ZOOM.testNudge(mouse, 200.0);
        });
        Thread.sleep(400);
        onRenderThread(() -> yaw[1] = (Float) call(player, "getYRot"));
        return yaw[1] - yaw[0];
    }
}
