import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Random;

import bastion.companion.home.HomeScreen;
import bastion.companion.phone.Phone;
import bastion.companion.platform.Canvas;
import bastion.companion.platform.Commands;
import bastion.companion.platform.HomeAction;
import bastion.companion.platform.HomeLabels;
import bastion.companion.platform.HudLayer;
import bastion.companion.platform.Keys;
import bastion.companion.platform.Link;
import bastion.companion.platform.MenuBackground;
import bastion.companion.platform.Overlay;
import bastion.companion.platform.Pixels;
import bastion.companion.platform.Platform;
import bastion.companion.platform.Texture;
import bastion.companion.theme.DarkMenus;
import bastion.companion.ui.Clock;
import bastion.companion.ui.Hits;
import bastion.companion.ui.Rect;

/**
 * The phone, the home screen and the dark menus against a stand-in game (tests/phone_ui.rs runs
 * this; no Minecraft). It plays the game's part — the platform: screens, keys, the mouse, a
 * clock it moves itself — and the launcher's: app states (well formed, hostile and mutated),
 * pictures, notices; and it records:
 *
 * <ul>
 * <li>every command line the phone sends ({@code cmd <line>}; the Rust test parses each one
 * with the launcher's own parser and checks it against the apps' vocabularies);
 * <li>every place where something is drawn outside where it belongs ({@code violation …}):
 * outside its clip (text cut instead of shortened), outside the phone, outside the screen,
 * text drawn over other text, rectangles turned inside out — on many window sizes;
 * <li>the outcome of scripted checks ({@code ok …} / {@code fail …}).
 * </ul>
 */
public final class PhoneUiCheck {
    // ------------------------------------------------------------------ results

    static final List<String> violations = new ArrayList<>();
    static int violationCount;
    static int failures;
    static int checks;
    static int frames;
    static int drawOps;

    static void violation(String where, String what) {
        violationCount++;
        if (violations.size() < 400) {
            violations.add(where + ": " + what);
        }
    }

    static void check(boolean condition, String what) {
        checks++;
        if (condition) {
            System.out.println("ok " + what);
        } else {
            failures++;
            System.out.println("fail " + what);
        }
    }

    // ------------------------------------------------------------------ the clock

    static long now = 5_000_000_000_000L;

    static void advance(long millis) {
        now += millis * 1_000_000L;
    }

    // ------------------------------------------------------------------ the font

    /** Advance widths close to the game's font: most ASCII 6, narrow ones less, wide scripts 9. */
    static int glyph(int c) {
        if (c < 0x80) {
            switch (c) {
                case 'i': case '!': case '.': case ',': case ':': case ';': case '|': case '\'':
                    return 2;
                case 'l': case '`':
                    return 3;
                case ' ': case 'I': case 't': case '(': case ')': case '[': case ']': case '{': case '}': case '*':
                case '"':
                    return 4;
                case 'f': case 'k': case '<': case '>':
                    return 5;
                case '@': case '~':
                    return 7;
                default:
                    return 6;
            }
        }
        return c >= 0x2E80 ? 9 : 6;
    }

    static int textWidth(String s) {
        int w = 0;
        for (int i = 0; i < s.length();) {
            int c = s.codePointAt(i);
            w += glyph(c);
            i += Character.charCount(c);
        }
        return w;
    }

    // ------------------------------------------------------------------ the checking canvas

    /** Draws nothing; checks where everything would be drawn. */
    static class Probe extends Canvas {
        final int w;
        final int h;
        final String where;
        /** Where everything drawn must stay (the phone's body, the screen). */
        Rect region;
        /** Where text must lie entirely, clipped or not (the phone's screen). */
        Rect textRegion;
        private float tx;
        private float ty;
        private float s = 1.0f;
        private final Deque<float[]> stack = new ArrayDeque<>();
        private final Deque<Rect> clips = new ArrayDeque<>();
        private final List<Rect> texts = new ArrayList<>();
        final List<String> textsDrawn = new ArrayList<>();
        int images;

        Probe(int w, int h, String where, Rect region) {
            this.w = w;
            this.h = h;
            this.where = where;
            this.region = region;
        }

        Rect screen() {
            return new Rect(0, 0, w, h);
        }

        private Rect map(float x0, float y0, float x1, float y1) {
            int ax = (int) Math.floor(tx + x0 * s);
            int ay = (int) Math.floor(ty + y0 * s);
            int bx = (int) Math.ceil(tx + x1 * s);
            int by = (int) Math.ceil(ty + y1 * s);
            return new Rect(ax, ay, bx - ax, by - ay);
        }

        private Rect clipNow() {
            Rect c = screen();
            for (Rect r : clips) {
                c = c.intersect(r);
            }
            return c;
        }

        private void checkOp(String kind, Rect r, String text) {
            drawOps++;
            if (r.w() < 0 || r.h() < 0) {
                violation(where, kind + " inside out " + r);
                return;
            }
            if (r.empty()) {
                return;
            }
            Rect clip = clipNow();
            if (clips.isEmpty() && !r.within(screen())) {
                violation(where, kind + " off the screen " + r + (text != null ? " \"" + text + "\"" : ""));
            }
            Rect visible = r.intersect(clip);
            if (visible.empty()) {
                return;
            }
            if (region != null && !visible.within(region)) {
                violation(where, kind + " outside its panel " + region + ": " + visible
                        + (text != null ? " \"" + text + "\"" : ""));
            }
            if (text != null) {
                if (r.x() < clip.x() || r.right() > clip.right()) {
                    violation(where, "text cut by a clip instead of shortened: \"" + text + "\" " + r + " clip " + clip);
                }
                if (region != null && (r.x() < region.x() || r.right() > region.right())) {
                    violation(where, "text runs out of its panel: \"" + text + "\" " + r);
                }
                if (textRegion != null && !r.within(textRegion)) {
                    violation(where, "text runs out of the phone's screen: \"" + text + "\" " + r + " screen " + textRegion);
                }
                for (Rect other : texts) {
                    if (!other.intersect(visible).empty()) {
                        violation(where, "text over text: \"" + text + "\" " + visible + " meets " + other);
                        break;
                    }
                }
                texts.add(visible);
                textsDrawn.add(text);
            }
        }

        @Override
        public int width() {
            return w;
        }

        @Override
        public int height() {
            return h;
        }

        boolean coveredScreen;

        @Override
        public void fill(int x0, int y0, int x1, int y1, int argb) {
            Rect r = map(x0, y0, x1, y1);
            if (r.equals(screen()) && (argb >>> 24) == 0xFF) {
                coveredScreen = true;
            }
            if (x1 < x0 || y1 < y0) {
                violation(where, "fill inside out " + x0 + "," + y0 + "," + x1 + "," + y1);
                return;
            }
            checkOp("fill", r, null);
            if ((argb >>> 24) == 0xFF) {
                // An opaque fill hides what is under it: text it covers no longer counts.
                Rect cover = r.intersect(clipNow());
                List<Rect> kept = new ArrayList<>();
                for (Rect t : texts) {
                    kept.add(subtract(t, cover));
                }
                texts.clear();
                for (Rect t : kept) {
                    if (!t.empty()) {
                        texts.add(t);
                    }
                }
            }
        }

        /** What is left of t outside cover, where that is a rectangle (else t). */
        private static Rect subtract(Rect t, Rect cover) {
            if (cover.empty() || t.intersect(cover).empty()) {
                return t;
            }
            if (t.within(cover)) {
                return Rect.EMPTY;
            }
            boolean fullWidth = cover.x() <= t.x() && cover.right() >= t.right();
            boolean fullHeight = cover.y() <= t.y() && cover.bottom() >= t.bottom();
            if (fullWidth) {
                if (cover.y() <= t.y()) {
                    return new Rect(t.x(), cover.bottom(), t.w(), t.bottom() - cover.bottom());
                }
                if (cover.bottom() >= t.bottom()) {
                    return new Rect(t.x(), t.y(), t.w(), cover.y() - t.y());
                }
            }
            if (fullHeight) {
                if (cover.x() <= t.x()) {
                    return new Rect(cover.right(), t.y(), t.right() - cover.right(), t.h());
                }
                if (cover.right() >= t.right()) {
                    return new Rect(t.x(), t.y(), cover.x() - t.x(), t.h());
                }
            }
            return t;
        }

        @Override
        public void text(String text, int x, int y, int argb, boolean shadow) {
            if (text == null) {
                violation(where, "null text");
                return;
            }
            for (int i = 0; i < text.length(); i++) {
                char c = text.charAt(i);
                if (c < 0x20 || c == 0x7f || c == 0xA7 || (c >= 0x202A && c <= 0x202E) || (c >= 0x2066 && c <= 0x2069)) {
                    violation(where, "drew a control, direction or formatting character: " + (int) c);
                }
                if (Character.isSurrogate(c) && !(Character.isHighSurrogate(c) && i + 1 < text.length()
                        && Character.isLowSurrogate(text.charAt(i + 1)))
                        && !(Character.isLowSurrogate(c) && i > 0 && Character.isHighSurrogate(text.charAt(i - 1)))) {
                    violation(where, "drew a lone surrogate");
                }
            }
            checkOp("text", map(x, y, x + textWidth(text), y + 9), text);
        }

        @Override
        public int textWidth(String text) {
            return text == null ? 0 : PhoneUiCheck.textWidth(text);
        }

        @Override
        public int lineHeight() {
            return 9;
        }

        @Override
        public void image(Texture texture, int x, int y, int w, int h) {
            images++;
            if (w < 0 || h < 0) {
                violation(where, "image inside out");
            }
            checkOp("image", map(x, y, x + w, y + h), null);
        }

        @Override
        public void clip(int x0, int y0, int x1, int y1) {
            Rect r = new Rect(x0, y0, x1 - x0, y1 - y0);
            if (r.w() < 0 || r.h() < 0) {
                violation(where, "clip inside out " + r);
            }
            if (!r.within(screen()) && !r.empty()) {
                violation(where, "clip off the screen " + r);
            }
            clips.push(r);
        }

        @Override
        public void unclip() {
            if (clips.poll() == null) {
                violation(where, "unclip without clip");
            }
        }

        @Override
        public void push() {
            stack.push(new float[] {tx, ty, s});
        }

        @Override
        public void pop() {
            float[] t = stack.poll();
            if (t == null) {
                violation(where, "pop without push");
                return;
            }
            tx = t[0];
            ty = t[1];
            s = t[2];
        }

        @Override
        public void translate(float dx, float dy) {
            tx += dx * s;
            ty += dy * s;
        }

        @Override
        public void scale(float factor) {
            s *= factor;
        }

        void finish() {
            if (!clips.isEmpty()) {
                violation(where, clips.size() + " clip(s) left open");
            }
            if (!stack.isEmpty()) {
                violation(where, stack.size() + " push(es) left open");
            }
        }
    }

    // ------------------------------------------------------------------ the game's part

    static final class Tex implements Texture {
        int w;
        int h;
        boolean closed;
        Pixels pixels;

        /** Set by the preview renderer, which draws the pictures; the check only needs their sizes. */
        static boolean keepPixels;

        Tex(Pixels p) {
            w = p.width();
            h = p.height();
            pixels = keepPixels ? p : null;
        }

        @Override
        public int width() {
            return w;
        }

        @Override
        public int height() {
            return h;
        }

        @Override
        public void replace(Pixels pixels) {
            w = pixels.width();
            h = pixels.height();
            this.pixels = keepPixels ? pixels : null;
        }

        @Override
        public void close() {
            if (!closed) {
                closed = true;
                game.texturesClosed++;
            }
        }
    }

    static final class Game implements Platform {
        static final int PHONE_KEY = 'R';
        int width;
        int height;
        double scale = 2.0;
        boolean demo = true;
        boolean modMenu;
        boolean inWorld = true;
        Overlay open;
        final Deque<Overlay> beneath = new ArrayDeque<>();
        Overlay home;
        HudLayer hud;
        MenuBackground menu;
        Runnable phoneKey;
        final List<Runnable> tasks = new ArrayList<>();
        final List<String> performed = new ArrayList<>();
        int clicks;
        int textures;
        int texturesClosed;

        Game(int width, int height) {
            this.width = width;
            this.height = height;
        }

        @Override
        public String minecraftVersion() {
            return "26.3";
        }

        @Override
        public String family() {
            return "check";
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
                open = beneath.poll();
                overlay.closed();
            }
        }

        @Override
        public boolean isOpen(Overlay overlay) {
            return overlay != null && overlay == open;
        }

        @Override
        public boolean playing() {
            return inWorld && open == null;
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
                    return demo;
                case SINGLEPLAYER:
                case MULTIPLAYER:
                case REALMS:
                    return !demo;
                case MODS:
                    return modMenu;
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
        public boolean demo() {
            return demo;
        }

        @Override
        public void setHud(HudLayer layer) {
            hud = layer;
        }

        @Override
        public void setMenuBackground(MenuBackground painter) {
            menu = painter;
        }

        @Override
        public void setPhoneKey(Runnable onPress) {
            phoneKey = onPress;
        }

        @Override
        public String phoneKeyName() {
            return "R";
        }

        @Override
        public Texture texture(Pixels pixels) {
            textures++;
            return new Tex(pixels);
        }

        @Override
        public double guiScale() {
            return scale;
        }

        @Override
        public void clickSound() {
            clicks++;
        }

        @Override
        public void runOnRenderThread(Runnable task) {
            tasks.add(task);
        }

        void runTasks() {
            List<Runnable> now = new ArrayList<>(tasks);
            tasks.clear();
            for (Runnable task : now) {
                task.run();
            }
        }

        /** A key as the adapter delivers it: Escape and the phone key by the platform's rules. */
        void press(int key, int modifiers) {
            if (open == null) {
                if (key == PHONE_KEY && playing() && phoneKey != null) {
                    phoneKey.run();
                }
                return;
            }
            Overlay overlay = open;
            if (key == Keys.ESCAPE && overlay.closesOnEscape()) {
                close(overlay);
                return;
            }
            if (!overlay.keyPressed(key, modifiers) && key == PHONE_KEY && phoneKey != null) {
                phoneKey.run();
            }
        }

        void resize(int w, int h) {
            width = w;
            height = h;
            if (open != null) {
                open.opened(w, h);
            }
        }
    }

    /** The launcher's end of the link: what the phone sent; states, pictures and notices to deliver. */
    static final class Line implements Link {
        boolean up = true;
        final List<String> sent = new ArrayList<>();
        Listener listener;
        final List<Runnable> pending = new ArrayList<>();
        final Map<String, String> states = new LinkedHashMap<>();
        int refused;

        @Override
        public boolean connected() {
            return up;
        }

        @Override
        public boolean send(String command) {
            if (!Commands.valid(command)) {
                refused++;
                System.out.println("refused " + command);
                return false;
            }
            if (!up) {
                return false;
            }
            sent.add(command);
            System.out.println("cmd " + command);
            return true;
        }

        @Override
        public State state(String app) {
            return null;
        }

        @Override
        public Picture picture(String app, int slot) {
            return null;
        }

        @Override
        public Notice nextNotice() {
            return null;
        }

        @Override
        public long changes() {
            return 0;
        }

        @Override
        public void setListener(Listener l) {
            listener = l;
            if (l != null) {
                l.link(up);
                for (Map.Entry<String, String> e : states.entrySet()) {
                    l.state(e.getKey(), e.getValue());
                }
            }
        }

        void state(String app, String json) {
            states.put(app, json);
            pending.add(() -> listener.state(app, json));
        }

        void picture(String app, int slot, Pixels p) {
            pending.add(() -> listener.frame(app, slot, p));
        }

        void notice(String app, String text) {
            pending.add(() -> listener.notice(app, text));
        }

        void link(boolean connected) {
            up = connected;
            pending.add(() -> listener.link(connected));
        }

        void deliver() {
            List<Runnable> now = new ArrayList<>(pending);
            pending.clear();
            for (Runnable r : now) {
                r.run();
            }
        }

        /** The commands sent since index {@code from}. */
        List<String> since(int from) {
            return new ArrayList<>(sent.subList(Math.min(from, sent.size()), sent.size()));
        }
    }

    // ------------------------------------------------------------------ the session

    static Game game;
    static Line line;
    static Phone phone;
    static Probe last;

    static void start(int w, int h) {
        game = new Game(w, h);
        line = new Line();
        phone = new Phone(game, line);
        line.setListener(phone.store());
        game.setPhoneKey(phone::toggle);
        game.setHud(phone);
    }

    /** One frame of whatever is open, checked; {@code where} names it in violations. */
    static Probe frame(String where) {
        frames++;
        game.runTasks();
        line.deliver();
        Probe p = new Probe(game.width, game.height, where, null);
        if (game.open == phone) {
            boolean settledBefore = phone.settled();
            p.region = settledBefore ? Phone.body(game.width, game.height, phone.landscape(), game.inWorld) : p.screen();
            p.textRegion = settledBefore ? p.region.inset(4) : null;
            phone.render(p, -1, -1, 0.5f);
            if (settledBefore && phone.settled() && !phone.lastBody().equals(p.region)) {
                violation(where, "phone drawn at " + phone.lastBody() + ", expected " + p.region);
            }
        } else if (game.open != null) {
            p.region = p.screen();
            game.open.render(p, -1, -1, 0.5f);
        } else if (game.hud != null && game.inWorld) {
            p.region = p.screen();
            game.hud.render(p, 0.5f, false);
        }
        p.finish();
        last = p;
        return p;
    }

    static boolean landscapeNow() {
        return phone.landscape();
    }

    /** Frames until the phone has finished sliding (the clock moved by hand). */
    static void settle(String where) {
        for (int i = 0; i < 20; i++) {
            advance(40);
            frame(where);
            if (game.open != phone || phone.settled()) {
                break;
            }
        }
        frame(where);
    }

    static Hits.Hit hit(String id) {
        Hits hits = game.open == phone ? phone.hits() : game.open instanceof HomeScreen ? ((HomeScreen) game.open).hits() : null;
        if (hits == null) {
            return null;
        }
        for (Hits.Hit h : hits.shown()) {
            if (h.id.equals(id)) {
                return h;
            }
        }
        return null;
    }

    static boolean has(String id) {
        return hit(id) != null;
    }

    /** Clicks the middle of the place {@code id}; false when there is none. */
    static boolean click(String id) {
        Hits.Hit h = hit(id);
        if (h == null) {
            StringBuilder ids = new StringBuilder();
            Hits hits = game.open == phone ? phone.hits() : null;
            if (hits != null) {
                for (Hits.Hit x : hits.shown()) {
                    ids.append(' ').append(x.id);
                }
            }
            System.out.println("note no place " + id + " (" + game.width + "x" + game.height + ", app "
                    + (game.open == phone ? phone.currentId() : "-") + "; places:" + ids + ")");
            return false;
        }
        clickAt(h.rect.x() + h.rect.w() / 2.0, h.rect.y() + h.rect.h() / 2.0);
        return true;
    }

    static void clickAt(double x, double y) {
        Overlay o = game.open;
        if (o == null) {
            return;
        }
        o.mousePressed(x, y, Keys.MOUSE_LEFT, 0);
        if (game.open != null) {
            game.open.mouseReleased(x, y, Keys.MOUSE_LEFT, 0);
        }
    }

    /** Presses at a fraction of the width of {@code id}, drags, releases. */
    static boolean slide(String id, double from, double to) {
        Hits.Hit h = hit(id);
        if (h == null) {
            System.out.println("note no slider " + id);
            return false;
        }
        double y = h.rect.y() + h.rect.h() / 2.0;
        double x0 = h.rect.x() + h.rect.w() * from;
        double x1 = h.rect.x() + h.rect.w() * to;
        game.open.mousePressed(x0, y, Keys.MOUSE_LEFT, 0);
        for (int i = 1; i <= 4; i++) {
            advance(100);
            game.open.mouseDragged(x0 + (x1 - x0) * i / 4, y, Keys.MOUSE_LEFT, 0, 0);
        }
        game.open.mouseReleased(x1, y, Keys.MOUSE_LEFT, 0);
        return true;
    }

    static void key(int key) {
        game.press(key, 0);
    }

    static void key(int key, int mods) {
        game.press(key, mods);
    }

    static void type(String text) {
        if (game.open == null) {
            return;
        }
        text.codePoints().forEach(c -> {
            // A letter key goes down before its character, as with a real keyboard.
            int upper = Character.toUpperCase(c);
            if (upper >= 'A' && upper <= 'Z') {
                game.press(upper, 0);
            }
            if (game.open != null) {
                game.open.charTyped(c, 0);
            }
        });
    }

    static void tick() {
        if (game.open != null) {
            game.open.tick();
        }
    }

    static Pixels picture(int w, int h, int argb) {
        return Pixels.filled(w, h, argb);
    }

    // ------------------------------------------------------------------ states

    static final String PHONE_ALL = "{\"apps\":[\"spotify\",\"browser\",\"clips\",\"realms\",\"settings\"],\"theme\":\"dark\"}";

    static String spotify(boolean playing, String results, String devices) {
        return "{\"source\":\"mpris\",\"playing\":" + playing + ",\"track\":\"Harder, Better, Faster, Stronger\","
                + "\"artist\":\"Daft Punk\",\"album\":\"Discovery\",\"duration_ms\":224000,\"position_ms\":61000,"
                + "\"volume\":40,\"shuffle\":false,\"repeat\":\"off\",\"results\":" + results + ",\"devices\":" + devices
                + ",\"error\":null}";
    }

    static final String RESULTS = "[{\"uri\":\"spotify:track:0DiWol3AO6WpXZgp0goxAV\",\"title\":\"One More Time\","
            + "\"subtitle\":\"Daft Punk · Discovery\"},{\"uri\":\"spotify:album:2noRn2Aes5aoNVsU6iWThc\","
            + "\"title\":\"Discovery\",\"subtitle\":\"Album · Daft Punk\"},{\"uri\":\"spotify:artist:4tZwfgrHOc3mvqYlEYSvVi\","
            + "\"title\":\"Daft Punk\",\"subtitle\":\"Artist\"},{\"uri\":\"not a uri\",\"title\":\"Broken entry\","
            + "\"subtitle\":\"cannot be opened\"}]";
    static final String DEVICES = "[{\"id\":\"desk-1\",\"name\":\"This computer\",\"active\":true},"
            + "{\"id\":\"kitchen-2\",\"name\":\"Kitchen speaker\",\"active\":false},"
            + "{\"id\":\"has space\",\"name\":\"Bad id\",\"active\":false}]";

    static String browser(String open) {
        return "{\"sites\":[{\"name\":\"discord\",\"title\":\"Discord\"},{\"name\":\"wiki\",\"title\":\"Minecraft Wiki\"}],"
                + "\"open\":" + (open == null ? "null" : "\"" + open + "\"") + ",\"host\":\""
                + (open == null ? "" : open + ".com") + "\",\"loading\":false,\"title\":\""
                + (open == null ? "" : "Discord | Friends") + "\",\"error\":null,\"signed_in_hint\":null}";
    }

    static final String CLIPS = "{\"recording\":true,\"buffer_seconds\":60,\"clips\":["
            + "{\"id\":\"c1\",\"name\":\"Creeper ambush\",\"created\":1763100000,\"seconds\":30,\"bytes\":12345678},"
            + "{\"id\":\"c2\",\"name\":\"Nether portal\",\"created\":\"yesterday\",\"seconds\":15,\"bytes\":5432100},"
            + "{\"id\":\"c3\",\"name\":\"\",\"created\":null,\"seconds\":60,\"bytes\":99999999}],"
            + "\"folder\":\"/home/player/Videos/bastion-mc\",\"error\":null}";

    static final String REALMS_PROMPT = "{\"mode\":\"code\",\"prompt\":{\"world\":\"Our Realm\",\"address\":\"realm.example\","
            + "\"expires_in\":90,\"attempts_left\":3},\"last\":null}";
    static final String REALMS_NONE = "{\"mode\":\"totp\",\"prompt\":null,\"last\":\"approved\"}";
    static final String SETTINGS = "{\"sync\":true,\"last_sync\":1763100000,\"keys\":42,"
            + "\"instances\":[\"vanilla-latest\",\"fab\",\"fabvoice\"],\"error\":null}";

    static void allStates() {
        line.state("phone", PHONE_ALL);
        line.state("spotify", spotify(true, "[]", "[]"));
        line.state("browser", browser(null));
        line.state("clips", CLIPS);
        line.state("realms", REALMS_PROMPT);
        line.state("settings", SETTINGS);
    }

    // ------------------------------------------------------------------ scenario: using every app

    static void useEveryApp() {
        Clock.source = () -> now;
        start(480, 270);
        allStates();
        frame("closed");
        check(game.open == null, "the phone is away at first");
        key(Game.PHONE_KEY);
        check(game.open == phone, "the phone key pulls the phone out");
        settle("home");
        check(phone.settled(), "the phone settles after its slide");
        for (String app : new String[] {"spotify", "discord", "browser", "clips", "realms", "settings"}) {
            check(has("app:" + app), "the home screen shows " + app);
        }

        // Spotify.
        int mark = line.sent.size();
        click("app:spotify");
        frame("spotify now");
        check(line.since(mark).contains("spotify art"), "spotify asks for the album art it lacks");
        line.picture("spotify", 0, picture(64, 64, 0xFF336699));
        frame("spotify now art");
        check(last.images == 1, "the album art is drawn");
        mark = line.sent.size();
        click("sp-play");
        check(line.since(mark).contains("spotify pause"), "play/pause while playing sends pause");
        line.state("spotify", spotify(false, "[]", "[]"));
        frame("spotify paused");
        mark = line.sent.size();
        click("sp-play");
        check(line.since(mark).contains("spotify play"), "play/pause while paused sends play");
        line.state("spotify", spotify(true, "[]", "[]"));
        frame("spotify playing");
        mark = line.sent.size();
        click("sp-next");
        click("sp-previous");
        click("sp-shuffle");
        click("sp-repeat");
        List<String> got = line.since(mark);
        check(got.contains("spotify next") && got.contains("spotify previous"), "next and previous");
        check(got.contains("spotify shuffle on"), "shuffle on");
        check(got.contains("spotify repeat context"), "repeat cycles to context");
        mark = line.sent.size();
        slide("sp-seek", 0.1, 0.5);
        got = line.since(mark);
        check(got.size() == 1 && got.get(0).startsWith("spotify seek "), "seek is sent once, when let go: " + got);
        if (got.size() == 1) {
            long ms = Long.parseLong(got.get(0).substring("spotify seek ".length()));
            check(ms > 95_000 && ms < 130_000, "seek to the middle of 224 s: " + ms);
        }
        frame("spotify after seek");
        mark = line.sent.size();
        slide("sp-volume", 0.4, 0.8);
        got = line.since(mark);
        check(!got.isEmpty() && got.get(got.size() - 1).matches("spotify volume (7[0-9]|8[0-9])"),
                "volume follows the slider, last value sent on release: " + got);
        check(got.size() <= 5, "volume is rate-limited while dragging: " + got.size());
        frame("spotify after volume");
        // Search.
        click("sp-tab1");
        frame("spotify search");
        check("sp-query".equals(phone.hits().focus()), "the search field has the focus");
        mark = line.sent.size();
        type("Daft Punk");
        frame("spotify search typed");
        check(game.open == phone, "typing R into a field does not put the phone away");
        key(Keys.ENTER);
        got = line.since(mark);
        check(got.contains("spotify search -- Daft Punk"), "search sends the typed text: " + got);
        line.state("spotify", spotify(true, RESULTS, "[]"));
        frame("spotify results");
        key(Keys.ESCAPE);
        check(phone.hits().focus() == null && game.open == phone, "Escape first leaves the field");
        mark = line.sent.size();
        click("sp-result0");
        click("sp-queue0");
        click("sp-result3");
        got = line.since(mark);
        check(got.contains("spotify open spotify:track:0DiWol3AO6WpXZgp0goxAV"), "a result opens its URI");
        check(got.contains("spotify queue spotify:track:0DiWol3AO6WpXZgp0goxAV"), "a track can be queued");
        check(got.size() == 2, "an entry without a valid URI sends nothing: " + got);
        // Devices.
        mark = line.sent.size();
        click("sp-tab2");
        line.state("spotify", spotify(true, RESULTS, DEVICES));
        frame("spotify devices");
        click("sp-device1");
        click("sp-device2");
        got = line.since(mark);
        check(got.contains("spotify devices"), "the devices tab asks for devices");
        check(got.contains("spotify transfer kitchen-2"), "a device can be chosen");
        check(got.size() == 2, "a device id outside the grammar is not sent: " + got);
        key(Keys.ESCAPE);
        frame("spotify back to now");
        key(Keys.ESCAPE);
        frame("home again");
        check(phone.currentId() == null, "Escape goes back to the home screen");

        // Discord: the browser at the site named discord.
        mark = line.sent.size();
        click("app:discord");
        frame("discord opening");
        check(line.since(mark).contains("browser open discord"), "Discord opens the configured site");
        line.state("browser", browser("discord"));
        frame("discord page waiting");
        advance(400);
        frame("discord page waiting 2");
        got = line.since(mark);
        check(got.contains("browser frames on"), "the page's pictures are asked for while it shows");
        String size = null;
        for (String c : got) {
            if (c.startsWith("browser size ")) {
                size = c;
            }
        }
        check(size != null, "the viewport size is sent: " + size);
        line.picture("browser", 0, picture(360, 560, 0xFF202020));
        frame("discord page");
        Rect page = pageRect();
        check(page != null, "the page picture is drawn");
        if (page != null) {
            mark = line.sent.size();
            double cx = page.x() + page.w() * 0.25;
            double cy = page.y() + page.h() * 0.75;
            game.open.mousePressed(cx, cy, Keys.MOUSE_LEFT, 0);
            game.open.mouseReleased(cx, cy, Keys.MOUSE_LEFT, 0);
            got = line.since(mark);
            int ex = (int) Math.floor((cx - page.x()) * 360 / page.w());
            int ey = (int) Math.floor((cy - page.y()) * 560 / page.h());
            check(got.contains("browser mouse down " + ex + " " + ey + " left")
                    && got.contains("browser mouse up " + ex + " " + ey + " left"),
                    "a click lands on the page pixel under it (" + ex + "," + ey + "): " + got);
            mark = line.sent.size();
            type("hello, R");
            check(game.open == phone, "typing into the page never puts the phone away");
            advance(150);
            tick();
            key(Keys.ENTER);
            key('A', Keys.MOD_CONTROL);
            game.open.mouseScrolled(cx, cy, 0, 1.0);
            advance(150);
            tick();
            got = line.since(mark);
            check(got.contains("browser text -- hello, R"), "typing is sent as text: " + got);
            check(got.contains("browser key Enter") && got.contains("browser key ctrl-a"), "keys by name: " + got);
            check(got.contains("browser scroll 0 -100"), "the wheel scrolls the page: " + got);
            key(Keys.ESCAPE);
            check(game.open == phone && "discord".equals(phone.currentId()), "Escape takes the keyboard back from the page");
        }
        mark = line.sent.size();
        click("br-back");
        click("br-forward");
        click("br-reload");
        got = line.since(mark);
        check(got.equals(List.of("browser back", "browser forward", "browser reload")), "back, forward, reload: " + got);
        click("br-rotate");
        frame("discord landscape");
        advance(400);
        frame("discord landscape 2");
        check(landscapeNow(), "the phone turns sideways for reading");
        mark = line.sent.size();
        click("br-close");
        got = line.since(mark);
        check(got.contains("browser close"), "the page can be closed");
        line.state("browser", browser(null));
        frame("discord closed");
        key(Keys.ESCAPE);
        frame("home after discord");
        check(line.sent.contains("browser frames off"), "leaving the page stops its pictures");

        // The browser's site list.
        mark = line.sent.size();
        click("app:browser");
        frame("browser sites");
        click("br-site1");
        check(line.since(mark).contains("browser open wiki"), "a site from the list opens");
        key(Keys.ESCAPE);
        frame("home after browser");

        // Clips.
        mark = line.sent.size();
        click("app:clips");
        for (int i = 0; i < 6; i++) {
            advance(300);
            frame("clips list");
        }
        got = line.since(mark);
        check(got.contains("clips list"), "clips asks for the list");
        check(got.contains("clips thumb c1") && got.contains("clips thumb c2"), "thumbnails are asked for: " + got);
        line.picture("clips", 0, picture(160, 90, 0xFF445566));
        line.picture("clips", 1, picture(160, 90, 0xFF665544));
        frame("clips thumbs");
        check(last.images >= 2, "thumbnails are drawn");
        mark = line.sent.size();
        click("cl-save");
        click("cl-length");
        frame("clips length");
        click("cl-save");
        got = line.since(mark);
        check(got.contains("clips save 30") && got.contains("clips save 60"), "save the last 30 s, then 60 s: " + got);
        click("cl-clip0");
        frame("clip detail");
        click("cl-name");
        for (int i = 0; i < 20; i++) {
            key(Keys.BACKSPACE);
        }
        mark = line.sent.size();
        type("Best moment");
        key(Keys.ENTER);
        got = line.since(mark);
        check(got.contains("clips rename c1 -- Best moment"), "a clip can be renamed: " + got);
        mark = line.sent.size();
        click("cl-delete");
        frame("clip delete confirm");
        check(line.since(mark).isEmpty(), "delete asks first");
        click("cl-delete");
        check(line.since(mark).contains("clips delete c1"), "delete, confirmed");
        frame("clips after delete");
        key(Keys.ESCAPE);
        frame("home after clips");

        // Realms.
        mark = line.sent.size();
        click("app:realms");
        frame("realms prompt");
        frame("realms prompt 2");
        check("rl-code".equals(phone.hits().focus()), "the code field has the focus when a code is asked for");
        type("12a34r56");
        key(Keys.ENTER);
        got = line.since(mark);
        check(got.contains("realms code -- 123456"), "only digits are typed, and the code is sent: " + got);
        click("rl-cancel");
        check(line.since(mark).contains("realms cancel"), "a sign-in can be cancelled");
        line.state("realms", REALMS_NONE);
        frame("realms none");
        key(Keys.ESCAPE);
        frame("home after realms");

        // Settings.
        mark = line.sent.size();
        click("app:settings");
        frame("settings");
        click("st-sync");
        got = line.since(mark);
        check(got.contains("settings status") && got.contains("settings sync-now"), "settings: status, sync now: " + got);
        key(Keys.ESCAPE);
        frame("home after settings");

        // Keyboard only: arrows and Enter open an app.
        key(Keys.DOWN);
        key(Keys.ENTER);
        frame("keyboard opened");
        check(phone.currentId() != null, "arrow keys and Enter open an app: " + phone.currentId());
        key(Keys.ESCAPE);
        frame("home after keyboard");

        // In a world, a phone over the hotbar's right end stands on the hotbar row.
        for (int[] wh : new int[][] {{427, 240}, {480, 270}, {640, 360}, {960, 540}, {320, 240}}) {
            Rect inWorld = Phone.body(wh[0], wh[1], false, true);
            Rect out = Phone.body(wh[0], wh[1], false, false);
            boolean clear = inWorld.x() >= wh[0] / 2 + 91 || inWorld.bottom() <= wh[1] - 22;
            boolean tooSmall = wh[1] - 16 - 26 < 180;
            check(clear || tooSmall, "in a world the phone leaves the hotbar visible at " + wh[0] + "x" + wh[1]
                    + ": " + inWorld + " (out of a world " + out + ")");
            check(inWorld.within(new Rect(0, 0, wh[0], wh[1])) && inWorld.h() >= Math.min(180, out.h()),
                    "and stays on the screen and tall enough at " + wh[0] + "x" + wh[1]);
        }

        // Out of a world (over the home screen): a dark backdrop behind the phone, the phone
        // itself still inside its body.
        game.inWorld = false;
        Probe out = new Probe(game.width, game.height, "out of world", null);
        phone.render(out, -1, -1, 0.5f);
        out.finish();
        check(out.coveredScreen, "out of a world the phone has a backdrop over the whole screen");
        game.inWorld = true;

        // Put away with the phone key; a notice then peeks in.
        key(Game.PHONE_KEY);
        settle("closing");
        game.runTasks();
        check(game.open == null, "the phone key puts the phone away");
        line.state("realms", "{\"mode\":\"code\",\"prompt\":null,\"last\":null}");
        line.notice("clips", "Saved clip: Creeper ambush (30 s)");
        frame("toast");
        check(last.textsDrawn.stream().anyMatch(t -> t.contains("Saved clip")), "a notice peeks in while the phone is away");
        advance(6000);
        frame("toast gone");
        check(last.textsDrawn.isEmpty(), "and goes again after a few seconds");
        line.state("realms", REALMS_PROMPT);
        frame("realms toast");
        check(last.textsDrawn.stream().anyMatch(t -> t.contains("code")), "a waiting Realms sign-in peeks in");
        key(Game.PHONE_KEY);
        settle("out again");
        check(phone.currentId() == null && game.open == phone, "the phone comes back where it was put away");
        int clicksBefore = game.clicks;
        click("app:settings");
        check(game.clicks > clicksBefore, "buttons click");
        System.out.println("count commands=" + line.sent.size());
    }

    /** Where the browser page was drawn: the one image in the last frame's page area. */
    static Rect pageRect() {
        // The page picture is drawn into the area below the toolbar; recompute from the hits:
        // the page has no hit, so find it as the largest rectangle the phone drew an image in.
        Probe p = new Probe(game.width, game.height, "page", null) {
            Rect best;

            @Override
            public void image(Texture texture, int x, int y, int w, int h) {
                if (best == null || w * h > best.w() * best.h()) {
                    best = new Rect(x, y, w, h);
                }
                found = best;
            }
        };
        phone.render(p, -1, -1, 0.5f);
        return found;
    }

    static Rect found;

    // ------------------------------------------------------------------ scenario: every screen, every size

    static final int[][] SIZES = {
        {320, 240}, {427, 240}, {480, 270}, {640, 360}, {854, 480}, {960, 540}, {1920, 1080}, {1280, 720},
        {320, 180}, {240, 320}, {200, 150}, {360, 640}, {2560, 1080},
    };

    static void everyScreen(String tag, Runnable states) {
        for (int[] size : SIZES) {
            start(size[0], size[1]);
            states.run();
            String at = tag + " " + size[0] + "x" + size[1];
            key(Game.PHONE_KEY);
            settle(at + " home");
            String[] apps = {"spotify", "discord", "browser", "clips", "realms", "settings"};
            for (String app : apps) {
                phone.openApp(app);
                if (!app.equals(phone.currentId())) {
                    continue;
                }
                frame(at + " " + app);
                frame(at + " " + app);
                if ("spotify".equals(app)) {
                    line.picture("spotify", 0, picture(300, 300, 0xFF884422));
                    frame(at + " spotify art");
                    for (int t = 0; t < 3; t++) {
                        click("sp-tab" + t);
                        frame(at + " spotify tab " + t);
                    }
                }
                if ("discord".equals(app) || "browser".equals(app)) {
                    line.state("browser", browser("discord"));
                    line.picture("browser", 0, picture(360, 640, 0xFF101010));
                    frame(at + " " + app + " page");
                    click("br-rotate");
                    advance(50);
                    frame(at + " " + app + " landscape");
                    line.picture("browser", 0, picture(1024, 576, 0xFF101010));
                    frame(at + " " + app + " landscape wide");
                    click("br-rotate");
                    frame(at + " " + app + " portrait");
                    line.state("browser", browser(null));
                    frame(at + " " + app + " closed");
                }
                if ("clips".equals(app)) {
                    line.picture("clips", 0, picture(160, 90, 0xFF223344));
                    frame(at + " clips thumb");
                    click("cl-clip0");
                    frame(at + " clip detail");
                    click("cl-delete");
                    frame(at + " clip detail confirm");
                    key(Keys.ESCAPE);
                }
                key(Keys.ESCAPE);
                frame(at + " home after " + app);
            }
            // Notices on the home screen and as a banner.
            line.notice("spotify", "Now playing: a very long title that would never fit on one line of a phone screen");
            line.notice("realms", "Sign-in approved");
            frame(at + " home with notices");
            phone.openApp("settings");
            line.notice("clips", "Saved clip");
            frame(at + " banner");
            frame(at + " banner");
            if (size[1] >= 240 && "settings".equals(phone.currentId())) {
                check(last.textsDrawn.contains("Saved clip"), "the banner shows the notice's text at " + at);
            }
            // Away: the HUD toast.
            key(Game.PHONE_KEY);
            settle(at + " away");
            game.runTasks();
            line.notice("clips", "Saved clip: something long enough to need shortening on a small screen");
            frame(at + " toast");
            // The home screen and the dark menus.
            HomeScreen home = new HomeScreen(game);
            for (boolean demo : new boolean[] {true, false}) {
                game.demo = demo;
                game.modMenu = !demo;
                game.open = null;
                game.open(home);
                advance(500);
                frame(at + " home screen demo=" + demo);
            }
            game.open = null;
            for (boolean inWorld : new boolean[] {false, true}) {
                Probe p = new Probe(size[0], size[1], at + " dark menus inWorld=" + inWorld, new Rect(0, 0, size[0], size[1]));
                DarkMenus.paintBackground(p, inWorld);
                p.finish();
            }
        }
    }

    static final String LONG = "W".repeat(150);

    static void richStates() {
        allStates();
        line.state("spotify", spotify(true, RESULTS, DEVICES));
    }

    static void longStates() {
        StringBuilder results = new StringBuilder("[");
        for (int i = 0; i < 64; i++) {
            results.append(i == 0 ? "" : ",").append("{\"uri\":\"spotify:track:abc").append(i).append("\",\"title\":\"")
                    .append(LONG).append("\",\"subtitle\":\"").append(LONG).append("\"}");
        }
        results.append("]");
        line.state("phone", PHONE_ALL);
        line.state("spotify", "{\"source\":\"web\",\"playing\":false,\"track\":\"" + LONG + "\",\"artist\":\"" + LONG
                + "\",\"album\":\"" + LONG + "\",\"duration_ms\":86400000,\"position_ms\":86399000,\"volume\":100,"
                + "\"shuffle\":true,\"repeat\":\"track\",\"results\":" + results + ",\"devices\":[{\"id\":\"x\",\"name\":\""
                + LONG + "\",\"active\":true}],\"error\":\"" + LONG + "\"}");
        StringBuilder sites = new StringBuilder("[");
        for (int i = 0; i < 40; i++) {
            sites.append(i == 0 ? "" : ",").append("{\"name\":\"").append(i == 0 ? "discord" : "site" + i)
                    .append("\",\"title\":\"").append(LONG).append("\"}");
        }
        sites.append("]");
        line.state("browser", "{\"sites\":" + sites + ",\"open\":null,\"host\":\"" + LONG + "\",\"loading\":true,\"title\":\""
                + LONG + "\",\"error\":\"" + LONG + "\",\"signed_in_hint\":\"" + LONG + "\"}");
        StringBuilder clips = new StringBuilder("[");
        for (int i = 0; i < 64; i++) {
            clips.append(i == 0 ? "" : ",").append("{\"id\":\"clip").append(i).append("\",\"name\":\"").append(LONG)
                    .append("\",\"created\":\"").append(LONG).append("\",\"seconds\":86400,\"bytes\":999999999999999}");
        }
        clips.append("]");
        line.state("clips", "{\"recording\":true,\"buffer_seconds\":86400,\"clips\":" + clips + ",\"folder\":\"" + LONG
                + "\",\"error\":\"" + LONG + "\"}");
        line.state("realms", "{\"mode\":\"totp\",\"prompt\":{\"world\":\"" + LONG + "\",\"address\":\"" + LONG
                + "\",\"expires_in\":86400,\"attempts_left\":1000},\"last\":\"refused\"}");
        line.state("settings", "{\"sync\":false,\"last_sync\":\"" + LONG + "\",\"keys\":\"" + LONG + "\",\"instances\":[\""
                + LONG + "\",\"" + LONG + "\"],\"error\":\"" + LONG + "\"}");
    }

    static void noStates() {
        line.link(false);
    }

    static void wideScriptStates() {
        allStates();
        String cjk = "東京の夜空に響く歌声と遠い記憶の物語".repeat(4);
        line.state("spotify", "{\"source\":\"mpris\",\"playing\":true,\"track\":\"" + cjk + "\",\"artist\":\"" + cjk
                + "\",\"album\":\"Ünïcödé §k\\u202eevil\\u0007\",\"duration_ms\":1000,\"position_ms\":0,\"volume\":0,"
                + "\"results\":[],\"devices\":[]}");
    }

    // ------------------------------------------------------------------ scenario: hostile launcher messages

    static final String[] HOSTILE = {
        "", "null", "[]", "[1,2]", "\"text\"", "42", "{", "{\"a\":", "{\"apps\":[1,2,{}]}", "{\"apps\":\"spotify\"}",
        "{\"apps\":[\"Spotify\",\"-x\",\"seventeen-letters\",\"ok\",\"ok\"]}",
        "{\"source\":7,\"playing\":\"yes\",\"track\":{},\"duration_ms\":-5,\"position_ms\":1e308,\"volume\":-1e308}",
        "{\"source\":\"web\",\"duration_ms\":1e400}",
        "{\"results\":[{\"uri\":\"spotify:track:../../etc/passwd\"},{\"uri\":\"spotify:track:abc def\"},"
                + "{\"uri\":\"https://evil.example/\"},{\"uri\":\"spotify:user:x\"},null,7]}",
        "{\"devices\":[{\"id\":\"a b\"},{\"id\":\"--\"},{\"id\":\"" + "x".repeat(65) + "\"},{\"id\":\"é\"}]}",
        "{\"sites\":[{\"name\":\"open --\"},{\"name\":\"--\"},{\"name\":\"\"},{\"name\":12}],\"open\":\"a b\"}",
        "{\"clips\":[{\"id\":\"../x\",\"seconds\":-1,\"bytes\":-9},{\"id\":\"ok\",\"seconds\":1e18}],\"buffer_seconds\":-30}",
        "{\"mode\":\"evil\",\"prompt\":{\"expires_in\":-100,\"attempts_left\":-1},\"last\":\"maybe\"}",
        "{\"mode\":\"code\",\"prompt\":[]}",
        "{\"sync\":\"true\",\"keys\":{},\"instances\":[1,null,\"\\u202e\"]}",
        "{\"track\":\"\\ud800\\udc00\\ud800 lone \\udfff\"}",
        "{\"track\":\"\\u0000\\u001b[31mred\\u007f\\u009b\"}",
        "{\"a\":" + "[".repeat(40) + "]".repeat(40) + "}",
        "{\"a\":1}trailing", "{\"a\":01}", "{\"a\":.5}", "{\"a\":NaN}", "{'a':1}", "{\"a\":1,}",
    };

    static void hostile() {
        start(480, 270);
        allStates();
        key(Game.PHONE_KEY);
        settle("hostile home");
        String[] apps = {"phone", "spotify", "browser", "clips", "realms", "settings"};
        int exceptions = 0;
        for (String json : HOSTILE) {
            for (String app : apps) {
                line.state(app, json);
            }
            exceptions += exercise("hostile " + json.substring(0, Math.min(30, json.length())));
            allStates();
        }
        // Mutations of well-formed states: bytes flipped, inserted, removed.
        Random random = new Random(20261114L);
        String[] sources = {PHONE_ALL, spotify(true, RESULTS, DEVICES), browser("discord"), CLIPS, REALMS_PROMPT, SETTINGS};
        int mutated = 0;
        for (int round = 0; round < 300; round++) {
            int which = random.nextInt(sources.length);
            StringBuilder s = new StringBuilder(sources[which]);
            int edits = 1 + random.nextInt(4);
            for (int e = 0; e < edits && s.length() > 0; e++) {
                int at = random.nextInt(s.length());
                switch (random.nextInt(4)) {
                    case 0:
                        s.setCharAt(at, (char) random.nextInt(0x80));
                        break;
                    case 1:
                        s.insert(at, "\"\\u202e§k\\u0000x\"".charAt(random.nextInt(14)));
                        break;
                    case 2:
                        s.deleteCharAt(at);
                        break;
                    default:
                        s.insert(at, String.valueOf(random.nextInt()));
                }
            }
            line.state(apps[which], s.toString());
            mutated++;
            if (round % 10 == 0) {
                exceptions += exercise("mutated " + round);
            }
        }
        exceptions += exercise("mutated final");
        check(exceptions == 0, "no launcher message makes the phone throw (" + HOSTILE.length + " hostile, " + mutated
                + " mutated): " + exceptions);
        // Pictures of every allowed size, including huge and tiny ones, in every slot.
        for (int[] wh : new int[][] {{1, 1}, {2048, 1}, {1, 2048}, {320, 320}, {1280, 1280}}) {
            // Every slot (one past the last too) at the small sizes; the large picture (6.5 MB
            // as pixels) in three slots, so that the check stays within a small heap.
            int slots = wh[0] * wh[1] > 320 * 320 ? 3 : 34;
            for (int slot = 0; slot < slots; slot++) {
                line.picture("clips", slot, picture(wh[0], wh[1], 0xFF000000));
            }
            line.picture("spotify", 0, picture(wh[0], wh[1], 0xFF000000));
            line.picture("browser", 0, picture(wh[0], wh[1], 0xFF000000));
            exceptions += exercise("picture " + wh[0] + "x" + wh[1]);
        }
        for (int i = 0; i < 100; i++) {
            line.notice("spotify", "notice " + i + " \u202e\u0007§k " + LONG);
        }
        exceptions += exercise("notices");
        check(exceptions == 0, "no picture or notice makes the phone throw: " + exceptions);
        check(phone.store().notices().size() <= 20, "notices are bounded: " + phone.store().notices().size());
        check(game.textures - game.texturesClosed <= 2 + 33, "textures are reused or released: "
                + game.textures + " made, " + game.texturesClosed + " released");
    }

    /** Every app, every tab, every control pressed once; returns the exceptions seen. */
    static int exercise(String where) {
        int exceptions = 0;
        String[] apps = {null, "spotify", "discord", "browser", "clips", "realms", "settings"};
        for (String app : apps) {
            try {
                if (app == null) {
                    phone.openApp("settings");
                    key(Keys.ESCAPE);
                } else {
                    phone.openApp(app);
                }
                frame(where + " " + app);
                for (int round = 0; round < 2; round++) {
                    List<String> ids = new ArrayList<>();
                    for (Hits.Hit h : phone.hits().shown()) {
                        ids.add(h.id);
                    }
                    for (String id : ids) {
                        if (id.equals("back") || id.equals("home") || id.startsWith("app:") || id.equals("banner")) {
                            continue;
                        }
                        click(id);
                        type("x1");
                        key(Keys.ENTER);
                        key(Keys.ESCAPE);
                        frame(where + " " + app + " after " + id);
                        if (game.open != phone) {
                            key(Game.PHONE_KEY);
                            settle(where + " reopen");
                        }
                        if (app != null && !app.equals(phone.currentId())) {
                            phone.openApp(app);
                        }
                    }
                }
                key(Keys.ESCAPE);
                key(Keys.ESCAPE);
                if (game.open != phone) {
                    key(Game.PHONE_KEY);
                    settle(where + " reopen");
                }
            } catch (RuntimeException e) {
                exceptions++;
                System.out.println("exception " + where + " " + app + ": " + e);
                for (StackTraceElement el : e.getStackTrace()) {
                    if (el.getClassName().startsWith("bastion")) {
                        System.out.println("  at " + el);
                        break;
                    }
                }
                start(480, 270);
                allStates();
                key(Game.PHONE_KEY);
                settle("restart");
            }
        }
        return exceptions;
    }

    // ------------------------------------------------------------------ scenario: the home screen

    static void homeScreen() {
        Clock.source = () -> now;
        game = new Game(480, 270);
        HomeScreen home = new HomeScreen(game);
        game.setHome(home);
        game.open(home);
        advance(500);
        frame("home screen");
        for (String id : new String[] {"home:PLAY_DEMO", "home:OPTIONS", "home:QUIT", "home:LANGUAGE", "home:ACCESSIBILITY",
            "home:credits"}) {
            check(has(id), "the home screen offers " + id);
        }
        check(!has("home:SINGLEPLAYER") && !has("home:MULTIPLAYER"), "demo mode: the demo world instead of the others");
        click("home:PLAY_DEMO");
        click("home:OPTIONS");
        click("home:credits");
        check(game.performed.equals(List.of("PLAY_DEMO", "OPTIONS", "CREDITS")), "buttons do what the title screen's do: "
                + game.performed);
        game.performed.clear();
        game.demo = false;
        game.modMenu = true;
        frame("home screen full");
        for (String id : new String[] {"home:SINGLEPLAYER", "home:MULTIPLAYER", "home:REALMS", "home:MODS"}) {
            check(has(id), "the full home screen offers " + id);
        }
        key(Keys.DOWN);
        key(Keys.ENTER);
        check(game.performed.size() == 1, "keyboard: Down and Enter use a button: " + game.performed);
        key(Keys.ESCAPE);
        check(game.open == home, "Escape does not leave the home screen");
    }

    // ------------------------------------------------------------------ main

    public static void main(String[] args) {
        Clock.source = () -> now;
        useEveryApp();
        homeScreen();
        everyScreen("rich", PhoneUiCheck::richStates);
        everyScreen("long", PhoneUiCheck::longStates);
        everyScreen("offline", PhoneUiCheck::noStates);
        everyScreen("wide", PhoneUiCheck::wideScriptStates);
        hostile();
        for (String v : violations) {
            System.out.println("violation " + v);
        }
        System.out.println("count violations=" + violationCount);
        System.out.println("count frames=" + frames);
        System.out.println("count draws=" + drawOps);
        System.out.println("count checks=" + checks);
        System.out.println("count failures=" + failures);
        System.out.println("done");
    }

    static Map<String, Integer> unused = new HashMap<>();
}
