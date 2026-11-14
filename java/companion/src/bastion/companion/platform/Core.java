package bastion.companion.platform;

import java.util.HashSet;
import java.util.Set;

import bastion.companion.Config;
import bastion.companion.Log;

/**
 * The {@link Platform}, built once on one family's {@link Glue}: which overlay is open and
 * what comes back when it closes, the home screen in place of the title screen, the HUD layer
 * and the menu background, the phone key, the launcher link's deliveries — the same on every
 * version. The family's mixins call the {@code on…} methods; nothing here names a game class.
 *
 * Whatever fails inside — the glue, an overlay, a layer — is logged once per kind and switches
 * that part off. No exception leaves a hook into the game.
 */
public final class Core implements Platform {
    private static volatile Core instance;

    private final Glue glue;
    private final String version;
    private final String phoneKeyName;
    private final Set<String> reported = new HashSet<>();

    private Host open;
    private Host homeHost;
    private Overlay home;
    /** The game's own title screen was asked for: leave it alone until a world has been entered. */
    private boolean classic;
    /** A title screen to let through once (see {@link #showClassicTitle}). */
    private Object passThrough;
    /** The overlay whose screen the game took away, to be settled at the next frame. */
    private Host replaced;
    /** Counters for the self-test: frames begun, HUD frames drawn, title screens asked for. */
    private volatile long frames;
    private volatile long hudFrames;
    private volatile long titles;
    private volatile long backgrounds;
    private HudLayer hud;
    private MenuBackground background;
    private Runnable phoneKey;
    /** The phone key's number in the family's numbering: 0 not looked up yet, -1 none. */
    private int phoneCode;
    private boolean phoneDown;

    Core(Glue glue, String version, Config config) {
        this.glue = glue;
        this.version = version;
        this.phoneKeyName = config.text("phone.key", "key.keyboard.r");
    }

    /** The platform the family's mixins report to; null before the mod has started one. */
    public static Core get() {
        return instance;
    }

    /** Makes {@code glue}'s platform the one of this game ({@link Platforms#current()}). */
    public static Core start(Glue glue, String version, Config config) {
        Core core = new Core(glue, version, config);
        instance = core;
        Platforms.install(core);
        return core;
    }

    public Glue glue() {
        return glue;
    }

    /** Logs a failure once per kind of place. */
    void problem(String where, Throwable failure) {
        synchronized (reported) {
            if (!reported.add(where)) {
                return;
            }
        }
        Log.warn("platform (" + glue.family() + "): " + where + " failed (" + Log.describe(failure) + ")");
    }

    // ------------------------------------------------------------------ Platform

    @Override
    public String minecraftVersion() {
        return version;
    }

    @Override
    public String family() {
        return glue.family();
    }

    @Override
    public void open(Overlay overlay) {
        if (overlay == null || (open != null && open.overlay() == overlay)) {
            return;
        }
        try {
            Host host = new Host(this, overlay, false);
            host.previous = glue.screen();
            host.screen = glue.newHostScreen(host);
            Host before = open;
            open = host;
            glue.setScreen(host.screen);
            if (before != null && before != host && glue.hostOf(host.previous) != before) {
                before.closed();
            }
        } catch (Throwable failure) {
            open = null;
            problem("opening an overlay", failure);
        }
    }

    @Override
    public void close(Overlay overlay) {
        Host host = open;
        if (host == null || host.overlay() != overlay) {
            return;
        }
        open = null;
        try {
            Object back = host.previous;
            Host underneath = glue.hostOf(back);
            if (underneath != null) {
                open = underneath;
            }
            glue.setScreen(back);
        } catch (Throwable failure) {
            problem("closing an overlay", failure);
        }
        host.closed();
    }

    /** A host failed: close it whatever it is. */
    void closeFailed(Host host) {
        if (host == homeHost) {
            homeHost = null;
            home = null;
            try {
                if (glue.hostOf(glue.screen()) == host) {
                    classic = true;
                    glue.setScreen(glue.newTitleScreen());
                }
            } catch (Throwable failure) {
                problem("falling back to the title screen", failure);
            }
            return;
        }
        if (open == host) {
            close(host.overlay());
        }
    }

    /**
     * The game is putting another screen up in place of {@code host}'s. The game tells the old
     * screen before it records the new one, so what replaced it is looked at at the start of
     * the next frame ({@link #onFrame}).
     */
    void removed(Host host) {
        if (open == host) {
            replaced = host;
        }
    }

    /** At the start of a frame: an overlay the game replaced (by a screen not of ours) is closed. */
    private void settleReplaced() {
        Host host = replaced;
        replaced = null;
        if (host == null || open != host) {
            return;
        }
        Object now;
        try {
            now = glue.screen();
        } catch (Throwable failure) {
            now = null;
        }
        if (now != host.screen && (now == null || glue.hostOf(now) == null)) {
            open = null;
            host.closed();
        }
    }

    @Override
    public boolean isOpen(Overlay overlay) {
        Host host = open;
        if (host == null || host.overlay() != overlay) {
            return false;
        }
        try {
            return glue.screen() == host.screen;
        } catch (Throwable failure) {
            problem("reading the open screen", failure);
            return false;
        }
    }

    @Override
    public boolean playing() {
        try {
            return glue.hasPlayer() && glue.screen() == null && !glue.loading();
        } catch (Throwable failure) {
            problem("reading the game's state", failure);
            return false;
        }
    }

    @Override
    public boolean inWorld() {
        try {
            return glue.hasPlayer();
        } catch (Throwable failure) {
            problem("reading the game's state", failure);
            return false;
        }
    }

    @Override
    public void setHome(Overlay overlay) {
        home = overlay;
        homeHost = null;
    }

    @Override
    public boolean available(HomeAction action) {
        try {
            return glue.available(action);
        } catch (Throwable failure) {
            problem("asking about " + action, failure);
            return false;
        }
    }

    @Override
    public void perform(HomeAction action) {
        try {
            if (action == HomeAction.CLASSIC_TITLE) {
                classic = true;
                glue.setScreen(glue.newTitleScreen());
                return;
            }
            glue.perform(action, glue.screen());
        } catch (Throwable failure) {
            problem("the title screen's action " + action, failure);
        }
    }

    @Override
    public String label(HomeAction action) {
        String key = HomeLabels.key(action);
        if (key != null) {
            try {
                String text = glue.translate(key);
                if (text != null && !text.isEmpty() && !text.equals(key)) {
                    return text;
                }
            } catch (Throwable failure) {
                problem("translating " + key, failure);
            }
        }
        return HomeLabels.english(action);
    }

    @Override
    public boolean demo() {
        try {
            return glue.demo();
        } catch (Throwable failure) {
            problem("asking about demo mode", failure);
            return false;
        }
    }

    @Override
    public void setHud(HudLayer layer) {
        hud = layer;
    }

    @Override
    public void setMenuBackground(MenuBackground painter) {
        background = painter;
    }

    @Override
    public void setPhoneKey(Runnable onPress) {
        phoneKey = onPress;
    }

    @Override
    public String phoneKeyName() {
        try {
            String label = glue.keyLabel(phoneKeyName);
            return label == null ? "" : label;
        } catch (Throwable failure) {
            problem("naming the phone key", failure);
            return "";
        }
    }

    @Override
    public Texture texture(Pixels pixels) {
        if (pixels == null) {
            return null;
        }
        try {
            return glue.texture(pixels);
        } catch (Throwable failure) {
            problem("making a texture", failure);
            return null;
        }
    }

    @Override
    public void clickSound() {
        try {
            glue.clickSound();
        } catch (Throwable failure) {
            problem("the click sound", failure);
        }
    }

    @Override
    public double guiScale() {
        try {
            return glue.guiScale();
        } catch (Throwable failure) {
            problem("reading the GUI scale", failure);
            return 1.0;
        }
    }

    @Override
    public void runOnRenderThread(Runnable task) {
        if (task == null) {
            return;
        }
        glue.execute(() -> {
            try {
                task.run();
            } catch (Throwable failure) {
                problem("a task on the render thread", failure);
            }
        });
    }

    // ------------------------------------------------------------------ the phone key

    boolean isPhoneKey(int raw) {
        return phoneCode > 0 && raw == phoneCode;
    }

    void phoneKeyPressed() {
        Runnable action = phoneKey;
        if (action == null) {
            return;
        }
        try {
            action.run();
        } catch (Throwable failure) {
            phoneKey = null;
            problem("the phone key's action", failure);
        }
    }

    // ------------------------------------------------------------------ hooks (the mixins call these)

    /**
     * Opens the game's own title screen {@code title} without putting the home screen in its
     * place — once, for the adapters that press one of its buttons.
     */
    public void showClassicTitle(Object title) {
        passThrough = title;
        try {
            glue.setScreen(title);
        } catch (Throwable failure) {
            problem("opening the title screen", failure);
        } finally {
            passThrough = null;
        }
    }

    /** Frames begun since the platform started (the self-test reads this). */
    public long frames() {
        return frames;
    }

    /** Frames in which the HUD was drawn (and the layer called, if there is one). */
    public long hudFrames() {
        return hudFrames;
    }

    /** How often the game asked for its title screen. */
    public long titles() {
        return titles;
    }

    /** Backgrounds of the game's own screens drawn by the painter instead of the game. */
    public long backgrounds() {
        return backgrounds;
    }

    /** The game is about to open {@code screen}: what to open instead (or {@code screen} itself). */
    public Object onSetScreen(Object screen) {
        try {
            if (screen == null || screen == passThrough) {
                return screen;
            }
            if (glue.isTitleScreen(screen)) {
                titles++;
                if (home == null || classic) {
                    return screen;
                }
                if (homeHost == null) {
                    homeHost = new Host(this, home, true);
                    homeHost.screen = glue.newHostScreen(homeHost);
                }
                return homeHost.screen;
            }
        } catch (Throwable failure) {
            home = null;
            homeHost = null;
            problem("replacing the title screen", failure);
        }
        return screen;
    }

    /** Once per frame, at its start. */
    public void onFrame() {
        frames++;
        settleReplaced();
        try {
            if (classic && glue.hasPlayer()) {
                classic = false;
            }
            if (phoneCode == 0) {
                phoneCode = Math.max(-1, glue.keyCode(phoneKeyName));
                if (phoneCode < 0) {
                    Log.warn("the phone key " + phoneKeyName + " is not a keyboard key of this game; the phone has no key");
                }
            }
            if (phoneCode > 0) {
                boolean down = glue.keyDown(phoneCode);
                if (down && !phoneDown && playing()) {
                    phoneKeyPressed();
                }
                phoneDown = down;
            }
        } catch (Throwable failure) {
            phoneCode = -1;
            problem("watching the phone key", failure);
        }
        Link line = Platforms.link();
        if (line instanceof Delivering) {
            try {
                ((Delivering) line).deliver();
            } catch (Throwable failure) {
                problem("delivering what the launcher sent", failure);
            }
        }
    }

    /** At the end of the game's HUD, in a frame in which it was drawn. */
    public void onHud(Canvas canvas, boolean screenOpen) {
        hudFrames++;
        HudLayer layer = hud;
        if (layer == null) {
            return;
        }
        try {
            layer.render(canvas, 0.0f, screenOpen);
        } catch (Throwable failure) {
            hud = null;
            problem("the HUD layer", failure);
        }
    }

    /**
     * The game is about to draw {@code screen}'s background. True: it is drawn here, the game
     * must not.
     */
    public boolean onBackground(Canvas canvas, Object screen) {
        Host host = glue.hostOf(screen);
        if (host != null) {
            return !host.gameBackground();
        }
        MenuBackground painter = background;
        if (painter == null) {
            return false;
        }
        try {
            boolean painted = painter.paint(canvas, glue.hasPlayer());
            if (painted) {
                backgrounds++;
            }
            return painted;
        } catch (Throwable failure) {
            background = null;
            problem("the menu background", failure);
            return false;
        }
    }

    /** A {@link Link} that hands what it received to its listener when asked, on the render thread. */
    public interface Delivering {
        void deliver();
    }
}
