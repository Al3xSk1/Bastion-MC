package bastion.companion.platform;

/** Paints the background behind the game's own screens ({@link Platform#setMenuBackground}). */
public interface MenuBackground {
    /**
     * Draws the background of a screen of the game's own, instead of the game. Returns false
     * to have the game's background drawn after all (this frame). {@code inWorld}: a world is
     * running behind the screen (the pause menu, the inventory's options …). Exceptions switch
     * the painter off and are logged once.
     */
    boolean paint(Canvas canvas, boolean inWorld);
}
