import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Commands;
import bastion.companion.platform.HomeAction;
import bastion.companion.platform.HomeLabels;
import bastion.companion.platform.HudLayer;
import bastion.companion.platform.Json;
import bastion.companion.platform.Keys;
import bastion.companion.platform.Link;
import bastion.companion.platform.MenuBackground;
import bastion.companion.platform.Overlay;
import bastion.companion.platform.Pixels;
import bastion.companion.platform.Platform;
import bastion.companion.platform.Texture;

/**
 * A stand-in {@link Platform} for testing what is built on the platform API — the phone, its
 * apps, the home screen — without Minecraft: it keeps one "open screen", draws into a list of
 * readable operations, and lets a test play the game's part (a frame, a key, a click, the
 * title screen appearing, the phone key).
 *
 * Use: compile against the embedded jar (as tests/companion.rs does for the other test
 * programs), construct, hand it to the code under test, drive it.
 *
 * <pre>
 *   FakePlatform game = new FakePlatform(480, 270);
 *   Ui.start(game, game.link, config);       // or construct the screen directly
 *   game.showTitle();                         // the game would show its title screen now
 *   List&lt;String&gt; drawn = game.frame();    // "fill 0,0,480,270 ff101418", "text ..."
 *   game.click(240, 135);
 * </pre>
 *
 * Every drawing operation is recorded as one line:
 * {@code fill x0,y0,x1,y1 aarrggbb}, {@code text "..." x,y aarrggbb [shadow]},
 * {@code image <id> x,y wxh}, {@code clip x0,y0,x1,y1}, {@code unclip}, {@code push},
 * {@code pop}, {@code translate dx,dy}, {@code scale f}. Text is measured as 6 units per
 * character (the game's font is about that for ASCII), line height 9.
 */
public final class FakePlatform implements Platform {
    public final FakeLink link = new FakeLink();
    public String version = "26.3";
    public boolean demoMode = true;
    public boolean inWorld;
    public final List<String> performed = new ArrayList<>();
    public int clicks;

    private int width;
    private int height;
    private Overlay home;
    /** The open screen: an overlay of ours, or null for "the game's own" (title or world). */
    private Overlay open;
    private final Deque<Overlay> beneath = new ArrayDeque<>();
    private boolean gameScreenOpen;
    private HudLayer hud;
    public MenuBackground menuBackground;
    public double scale = 2.0;
    private Runnable phoneKey;
    private final List<Runnable> tasks = new ArrayList<>();
    private int textures;

    public FakePlatform(int width, int height) {
        this.width = width;
        this.height = height;
    }

    // ------------------------------------------------------------ Platform

    @Override
    public String minecraftVersion() {
        return version;
    }

    @Override
    public String family() {
        return "fake";
    }

    @Override
    public void open(Overlay overlay) {
        if (overlay == null || overlay == open) {
            return;
        }
        if (open != null) {
            beneath.push(open);
        }
        open = overlay;
        overlay.opened(width, height);
    }

    @Override
    public void close(Overlay overlay) {
        if (overlay != null && overlay == open) {
            open = beneath.isEmpty() ? null : beneath.pop();
            overlay.closed();
        }
    }

    @Override
    public boolean isOpen(Overlay overlay) {
        return overlay != null && overlay == open;
    }

    @Override
    public boolean playing() {
        return inWorld && open == null && !gameScreenOpen;
    }

    @Override
    public boolean inWorld() {
        return inWorld;
    }

    @Override
    public void setHome(Overlay home) {
        this.home = home;
    }

    @Override
    public boolean available(HomeAction action) {
        switch (action) {
            case PLAY_DEMO:
                return demoMode;
            case MULTIPLAYER:
            case REALMS:
                return !demoMode;
            default:
                return true;
        }
    }

    @Override
    public void perform(HomeAction action) {
        performed.add(action.name());
    }

    @Override
    public String label(HomeAction action) {
        return HomeLabels.english(action);
    }

    @Override
    public void setMenuBackground(MenuBackground painter) {
        menuBackground = painter;
    }

    @Override
    public double guiScale() {
        return scale;
    }

    @Override
    public boolean demo() {
        return demoMode;
    }

    @Override
    public void setHud(HudLayer layer) {
        hud = layer;
    }

    @Override
    public void setPhoneKey(Runnable onPress) {
        phoneKey = onPress;
    }

    @Override
    public String phoneKeyName() {
        return "M";
    }

    @Override
    public Texture texture(Pixels pixels) {
        return pixels == null ? null : new FakeTexture(++textures, pixels);
    }

    @Override
    public void clickSound() {
        clicks++;
    }

    @Override
    public void runOnRenderThread(Runnable task) {
        synchronized (tasks) {
            tasks.add(task);
        }
    }

    // ------------------------------------------------------------ playing the game's part

    /** The game would show its title screen now: the home overlay if one is set. */
    public void showTitle() {
        inWorld = false;
        gameScreenOpen = home == null;
        if (home != null) {
            beneath.clear();
            open = null;
            open(home);
        }
    }

    /** The player is in a world, nothing open. */
    public void enterWorld() {
        inWorld = true;
        gameScreenOpen = false;
        if (open != null) {
            Overlay was = open;
            open = null;
            beneath.clear();
            was.closed();
        }
    }

    /** The open overlay, or null. */
    public Overlay openOverlay() {
        return open;
    }

    public void resize(int width, int height) {
        this.width = width;
        this.height = height;
        if (open != null) {
            open.opened(width, height);
        }
    }

    /**
     * One frame: runs queued tasks, delivers what the link received to its listener, draws the
     * HUD and the open overlay (mouse at mx, my) — or, with a game screen open and a menu
     * background painter set, that painter.
     */
    public List<String> frame(int mx, int my) {
        List<Runnable> now;
        synchronized (tasks) {
            now = new ArrayList<>(tasks);
            tasks.clear();
        }
        for (Runnable task : now) {
            task.run();
        }
        link.deliver();
        Recorder canvas = new Recorder(width, height);
        if (gameScreenOpen && open == null && menuBackground != null) {
            menuBackground.paint(canvas, inWorld);
        }
        if (hud != null && inWorld) {
            hud.render(canvas, 0.5f, open != null || gameScreenOpen);
        }
        if (open != null) {
            open.render(canvas, mx, my, 0.5f);
        }
        return canvas.drawn;
    }

    public List<String> frame() {
        return frame(-1, -1);
    }

    public void tick() {
        if (open != null) {
            open.tick();
        }
    }

    /** A key press as the adapter delivers it, with the phone key and Escape rules. */
    public boolean press(int key, int modifiers) {
        if (open == null) {
            if (key == Keys.UNKNOWN) {
                return false;
            }
            if (key == 'M' && playing() && phoneKey != null) {
                phoneKey.run();
                return true;
            }
            return false;
        }
        Overlay overlay = open;
        if (key == Keys.ESCAPE && overlay.closesOnEscape()) {
            close(overlay);
            return true;
        }
        if (overlay.keyPressed(key, modifiers)) {
            return true;
        }
        if (key == 'M' && phoneKey != null) {
            phoneKey.run();
            return true;
        }
        return false;
    }

    public boolean press(int key) {
        return press(key, 0);
    }

    /** Types {@code text}, one code point at a time. */
    public void type(String text) {
        if (open == null) {
            return;
        }
        text.codePoints().forEach(c -> open.charTyped(c, 0));
    }

    public boolean click(double x, double y, int button) {
        if (open == null) {
            return false;
        }
        boolean used = open.mousePressed(x, y, button, 0);
        if (open != null) {
            open.mouseReleased(x, y, button, 0);
        }
        return used;
    }

    public boolean click(double x, double y) {
        return click(x, y, Keys.MOUSE_LEFT);
    }

    public boolean scroll(double x, double y, double vertical) {
        return open != null && open.mouseScrolled(x, y, 0.0, vertical);
    }

    // ------------------------------------------------------------ drawing

    /** A canvas that writes down what is drawn. */
    public static final class Recorder extends Canvas {
        public final List<String> drawn = new ArrayList<>();
        private final int width;
        private final int height;
        private int clips;
        private int pushes;

        public Recorder(int width, int height) {
            this.width = width;
            this.height = height;
        }

        @Override
        public int width() {
            return width;
        }

        @Override
        public int height() {
            return height;
        }

        @Override
        public void fill(int x0, int y0, int x1, int y1, int argb) {
            drawn.add(String.format(Locale.ROOT, "fill %d,%d,%d,%d %08x", x0, y0, x1, y1, argb));
        }

        @Override
        public void text(String text, int x, int y, int argb, boolean shadow) {
            drawn.add(String.format(Locale.ROOT, "text \"%s\" %d,%d %08x%s", printable(text), x, y, argb,
                    shadow ? " shadow" : ""));
        }

        @Override
        public int textWidth(String text) {
            return text == null ? 0 : text.codePointCount(0, text.length()) * 6;
        }

        @Override
        public int lineHeight() {
            return 9;
        }

        @Override
        public void image(Texture texture, int x, int y, int w, int h) {
            String id = texture instanceof FakeTexture ? "#" + ((FakeTexture) texture).id : "?";
            drawn.add(String.format(Locale.ROOT, "image %s %d,%d %dx%d", id, x, y, w, h));
        }

        @Override
        public void clip(int x0, int y0, int x1, int y1) {
            clips++;
            drawn.add(String.format(Locale.ROOT, "clip %d,%d,%d,%d", x0, y0, x1, y1));
        }

        @Override
        public void unclip() {
            if (--clips < 0) {
                throw new IllegalStateException("unclip without clip");
            }
            drawn.add("unclip");
        }

        @Override
        public void push() {
            pushes++;
            drawn.add("push");
        }

        @Override
        public void pop() {
            if (--pushes < 0) {
                throw new IllegalStateException("pop without push");
            }
            drawn.add("pop");
        }

        @Override
        public void translate(float dx, float dy) {
            drawn.add(String.format(Locale.ROOT, "translate %.1f,%.1f", dx, dy));
        }

        @Override
        public void scale(float factor) {
            drawn.add(String.format(Locale.ROOT, "scale %.3f", factor));
        }
    }

    /** A texture that remembers its pixels. */
    public static final class FakeTexture implements Texture {
        public final int id;
        public Pixels pixels;
        public boolean closed;
        public int replaced;

        FakeTexture(int id, Pixels pixels) {
            this.id = id;
            this.pixels = pixels;
        }

        @Override
        public int width() {
            return pixels.width();
        }

        @Override
        public int height() {
            return pixels.height();
        }

        @Override
        public void replace(Pixels pixels) {
            this.pixels = pixels;
            replaced++;
        }

        @Override
        public void close() {
            closed = true;
        }
    }

    // ------------------------------------------------------------ the launcher's part

    /**
     * A stand-in for the launcher link: records commands, lets the test send state. What the
     * test sends reaches a listener on the next {@link FakePlatform#frame}, as on the render
     * thread.
     */
    public static final class FakeLink implements Link {
        public boolean up = true;
        public final List<String> sent = new ArrayList<>();
        private final Map<String, State> states = new HashMap<>();
        private final Map<String, Picture> pictures = new HashMap<>();
        private final Deque<Notice> notices = new ArrayDeque<>();
        private final List<Runnable> pending = new ArrayList<>();
        private Listener listener;
        private long changes;
        private long serial;

        @Override
        public synchronized void setListener(Listener listener) {
            this.listener = listener;
            pending.clear();
            if (listener != null) {
                listener.link(up);
                for (State state : states.values()) {
                    listener.state(state.app, state.raw);
                }
            }
        }

        /** Hands what arrived since the last frame to the listener. */
        synchronized void deliver() {
            List<Runnable> now = new ArrayList<>(pending);
            pending.clear();
            for (Runnable delivery : now) {
                delivery.run();
            }
        }

        private void later(java.util.function.Consumer<Listener> delivery) {
            Listener target = listener;
            if (target != null) {
                pending.add(() -> delivery.accept(target));
            }
        }

        /** The connection goes up or down. */
        public synchronized void connected(boolean connected) {
            up = connected;
            later(l -> l.link(connected));
        }

        @Override
        public boolean connected() {
            return up;
        }

        @Override
        public synchronized boolean send(String command) {
            if (!up || !Commands.valid(command)) {
                return false;
            }
            sent.add(command);
            return true;
        }

        @Override
        public synchronized State state(String app) {
            return states.get(app);
        }

        @Override
        public synchronized Picture picture(String app, int slot) {
            return pictures.get(app + "/" + slot);
        }

        @Override
        public synchronized Notice nextNotice() {
            return notices.poll();
        }

        @Override
        public synchronized long changes() {
            return changes;
        }

        /** The launcher sends {@code app}'s state (a JSON object). */
        public synchronized void state(String app, String json) throws Json.Malformed {
            states.put(app, new State(app, ++serial, json, Json.object(json)));
            changes++;
            later(l -> l.state(app, json));
        }

        public synchronized void picture(String app, int slot, Pixels pixels) {
            pictures.put(app + "/" + slot, new Picture(app, slot, ++serial, pixels));
            changes++;
            later(l -> l.frame(app, slot, pixels));
        }

        public synchronized void notice(String app, String text) {
            if (notices.size() == 32) {
                notices.poll();
            }
            notices.add(new Notice(app, text, System.currentTimeMillis()));
            changes++;
            later(l -> l.notice(app, text));
        }
    }
}
