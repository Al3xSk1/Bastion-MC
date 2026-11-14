package bastion.companion.platform;

/** Drawing on top of the game's HUD, every frame in which the HUD is drawn ({@link Platform#setHud}). */
public interface HudLayer {
    /**
     * Draws this frame's part. {@code screenOpen} tells whether a screen is in front of the
     * world (the HUD is drawn under it). Exceptions switch the layer off and are logged once.
     */
    void render(Canvas canvas, float partialTick, boolean screenOpen);
}
