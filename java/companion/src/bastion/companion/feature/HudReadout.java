package bastion.companion.feature;

import java.lang.invoke.MethodHandle;
import java.util.List;
import java.util.Locale;

import bastion.companion.Client;
import bastion.companion.Feature;
import bastion.companion.Game;

/**
 * One line or three in the top-left corner while in a world: frames per second, and (unless
 * the game withholds them) the player's position and facing.
 *
 * Position and facing are what the debug screen (F3) shows, and are withheld under the
 * same condition: when the server (or the player's own option) asks for reduced debug
 * information, only the frame rate is shown. Nothing is drawn while the HUD is hidden (F1)
 * or the game's own debug overlay shows anything — F3, or debug entries the player has set
 * to show always ({@code DebugScreenOverlay.showDebugScreen} is true for both): the two
 * would be drawn over each other.
 *
 * Drawn at the end of {@code Hud.extractRenderState}, through the game's own text call. The
 * text is rebuilt ten times a second, not every frame.
 */
public final class HudReadout extends Feature {
    private static final long REFRESH_NANOS = 100_000_000L;
    private static final int WHITE = 0xFFFFFFFF;
    private static final int LEFT = 2;
    private static final int TOP = 2;
    private static final int LINE_HEIGHT = 10;
    private static final String[] COMPASS = {"south (+Z)", "west (-X)", "north (-Z)", "east (+X)"};

    private Client client;
    private MethodHandle hidden;
    private MethodHandle debugOverlay;
    private MethodHandle debugShown;
    private MethodHandle fps;
    private MethodHandle reduced;
    private MethodHandle font;
    private MethodHandle text;
    private MethodHandle x;
    private MethodHandle y;
    private MethodHandle z;
    private MethodHandle yaw;
    private MethodHandle pitch;

    private String[] lines = new String[0];
    private long builtNanos;
    private long drawn;

    public HudReadout() {
        super("hud");
    }

    @Override
    protected boolean writtenFor26_3() {
        return true;
    }

    @Override
    protected void resolve(Game game) throws Throwable {
        client = new Client(game);
        Class<?> minecraft = game.type("net.minecraft.client.Minecraft");
        Class<?> hud = game.type("net.minecraft.client.gui.Hud");
        Class<?> overlay = game.type("net.minecraft.client.gui.components.DebugScreenOverlay");
        Class<?> fontType = game.type("net.minecraft.client.gui.Font");
        Class<?> graphics = game.type("net.minecraft.client.gui.GuiGraphicsExtractor");
        Class<?> entity = game.type("net.minecraft.world.entity.Entity");
        hidden = game.virtual(hud, "isHidden", boolean.class);
        debugOverlay = game.virtual(minecraft, "getDebugOverlay", overlay);
        debugShown = game.virtual(overlay, "showDebugScreen", boolean.class);
        fps = game.virtual(minecraft, "getFps", int.class);
        reduced = game.virtual(minecraft, "showOnlyReducedInfo", boolean.class);
        font = game.getter(minecraft, "font", fontType);
        text = game.virtual(graphics, "text", void.class, fontType, String.class, int.class, int.class, int.class,
                boolean.class);
        x = game.virtual(entity, "getX", double.class);
        y = game.virtual(entity, "getY", double.class);
        z = game.virtual(entity, "getZ", double.class);
        yaw = game.virtual(entity, "getYRot", float.class);
        pitch = game.virtual(entity, "getXRot", float.class);
        game.requireHook(hud, "bastion$hudReadout", "extractRenderState", void.class, graphics,
                game.type("net.minecraft.client.DeltaTracker"));
    }

    /** Called once per frame with the game's HUD object and the drawing context of that frame. */
    public void draw(Object hud, Object graphics) throws Throwable {
        Object minecraft = client.minecraft();
        if (minecraft == null || (boolean) hidden.invokeExact(hud)) {
            return;
        }
        Object debug = (Object) debugOverlay.invokeExact(minecraft);
        if (debug != null && (boolean) debugShown.invokeExact(debug)) {
            return;
        }
        long now = System.nanoTime();
        if (now - builtNanos >= REFRESH_NANOS || lines.length == 0) {
            lines = describe(minecraft);
            builtNanos = now;
        }
        Object face = (Object) font.invokeExact(minecraft);
        if (face == null) {
            return;
        }
        for (int i = 0; i < lines.length; i++) {
            text.invokeExact(graphics, face, (Object) lines[i], LEFT, TOP + i * LINE_HEIGHT, WHITE, true);
        }
        drawn++;
    }

    /** The lines for the client's current state. */
    public String[] describe(Object minecraft) throws Throwable {
        String rate = (int) fps.invokeExact(minecraft) + " fps";
        Object player = client.player(minecraft);
        if (player == null || (boolean) reduced.invokeExact(minecraft)) {
            return new String[] {rate};
        }
        double px = (double) x.invokeExact(player);
        double py = (double) y.invokeExact(player);
        double pz = (double) z.invokeExact(player);
        float turn = (float) yaw.invokeExact(player);
        float tilt = (float) pitch.invokeExact(player);
        return new String[] {
            rate,
            String.format(Locale.ROOT, "XYZ %.1f / %.1f / %.1f", px, py, pz),
            String.format(Locale.ROOT, "facing %s, yaw %.1f, pitch %.1f", compass(turn), wrap(turn), tilt),
        };
    }

    /** Minecraft's yaw grows clockwise seen from above, with 0 looking along +Z (south). */
    static String compass(float yawDegrees) {
        if (Float.isNaN(yawDegrees) || Float.isInfinite(yawDegrees)) {
            return "?";
        }
        return COMPASS[Math.floorMod(Math.round(yawDegrees / 90.0f), 4)];
    }

    /** The angle in [-180, 180). */
    static float wrap(float degrees) {
        float wrapped = degrees % 360.0f;
        if (wrapped >= 180.0f) {
            wrapped -= 360.0f;
        } else if (wrapped < -180.0f) {
            wrapped += 360.0f;
        }
        return wrapped;
    }

    /** How many frames the readout has been drawn in (the self-test reads this). */
    public long drawn() {
        return drawn;
    }

    @Override
    protected void probe(Object minecraft, List<String> out) throws Throwable {
        out.add("hud lines=[" + String.join(" | ", describe(minecraft)) + "] frames_drawn=" + drawn);
    }
}
