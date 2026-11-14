package bastion.companion.feature;

import java.lang.invoke.MethodHandle;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.List;
import java.util.Locale;

import bastion.companion.Client;
import bastion.companion.Config;
import bastion.companion.Feature;
import bastion.companion.Game;
import bastion.companion.Hooks;

/**
 * Hold a key to narrow the field of view.
 *
 * <ul>
 * <li>Active only while playing: with a screen open (chat, inventory, a menu) the key is a
 *     letter to type, and nothing zooms.</li>
 * <li>The mouse wheel changes the factor while the key is held (and then does not switch
 *     the hotbar slot); releasing the key forgets the change.</li>
 * <li>Zooming in and out is eased over about a fifth of a second, independent of the frame
 *     rate.</li>
 * <li>Mouse movement is scaled by the ratio of the two view angles' tangents, so that a
 *     hand movement turns the picture by the same distance on screen as without zoom.</li>
 * </ul>
 *
 * The hook sits where the camera computes its field of view for the world
 * ({@code Camera.calculateFov}); the first-person hand keeps its own, fixed angle. All state
 * belongs to the render thread.
 */
public final class Zoom extends Feature {
    public static final double MIN_FACTOR = 1.0;
    public static final double MAX_FACTOR = 50.0;
    /** One wheel notch multiplies the factor by this. */
    private static final double WHEEL_STEP = 1.15;
    /** Time constant of the exponential ease; the transition is 98 % done after four of them. */
    private static final double EASE_SECONDS = 0.045;
    /** A frame gap longer than this is treated as this (a paused or stalled game must not jump). */
    private static final double MAX_STEP_SECONDS = 0.1;
    /** A turn scale older than this is not applied: the camera has stopped asking. */
    private static final long STALE_NANOS = 250_000_000L;

    private String keyName = "key.keyboard.c";
    private double configuredFactor = 4.0;
    private boolean wheel = true;

    private Client client;
    private MethodHandle keyDown;
    private MethodHandle keyNamed;
    private MethodHandle typeOfKey;
    private MethodHandle nameOfKey;
    private MethodHandle valueOfKey;
    private MethodHandle readMoveX;
    private MethodHandle readMoveY;
    private MethodHandle writeMoveX;
    private MethodHandle writeMoveY;
    /** The key's number in the game's keyboard state; 0 until the key has been looked up. */
    private int scancode;

    private boolean held;
    private double factor;
    /** Natural logarithm of the multiplier currently applied to the field of view; 0 = none. */
    private double level;
    private long lastNanos;
    private double turnScale = 1.0;
    private long turnScaleNanos;

    /**
     * Self-test only: stand in for the key, the "playing" state and the clock when non-null.
     * (Volatile because the self-test sets them from its own thread.)
     */
    public volatile Boolean testKey;
    public volatile Boolean testPlaying;
    public volatile Long testNanos;

    public Zoom() {
        super("zoom");
    }

    @Override
    protected void configure(Config config) {
        keyName = config.text("zoom.key", keyName);
        configuredFactor = config.number("zoom.factor", configuredFactor, 1.1, MAX_FACTOR);
        wheel = !"false".equals(config.text("zoom.wheel", "true"));
        factor = configuredFactor;
    }

    @Override
    protected void resolve(Game game) throws Throwable {
        client = new Client(game);
        Class<?> input = game.type("com.mojang.blaze3d.platform.InputConstants");
        Class<?> key = game.type("com.mojang.blaze3d.platform.InputConstants$Key");
        keyDown = game.statik(input, "isKeyDown", boolean.class, int.class);
        keyNamed = game.statik(input, "getKey", key, String.class);
        typeOfKey = game.virtual(key, "getType", game.type("com.mojang.blaze3d.platform.InputConstants$Type"));
        nameOfKey = game.virtual(key, "getName", String.class);
        valueOfKey = game.virtual(key, "getValue", int.class);
        Class<?> mouse = game.type("net.minecraft.client.MouseHandler");
        readMoveX = game.privateGetter(mouse, "accumulatedDX");
        readMoveY = game.privateGetter(mouse, "accumulatedDY");
        writeMoveX = game.privateSetter(mouse, "accumulatedDX");
        writeMoveY = game.privateSetter(mouse, "accumulatedDY");
        if (readMoveX.type().returnType() != double.class || readMoveY.type().returnType() != double.class) {
            throw new NoSuchFieldException("MouseHandler.accumulatedDX/DY are not doubles");
        }
        game.requireHook(game.type("net.minecraft.client.Camera"), "bastion$zoomFov", "calculateFov", float.class,
                float.class);
        game.requireHook(mouse, "bastion$zoomTurn", "turnPlayer", void.class, double.class);
        game.requireHook(mouse, "bastion$zoomWheel", "onScroll", void.class, long.class, double.class, double.class);
    }

    /**
     * The key, by the name Minecraft itself uses for it (the one in options.txt). Only a
     * keyboard key the game knows under exactly that name is accepted: the state of such a
     * key can be read by its number; a mouse button's cannot.
     *
     * Done on first use, not at start-up: asking the game for a key runs the static
     * initialisers of its input classes, and the mod never does that before the game has.
     */
    private void bindKey() throws Throwable {
        Object found;
        try {
            found = (Object) keyNamed.invokeExact((Object) keyName);
        } catch (RuntimeException unknown) {
            throw new IllegalArgumentException("the game knows no key named " + keyName);
        }
        Object type = (Object) typeOfKey.invokeExact(found);
        Object name = (Object) nameOfKey.invokeExact(found);
        int value = (int) valueOfKey.invokeExact(found);
        if (!"KEYBOARD".equals(String.valueOf(type)) || !keyName.equals(name) || value <= 0 || value >= 512) {
            throw new IllegalArgumentException("not a keyboard key the game knows: " + keyName);
        }
        scancode = value;
    }

    private long now() {
        Long scripted = testNanos;
        return scripted != null ? scripted : System.nanoTime();
    }

    private boolean wanted() throws Throwable {
        if (scancode == 0) {
            bindKey();
        }
        Boolean scriptedPlaying = testPlaying;
        if (!(scriptedPlaying != null ? scriptedPlaying : client.playing())) {
            return false;
        }
        Boolean scriptedKey = testKey;
        return scriptedKey != null ? scriptedKey : (boolean) keyDown.invokeExact(scancode);
    }

    /** The world's field of view for this frame, given the one the game computed. */
    public float fov(float base) throws Throwable {
        long now = now();
        boolean want = wanted();
        if (want && !held) {
            factor = configuredFactor;
        }
        held = want;

        double target = want ? -Math.log(factor) : 0.0;
        double seconds = Math.min(Math.max((now - lastNanos) / 1e9, 0.0), MAX_STEP_SECONDS);
        lastNanos = now;
        level += (target - level) * (1.0 - Math.exp(-seconds / EASE_SECONDS));
        if (Math.abs(target - level) < 1e-3) {
            level = target;
        }
        if (level == 0.0) {
            turnScale = 1.0;
            return base;
        }
        float zoomed = (float) (base * Math.exp(level));
        // Angles are in degrees and below 180: the tangents are finite and positive.
        turnScale = Math.tan(Math.toRadians(zoomed) / 2.0) / Math.tan(Math.toRadians(base) / 2.0);
        turnScaleNanos = now;
        return zoomed;
    }

    /** What mouse movement is multiplied by while zoomed; exactly 1 otherwise. */
    public double turnScale() {
        boolean fresh = now() - turnScaleNanos < STALE_NANOS;
        return fresh && turnScale > 0.0 && turnScale < 1.0 ? turnScale : 1.0;
    }

    /**
     * Called before the game turns the player by the mouse movement it has accumulated
     * since the last frame (`mouse` is its MouseHandler): scales that movement down while
     * zoomed. The game resets the two sums itself afterwards.
     */
    public void turn(Object mouse) throws Throwable {
        double scale = turnScale();
        if (scale == 1.0) {
            return;
        }
        writeMoveX.invokeExact(mouse, (double) readMoveX.invokeExact(mouse) * scale);
        writeMoveY.invokeExact(mouse, (double) readMoveY.invokeExact(mouse) * scale);
    }

    /** A wheel movement. True when it changed the zoom factor and must not reach the game. */
    public boolean wheel(double amount) throws Throwable {
        if (!wheel || !held || amount == 0.0 || Double.isNaN(amount) || !wanted()) {
            return false;
        }
        double steps = Math.max(-4.0, Math.min(4.0, amount));
        factor = Math.max(MIN_FACTOR, Math.min(MAX_FACTOR, factor * Math.pow(WHEEL_STEP, steps)));
        return true;
    }

    /** The factor the key currently zooms to (changes with the wheel). */
    public double factor() {
        return factor;
    }

    /**
     * Calls the camera's own field-of-view method — the one the callback is injected into —
     * with a scripted key and clock, and the mouse-movement hook with a known movement.
     */
    @Override
    protected void probe(Object minecraft, List<String> out) throws Throwable {
        Object renderer = minecraft.getClass().getField("gameRenderer").get(minecraft);
        Object camera = renderer.getClass().getMethod("mainCamera").invoke(renderer);
        Method calculate = camera.getClass().getDeclaredMethod("calculateFov", float.class);
        calculate.setAccessible(true);
        Object mouse = minecraft.getClass().getField("mouseHandler").get(minecraft);
        // Outside a world the camera's own modifier (sprinting, flying, a spyglass) has
        // never been computed and is 0, which makes every angle 0. Give it the value it has
        // for a player standing still, for the duration of the measurement.
        Field modifier = camera.getClass().getDeclaredField("fovModifier");
        Field oldModifier = camera.getClass().getDeclaredField("oldFovModifier");
        modifier.setAccessible(true);
        oldModifier.setAccessible(true);
        float[] kept = {modifier.getFloat(camera), oldModifier.getFloat(camera)};
        modifier.setFloat(camera, 1.0f);
        oldModifier.setFloat(camera, 1.0f);
        try {
            testNanos = 1_000_000_000L;
            testKey = false;
            float idle = settle(calculate, camera);

            // The real state of the client: with a screen open, the key must do nothing.
            testKey = true;
            boolean screenOpen = client.screen(minecraft) != null;
            float keyWithRealState = settle(calculate, camera);

            // As if playing.
            testPlaying = true;
            float firstFrame = frame(calculate, camera);
            float zoomed = settle(calculate, camera);
            double scale = turnScale();
            writeMoveX.invokeExact(mouse, 100.0);
            writeMoveY.invokeExact(mouse, -40.0);
            Hooks.turn(mouse);
            double movedX = (double) readMoveX.invokeExact(mouse);
            double movedY = (double) readMoveY.invokeExact(mouse);
            writeMoveX.invokeExact(mouse, 0.0);
            writeMoveY.invokeExact(mouse, 0.0);

            boolean wheelUsed = Hooks.wheel(1.0);
            double wheelFactor = factor;
            float wheeled = settle(calculate, camera);

            testKey = false;
            float firstFrameOut = frame(calculate, camera);
            float after = settle(calculate, camera);
            boolean wheelUsedWhenReleased = Hooks.wheel(1.0);
            out.add(String.format(Locale.ROOT,
                    "zoom key=%s scancode=%d factor=%.3f fov_idle=%.3f screen_open=%b fov_key_held_real_state=%.3f"
                            + " fov_first_frame=%.3f fov_zoomed=%.3f turn_scale=%.5f moved=%.3f/%.3f"
                            + " wheel_used=%b wheel_factor=%.3f fov_wheel=%.3f fov_first_frame_out=%.3f fov_after=%.3f"
                            + " wheel_used_when_released=%b",
                    keyName, scancode, configuredFactor, idle, screenOpen, keyWithRealState, firstFrame, zoomed, scale,
                    movedX, movedY, wheelUsed, wheelFactor, wheeled, firstFrameOut, after, wheelUsedWhenReleased));
        } finally {
            modifier.setFloat(camera, kept[0]);
            oldModifier.setFloat(camera, kept[1]);
            testKey = null;
            testPlaying = null;
            testNanos = null;
            held = false;
            level = 0.0;
            turnScale = 1.0;
            factor = configuredFactor;
        }
    }

    /** Self-test only: add horizontal mouse movement for the game to act on in its next frame. */
    public void testNudge(Object mouse, double horizontal) throws Throwable {
        writeMoveX.invokeExact(mouse, (double) readMoveX.invokeExact(mouse) + horizontal);
    }

    /** One 16 ms frame on the scripted clock. */
    private float frame(Method calculate, Object camera) throws ReflectiveOperationException {
        testNanos += 16_000_000L;
        return (Float) calculate.invoke(camera, 1.0f);
    }

    /** A second of frames: long enough for any transition to finish. */
    private float settle(Method calculate, Object camera) throws ReflectiveOperationException {
        float last = 0.0f;
        for (int i = 0; i < 60; i++) {
            last = frame(calculate, camera);
        }
        return last;
    }
}
