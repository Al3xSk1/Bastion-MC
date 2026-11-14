package bastion.companion.platform;

/** Captions of the title screen's buttons: the game's translation keys and English fallbacks. */
public final class HomeLabels {
    private HomeLabels() {
    }

    /** The translation key the game's title screen uses for {@code action}, or null. */
    public static String key(HomeAction action) {
        switch (action) {
            case SINGLEPLAYER: return "menu.singleplayer";
            case MULTIPLAYER: return "menu.multiplayer";
            case REALMS: return "menu.online";
            case OPTIONS: return "menu.options";
            case LANGUAGE: return "options.language";
            case ACCESSIBILITY: return "options.accessibility";
            case PLAY_DEMO: return "menu.playdemo";
            case QUIT: return "menu.quit";
            default: return null;
        }
    }

    /** An English caption. */
    public static String english(HomeAction action) {
        switch (action) {
            case SINGLEPLAYER: return "Singleplayer";
            case MULTIPLAYER: return "Multiplayer";
            case REALMS: return "Minecraft Realms";
            case OPTIONS: return "Options...";
            case LANGUAGE: return "Language...";
            case ACCESSIBILITY: return "Accessibility Settings...";
            case PLAY_DEMO: return "Play Demo World";
            case MODS: return "Mods";
            case CREDITS: return "Credits";
            case CLASSIC_TITLE: return "Classic Title Screen";
            case QUIT: return "Quit Game";
            default: return action.name();
        }
    }
}
