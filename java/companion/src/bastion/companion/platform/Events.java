package bastion.companion.platform;

/**
 * Where the families' mixins call in: thin, static, and harmless before the platform exists
 * (the game's own behaviour then). The work is {@link Core}'s.
 */
public final class Events {
    private Events() {
    }

    /** {@code Minecraft.setScreen(screen)} is starting: the screen to open instead, or {@code screen}. */
    public static Object setScreen(Object screen) {
        Core core = Core.get();
        return core == null ? screen : core.onSetScreen(screen);
    }

    /** Opens {@code screen} in place of the one {@link #setScreen} turned down. */
    public static void reopen(Object screen) {
        Core core = Core.get();
        if (core == null) {
            return;
        }
        try {
            core.glue().setScreen(screen);
        } catch (Throwable failure) {
            core.problem("opening the home screen", failure);
        }
    }

    /** The start of a frame. */
    public static void frame() {
        Core core = Core.get();
        if (core != null) {
            core.onFrame();
        }
    }

    /** The game's HUD has been drawn (not hidden) in this frame. */
    public static void hud(Canvas canvas, boolean screenOpen) {
        Core core = Core.get();
        if (core != null) {
            core.onHud(canvas, screenOpen);
        }
    }

    /** {@code screen}'s background is about to be drawn: true when it was drawn here instead. */
    public static boolean background(Canvas canvas, Object screen) {
        Core core = Core.get();
        return core != null && core.onBackground(canvas, screen);
    }

    /** Whether there is a platform that wants to know (saves building a canvas for nothing). */
    public static boolean live() {
        return Core.get() != null;
    }
}
