package bastion.companion.platform;

/**
 * What the phone and the home screen need from the game, the same on every Minecraft version
 * the mod supports.
 *
 * One implementation (an <em>adapter</em>) exists per family of Minecraft versions in which
 * these things work alike; {@link Platforms} picks it when the mod starts. Code built on this
 * interface — the phone, its apps, the home screen, the theme — never names a game class and
 * so runs unchanged on every family. Where an adapter cannot do something on its version, the
 * method says what happens instead (nothing is drawn, {@code false} is returned); it never
 * throws into the caller, and a failure inside the game is logged once and switches the
 * failing part off, as for the mod's features.
 *
 * <p><b>Threads.</b> Everything here belongs to the game's render thread, which is the thread
 * that calls {@link Overlay}, {@link HudLayer} and the phone-key callback. The only exceptions
 * are {@link #minecraftVersion()} and {@link #family()}, which may be called from anywhere.
 * Work from another thread (the launcher link's reader, say) reaches the render thread
 * through {@link #runOnRenderThread(Runnable)}.
 *
 * <p><b>Units.</b> Coordinates are the game's GUI units (scaled pixels, what the game's own
 * screens use), origin at the top left. Colours are {@code 0xAARRGGBB}; an alpha of 0 is
 * invisible.
 */
public interface Platform {
    /** The Minecraft version the game reports, such as {@code "1.21.1"} or {@code "26.3"}. */
    String minecraftVersion();

    /**
     * The family this adapter serves, as a short label (such as {@code "1.20.1-1.20.4"}); for
     * the log and the hello to the launcher.
     */
    String family();

    // ------------------------------------------------------------------ screens

    /**
     * Shows {@code overlay} as a screen in front of the game that does not pause it (the world
     * keeps running behind it in single-player too). The screen that was open before, if any,
     * comes back when the overlay closes. Opening an overlay that is already open does nothing.
     */
    void open(Overlay overlay);

    /** Closes {@code overlay} if it is the screen that is open; otherwise does nothing. */
    void close(Overlay overlay);

    /** Whether {@code overlay} is the screen that is open now. */
    boolean isOpen(Overlay overlay);

    /** In a world with nothing in front of it: no screen, no loading overlay. */
    boolean playing();

    /** In a world (a player exists), whether or not a screen is open. */
    boolean inWorld();

    // ------------------------------------------------------------------ the home screen

    /**
     * Shows {@code home} wherever the game would show its title screen (at start-up, after
     * leaving a world or a server). {@code null} gives the game's own title screen back. The
     * overlay should offer the {@link HomeAction}s that are {@link #available(HomeAction)}.
     */
    void setHome(Overlay home);

    /** Whether the game offers {@code action} now (demo mode, for one, has no multiplayer). */
    boolean available(HomeAction action);

    /** Does what the title screen's button for {@code action} does. */
    void perform(HomeAction action);

    /**
     * The game's own caption for {@code action} in the player's language (such as
     * "Singleplayer"), or an English one where the game has none.
     */
    String label(HomeAction action);

    /** The game runs in Mojang's demo mode (no account). */
    boolean demo();

    // ------------------------------------------------------------------ the HUD

    /**
     * Draws {@code layer} on top of the game's HUD in every frame in which the HUD is drawn
     * (in a world, the HUD not hidden with F1). {@code null}: nothing.
     */
    void setHud(HudLayer layer);

    // ------------------------------------------------------------------ the game's own menus

    /**
     * {@code painter} draws the background behind the game's own screens (the options, the
     * world list, …) instead of the game. {@code null}: the game's own background.
     */
    void setMenuBackground(MenuBackground painter);

    // ------------------------------------------------------------------ the phone key

    /**
     * {@code onPress} runs when the phone key (the launcher's setting {@code phone.key}) is
     * pressed while playing — and while one of this mod's overlays is open, when the overlay
     * did not use the key itself ({@link Overlay#keyPressed} returned {@code false}), so that
     * the same key puts the phone away.
     */
    void setPhoneKey(Runnable onPress);

    /** The phone key's name as the game shows it (such as {@code "M"}), for hints. */
    String phoneKeyName();

    // ------------------------------------------------------------------ pictures

    /**
     * A texture holding {@code pixels}, for {@link Canvas#image}. Returns {@code null} where the
     * adapter cannot make one (the picture is then simply not drawn). Release it with
     * {@link Texture#close()} when it is no longer needed.
     */
    Texture texture(Pixels pixels);

    // ------------------------------------------------------------------ odds and ends

    /**
     * Real pixels per GUI unit (the game's GUI scale, 1 to 6 or so; fractional where the game
     * allows it). A picture drawn {@code w / guiScale()} units wide is drawn pixel for pixel.
     */
    double guiScale();

    /** Plays the game's button click (where there is sound). */
    void clickSound();

    /**
     * Runs {@code task} on the render thread, soon; from any thread. Exceptions it throws are
     * logged (once per kind) and do not reach the game.
     */
    void runOnRenderThread(Runnable task);
}
