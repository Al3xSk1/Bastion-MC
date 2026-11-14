package bastion.companion.platform;

/**
 * A screen of this mod — the phone, the home screen — shown by {@link Platform#open} or
 * {@link Platform#setHome}. The adapter wraps it in a screen of the game's own kind and calls
 * these methods on the render thread.
 *
 * Input arrives normalised, whatever the version: keys in {@link Keys} numbering, mouse
 * buttons {@link Keys#MOUSE_LEFT}, {@link Keys#MOUSE_RIGHT}, {@link Keys#MOUSE_MIDDLE},
 * modifiers as {@link Keys#MOD_SHIFT} and friends, positions in GUI units. A method returns
 * {@code true} when it used the event (it then goes no further).
 *
 * An exception thrown from any of these methods closes the overlay and is logged once; it
 * does not reach the game.
 */
public interface Overlay {
    /**
     * The overlay is about to be shown, or the window changed size: the drawing area is now
     * {@code width} by {@code height}.
     */
    default void opened(int width, int height) {
    }

    /** Draws a frame. The mouse is at (mouseX, mouseY); {@code partialTick} is in [0, 1). */
    void render(Canvas canvas, int mouseX, int mouseY, float partialTick);

    /** Twenty times a second while shown. */
    default void tick() {
    }

    default boolean mousePressed(double x, double y, int button, int modifiers) {
        return false;
    }

    default boolean mouseReleased(double x, double y, int button, int modifiers) {
        return false;
    }

    /** The mouse moved with {@code button} held, by (dx, dy). */
    default boolean mouseDragged(double x, double y, int button, double dx, double dy) {
        return false;
    }

    /** The wheel turned: positive {@code vertical} is away from the user (scroll up). */
    default boolean mouseScrolled(double x, double y, double horizontal, double vertical) {
        return false;
    }

    /**
     * A key went down (or repeats while held). {@code key} is a {@link Keys} constant, or
     * {@link Keys#UNKNOWN} for a key without one.
     */
    default boolean keyPressed(int key, int modifiers) {
        return false;
    }

    default boolean keyReleased(int key, int modifiers) {
        return false;
    }

    /** A character was typed (a Unicode code point; already composed by the system). */
    default boolean charTyped(int codePoint, int modifiers) {
        return false;
    }

    /** Escape closes the overlay unless this returns false (and Escape then goes to {@link #keyPressed}). */
    default boolean closesOnEscape() {
        return true;
    }

    /**
     * Whether the game draws its usual background behind the overlay (in a world: the dimmed,
     * on newer versions blurred, view of it). False: the world shows through unchanged, which
     * suits a phone held in front of it.
     */
    default boolean gameBackground() {
        return false;
    }

    /** The overlay was closed (by {@link Platform#close}, Escape, or another screen replacing it). */
    default void closed() {
    }
}
