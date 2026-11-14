package bastion.companion.platform;

/** What the game's title screen offers, for a home screen that replaces it ({@link Platform#perform}). */
public enum HomeAction {
    /** The world list. */
    SINGLEPLAYER,
    /** The server list. */
    MULTIPLAYER,
    /** Realms (not in demo mode). */
    REALMS,
    /** The options screen. */
    OPTIONS,
    /** The language screen. */
    LANGUAGE,
    /** The accessibility options. */
    ACCESSIBILITY,
    /** Demo mode only: open (or create) the demo world. */
    PLAY_DEMO,
    /** The list of mods, where a mod that shows one (Mod Menu) is installed. */
    MODS,
    /** The credits and attribution (what the copyright line of the title screen opens). */
    CREDITS,
    /** The game's own title screen, once (until the home screen is asked for again). */
    CLASSIC_TITLE,
    /** Quit the game. */
    QUIT
}
