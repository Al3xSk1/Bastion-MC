package bastion.companion.platform;

/**
 * A picture the game can draw ({@link Canvas#image}), made by {@link Platform#texture}. Render
 * thread only. A texture that is closed, or whose adapter failed, draws nothing.
 */
public interface Texture {
    int width();

    int height();

    /**
     * Replaces the picture. Of the same size, the existing texture is updated in place (cheap
     * enough for a browser page fifteen times a second); otherwise it is made anew.
     */
    void replace(Pixels pixels);

    /** Releases the texture's memory in the game. Drawing it afterwards draws nothing. */
    void close();
}
