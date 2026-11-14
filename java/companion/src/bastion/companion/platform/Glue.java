package bastion.companion.platform;

/**
 * What one family of Minecraft versions has to provide for the platform: the operations that
 * name game classes. Implemented once per family (a class {@code FamilyGlue} in the package
 * {@code bastion.companion.platform.<family>}, most of it shared by the families of a group);
 * {@link Core} builds the whole {@link Platform} on these.
 *
 * Game objects travel through here as {@code Object}: this class is compiled without the
 * game. Everything is called on the render thread unless said otherwise, and may throw — the
 * caller ({@link Core}) contains it.
 */
public abstract class Glue {
    /** The family's id, as in families.txt (such as {@code "v1_20_1"}). */
    public abstract String family();

    // ------------------------------------------------------------------ screens

    /** The screen that is open, or null. */
    public abstract Object screen() throws Throwable;

    /** Opens {@code screen} (null: none) the way the game does. */
    public abstract void setScreen(Object screen) throws Throwable;

    /** A screen of the game's kind that shows {@code host}. */
    public abstract Object newHostScreen(Host host) throws Throwable;

    /** The host if {@code screen} is one of this mod's screens, else null. */
    public Host hostOf(Object screen) {
        return screen instanceof Hosted ? ((Hosted) screen).host() : null;
    }

    /** Whether {@code screen} is the game's own title screen (exactly that class). */
    public abstract boolean isTitleScreen(Object screen);

    /** A new title screen of the game's own. */
    public abstract Object newTitleScreen() throws Throwable;

    /** A local player exists (a world is loaded). */
    public abstract boolean hasPlayer() throws Throwable;

    /** A loading overlay is in front of everything. */
    public abstract boolean loading() throws Throwable;

    /** Mojang's demo mode. */
    public abstract boolean demo() throws Throwable;

    /** Real pixels per GUI unit. */
    public abstract double guiScale() throws Throwable;

    // ------------------------------------------------------------------ keys

    /**
     * The number by which the game reads the keyboard key named {@code name} (the name in
     * options.txt, such as {@code key.keyboard.r}), or -1 if it is not a keyboard key the game
     * knows. The number is the family's own (GLFW key code up to 1.21.x, SDL scan code later):
     * compare it only with {@link #keyDown} and with the raw codes a host screen receives.
     */
    public abstract int keyCode(String name) throws Throwable;

    /** The game's display name of that key ("R"). */
    public abstract String keyLabel(String name) throws Throwable;

    /** Whether the key with that number is held now. */
    public abstract boolean keyDown(int code) throws Throwable;

    // ------------------------------------------------------------------ the title screen's actions

    public abstract boolean available(HomeAction action) throws Throwable;

    /** Does {@code action}; {@code parent} is the screen to come back to (the home screen). */
    public abstract void perform(HomeAction action, Object parent) throws Throwable;

    /** The game's translation of {@code key} in the player's language, or null. */
    public abstract String translate(String key) throws Throwable;

    // ------------------------------------------------------------------ odds and ends

    public abstract void clickSound() throws Throwable;

    /** Runs {@code task} on the render thread, soon. Any thread. */
    public abstract void execute(Runnable task);

    /** A texture with {@code pixels}, or null where the family cannot make one. */
    public abstract Texture texture(Pixels pixels) throws Throwable;
}
