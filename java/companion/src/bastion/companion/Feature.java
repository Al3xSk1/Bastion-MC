package bastion.companion;

/**
 * One convenience, switched on or off by the launcher's configuration.
 *
 * A feature is a class of its own that (1) reads its settings, (2) resolves the game
 * members it needs through {@link Game}, and (3) is called from {@link Hooks}. Any failure
 * — a name that no longer resolves at start-up, an exception in a hook later — switches
 * that one feature off for the rest of the session with a single log line and a single
 * diagnostic to the launcher. It never reaches the game as an exception.
 *
 * To add one: a subclass, an entry in {@link Hooks#ALL}, a hook method there if it needs a
 * new injection point (plus the mixin that calls it), and its switch in the launcher
 * (`CompanionConfig` in `src/companion/mod.rs`, which lists the places).
 */
public abstract class Feature {
    private final String id;
    /** Read on the render thread every frame; written at start-up and on failure. */
    private volatile boolean live;

    protected Feature(String id) {
        this.id = id;
    }

    /** The name of the switch in the launcher's configuration. */
    public final String id() {
        return id;
    }

    public final boolean live() {
        return live;
    }

    /**
     * Whether this feature reaches the game through 26.3's names (the zoom, the HUD readout
     * and fullbright do): it is then started on that version only, and elsewhere reported as
     * not available. Features built on the platform API run on every family.
     */
    protected boolean writtenFor26_3() {
        return false;
    }

    /** Read settings. Called once, before {@link #resolve}. */
    protected void configure(Config config) {
    }

    /** Resolve everything this feature will call. Throwing means: not available here. */
    protected abstract void resolve(Game game) throws Throwable;

    /**
     * Self-test only (see {@link SelfTest}): exercise this feature against the running client
     * without a world, on the render thread, and add what was measured to `out`.
     */
    protected void probe(Object minecraft, java.util.List<String> out) throws Throwable {
    }

    final void start(Config config, Game game) {
        try {
            configure(config);
            resolve(game);
            live = true;
        } catch (Throwable failure) {
            Companion.disabled(this, "lookup failed (" + Log.describe(failure) + ")");
        }
    }

    /** A hook of this feature threw: stop calling it. */
    final void failed(String where, Throwable failure) {
        if (live) {
            live = false;
            Companion.disabled(this, where + " failed (" + Log.describe(failure) + ")");
        }
    }
}
