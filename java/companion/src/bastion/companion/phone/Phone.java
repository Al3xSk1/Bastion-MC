package bastion.companion.phone;

import java.time.LocalDate;
import java.time.LocalTime;
import java.time.format.DateTimeFormatter;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.HudLayer;
import bastion.companion.platform.Keys;
import bastion.companion.platform.Link;
import bastion.companion.platform.Overlay;
import bastion.companion.platform.Platform;
import bastion.companion.theme.Theme;
import bastion.companion.ui.Gfx;
import bastion.companion.ui.Hits;
import bastion.companion.ui.Icons;
import bastion.companion.ui.Kit;
import bastion.companion.ui.Rect;
import bastion.companion.ui.TextField;
import bastion.companion.ui.Clock;

/**
 * The phone: pulled out with the phone key, it slides up at the right of the screen like a
 * phone held in the hand. The game keeps running behind it (it is not a pause screen and draws
 * no backdrop); the mouse is free while it is out and captured again when it is put away.
 *
 * What it shows: a status bar (the time, unread notices, the link to the launcher), a home
 * screen with the apps the launcher announced for this launch, and one screen per app. Every
 * action is one command line to the launcher ({@link Send}); the phone holds no secret and
 * shows only what the launcher sends.
 *
 * Input: the mouse, and the keyboard alone — arrow keys and Tab move the focus, Enter or Space
 * uses what has it, Escape and Backspace go back (then put the phone away), the phone key puts
 * it away (unless a text field has the focus: then it is a letter like any other).
 */
public final class Phone implements Overlay, HudLayer {
    static final long SLIDE_NANOS = 200_000_000L;
    static final int MARGIN = 8;
    static final int BEZEL = 4;
    static final int STATUS = 11;
    static final int NAV = 8;
    static final int HEADER = 14;
    static final long BANNER_NANOS = 4_000_000_000L;
    static final long TOAST_NANOS = 5_000_000_000L;

    final Platform platform;
    final Link link;
    final Store store;
    final Send send;
    final Hits hits = new Hits();
    private final List<App> apps = new ArrayList<>();
    private App current;
    private int width = 320;
    private int height = 240;
    private boolean closing;
    private long slideStart;
    private boolean settled;
    private boolean closeScheduled;
    private Rect lastScreen = Rect.EMPTY;
    private Rect lastBody = Rect.EMPTY;
    private Store.Notice dismissed;

    public Phone(Platform platform, Link link) {
        this.platform = platform;
        this.link = link;
        this.store = new Store(platform);
        this.send = new Send(link);
        apps.add(new SpotifyApp(this));
        apps.add(new BrowserApp(this, "discord"));
        apps.add(new BrowserApp(this, null));
        apps.add(new ClipsApp(this));
        apps.add(new RealmsApp(this));
        apps.add(new SettingsApp(this));
    }

    /** What receives the link's deliveries ({@code Link.setListener}). */
    public Store store() {
        return store;
    }

    public Send sender() {
        return send;
    }

    /** The places of the last frame (the tests and the self-test press them by name). */
    public Hits hits() {
        return hits;
    }

    /** Turned sideways for the app on the screen (the browser's reading mode). */
    public boolean landscape() {
        return current != null && current.landscape();
    }

    /** Where the phone's body was drawn in the last frame (empty before the first). */
    public Rect lastBody() {
        return lastBody;
    }

    // ------------------------------------------------------------------ pulling it out

    /** The phone key: pull the phone out, or put it away. */
    public void toggle() {
        if (platform.isOpen(this)) {
            if (closing) {
                pullOut();
            } else {
                putAway();
            }
        } else {
            pullOut();
        }
    }

    public boolean isOut() {
        return platform.isOpen(this) && !closing;
    }

    /** Out and done sliding: what the player sees is where input lands. */
    public boolean settled() {
        return settled && !closing;
    }

    void pullOut() {
        boolean wasOpen = platform.isOpen(this);
        closing = false;
        settled = false;
        slideStart = Clock.now();
        if (!wasOpen) {
            platform.open(this);
        }
        if (current != null) {
            current.shown();
        }
    }

    void putAway() {
        if (closing) {
            return;
        }
        closing = true;
        settled = false;
        slideStart = Clock.now();
        hits.focus(null);
        if (current != null) {
            current.hidden();
        }
    }

    /** Opens {@code app} on the phone. */
    void open(App app) {
        if (current == app) {
            return;
        }
        if (current != null) {
            current.hidden();
        }
        current = app;
        hits.focus(null);
        if (app != null) {
            app.shown();
        }
    }

    void goHome() {
        open(null);
    }

    /** The app with that id, or null. */
    App app(String id) {
        for (App app : apps) {
            if (app.id().equals(id)) {
                return app;
            }
        }
        return null;
    }

    /** The app on the screen, or null for the home screen. */
    public String currentId() {
        return current == null ? null : current.id();
    }

    /** Opens the app with that id (the self-test; a notice's banner). */
    public void openApp(String id) {
        App app = app(id);
        if (app != null && app.available()) {
            open(app);
        }
    }

    // ------------------------------------------------------------------ geometry

    /**
     * Where the phone is when it is all the way out, on a screen of sw × sh GUI units: at the
     * right, standing on the bottom margin; or, turned sideways, centred at the bottom. Never
     * larger than the screen less its margins.
     */
    public static Rect body(int sw, int sh, boolean landscape) {
        return body(sw, sh, landscape, false);
    }

    /** The game's hotbar: 182 units wide, centred, 22 high at the bottom of the screen. */
    static final int HOTBAR_HALF = 91;
    static final int HOTBAR_ROW = 22 + 4;
    /** Shorter than this, the phone stands on the bottom margin over the hotbar after all. */
    static final int MIN_RAISED = 180;

    /**
     * As {@link #body(int, int, boolean)}; {@code inWorld}: a world's HUD is drawn under the
     * phone, and a phone standing on the bottom margin over the hotbar's right end stands on the
     * hotbar row instead (hearts, food and the hotbar stay visible), where that leaves it the
     * minimum height. Turned sideways (for reading) it covers the HUD regardless.
     */
    public static Rect body(int sw, int sh, boolean landscape, boolean inWorld) {
        Rect plain = bodyAbove(sw, sh, landscape, 0);
        if (!inWorld || landscape || plain.empty() || plain.x() >= sw / 2 + HOTBAR_HALF + 2) {
            return plain;
        }
        Rect raised = bodyAbove(sw, sh, false, HOTBAR_ROW);
        return raised.h() >= MIN_RAISED ? raised : plain;
    }

    private static Rect bodyAbove(int sw, int sh, boolean landscape, int reserve) {
        int margin = Math.min(MARGIN, Math.max(0, Math.min(sw, sh) / 20));
        int availW = Math.max(0, sw - 2 * margin);
        int availH = Math.max(0, sh - 2 * margin - reserve);
        if (landscape) {
            int w = Math.min(Math.min(availW, 900), Math.round(availH * 1.75f));
            int h = Math.min(availH, Math.round(w / 1.75f));
            return new Rect((sw - w) / 2, sh - margin - h, w, h);
        }
        int full = availH + reserve;
        int h = availH >= MIN_RAISED
                ? Math.min(Math.min(availH, 320), Math.max(Math.min(220, availH), Math.round(sh * 0.82f) - reserve))
                : availH;
        int w = Math.min(availW, Math.max(120, Math.min(196, Math.round(Math.max(h, Math.min(full, 220)) * 0.66f))));
        int right = Math.min(sw - margin, sw - Math.max(margin, sw / 40));
        return new Rect(Math.max(margin, right - w), sh - margin - reserve - h, w, h);
    }

    private static float ease(float t) {
        float u = 1.0f - Math.max(0.0f, Math.min(1.0f, t));
        return 1.0f - u * u * u;
    }

    // ------------------------------------------------------------------ Overlay

    @Override
    public void opened(int width, int height) {
        this.width = width;
        this.height = height;
    }

    @Override
    public boolean closesOnEscape() {
        return false;
    }

    @Override
    public boolean gameBackground() {
        return false;
    }

    @Override
    public void closed() {
        if (!closing && current != null) {
            current.hidden();
        }
        closing = false;
        settled = false;
        hits.clear();
    }

    @Override
    public void tick() {
        if (current != null && !closing) {
            current.tick();
        }
    }

    @Override
    public void render(Canvas c, int mouseX, int mouseY, float partialTick) {
        long now = Clock.now();
        width = c.width();
        height = c.height();
        float t = Math.min(1.0f, (now - slideStart) / (float) SLIDE_NANOS);
        if (closing && t >= 1.0f) {
            // Put away: close the screen after this frame, not in the middle of drawing it.
            if (!closeScheduled) {
                closeScheduled = true;
                platform.runOnRenderThread(() -> {
                    closeScheduled = false;
                    if (closing) {
                        platform.close(this);
                    }
                });
            }
            return;
        }
        settled = !closing && t >= 1.0f;
        float out = closing ? 1.0f - ease(t) : ease(t);
        boolean landscape = current != null && current.landscape();
        Rect rest = body(width, height, landscape, platform.inWorld());
        int travel = height - rest.y() + 4;
        Rect body = rest.moved(0, Math.round((1.0f - out) * travel));
        Rect screen = body.inset(BEZEL);
        Rect visible = screen.intersect(new Rect(0, 0, width, height));
        lastScreen = visible;
        lastBody = body;
        c.clip(0, 0, width, height);
        try {
            if (!platform.inWorld()) {
                // Out of a world (over the home screen) nothing would be behind the phone:
                // the dark background of the game's menus, fading in with the slide.
                backdrop(c, out);
            }
            drawBody(c, body);
            hits.begin(visible);
            Kit k = new Kit(c, hits, mouseX, mouseY, now, platform::clickSound);
            Gfx.clipped(c, visible, () -> drawScreen(k, screen, now));
            hits.end();
        } finally {
            c.unclip();
        }
    }

    private void backdrop(Canvas c, float out) {
        c.fill(0, 0, width, height, Theme.alpha(Theme.VOID, Math.max(0.0f, Math.min(1.0f, out))));
        if (out >= 1.0f) {
            bastion.companion.theme.DarkMenus.paintBackground(c, false);
        }
    }

    private void drawBody(Canvas c, Rect body) {
        if (body.empty()) {
            return;
        }
        c.roundRect(body.x(), body.y(), body.w(), body.h(), 9, Theme.LINE_HI);
        c.roundRect(body.x() + 1, body.y() + 1, body.w() - 2, body.h() - 2, 8, Theme.DECK);
        int slit = Math.min(16, body.w() / 6);
        c.fill(body.centerX() - slit / 2, body.y() + 2, body.centerX() + slit / 2, body.y() + 3, Theme.LINE);
    }

    private void drawScreen(Kit k, Rect screen, long now) {
        Canvas c = k.c;
        c.roundRect(screen.x(), screen.y(), screen.w(), screen.h(), 3, Theme.VOID);
        Rect status = screen.top(STATUS);
        Rect below = screen.belowTop(STATUS);
        Rect nav = below.bottomRows(NAV);
        Rect content = below.aboveBottom(NAV);
        drawStatus(k, status);
        if (current == null) {
            drawHome(k, content, now);
        } else {
            Rect header = content.top(HEADER);
            drawHeader(k, header);
            current.render(k, content.belowTop(HEADER).inset(4, 2));
        }
        drawNav(k, nav);
        drawBanner(k, content, now);
    }

    private void drawStatus(Kit k, Rect status) {
        Canvas c = k.c;
        Rect bar = status.inset(4, 0);
        String time = LocalTime.now().format(DateTimeFormatter.ofPattern("HH:mm", Locale.ROOT));
        int used = Gfx.label(c, time, bar, Theme.TEXT);
        Rect right = bar.rightOf(used + 4);
        int indicator;
        if (store.connected()) {
            // Four bars, the launcher's link: it says the line is up, nothing about strength.
            int bottom = status.bottom() - 2;
            int x = right.right() - 7;
            for (int i = 0; i < 4; i++) {
                int top = Math.max(status.y() + 1, bottom - 2 * (i + 1));
                c.fill(x + 2 * i, top, x + 2 * i + 1, bottom, Theme.ACCENT);
            }
            indicator = 9;
        } else {
            indicator = Gfx.caps(c, "offline", right, Theme.RED, Gfx.RIGHT) + 3;
        }
        int unread = store.unread();
        if (unread > 0) {
            Rect middle = right.leftOf(indicator);
            String count = String.valueOf(unread);
            int w = c.textWidth(count) + 9;
            if (middle.w() >= w) {
                Rect spot = middle.rightCols(w);
                Icons.draw(c, Icons.BELL, spot.x(), status.y() + 2, 1, Theme.AMBER);
                Gfx.label(c, count, spot.rightOf(9), Theme.AMBER);
            }
        }
    }

    private void drawHeader(Kit k, Rect header) {
        Canvas c = k.c;
        Gfx.hline(c, header.x(), header.right(), header.bottom() - 1, Theme.LINE);
        Rect row = header.aboveBottom(1).inset(2, 1);
        k.iconButton("back", row.left(12), Icons.BACK, false, this::back);
        Rect rest = row.rightOf(15);
        String badge = current.badge();
        int badgeWidth = 0;
        if (badge != null && !badge.isEmpty()) {
            badgeWidth = Gfx.caps(c, badge, rest.rightCols(Math.min(rest.w() / 2, Gfx.capsWidth(c, badge))),
                    Kit.toneColor(current.badgeTone()), Gfx.RIGHT);
        }
        Gfx.caps(c, current.title(), rest.leftOf(badgeWidth > 0 ? badgeWidth + 4 : 0), Theme.TEXT, Gfx.LEFT);
    }

    private void drawNav(Kit k, Rect nav) {
        Canvas c = k.c;
        int w = Math.min(36, nav.w() / 3);
        Rect bar = new Rect(nav.centerX() - w / 2, nav.centerY() - 1, w, 2);
        Rect hit = nav.center(w + 16, nav.h());
        boolean hot = current != null && k.hover(hit);
        c.fill(bar.x(), bar.y(), bar.right(), bar.bottom(), hot ? Theme.ACCENT : Theme.LINE_HI);
        if (current != null) {
            k.hits.area("home", hit, this::goHome);
        }
    }

    private void drawBanner(Kit k, Rect content, long now) {
        Store.Notice latest = store.latest();
        if (latest == null || latest == dismissed || now - latest.nanos() > BANNER_NANOS
                || (current == null && content.h() > 120)) {
            return;
        }
        Canvas c = k.c;
        // Two lines (the app, the notice) inside a panel: 2 lines + 4 inside + 4 around.
        Rect banner = content.top(2 * c.lineHeight() + 8).inset(3, 2);
        if (banner.h() < 2 * c.lineHeight() + 4) {
            return;
        }
        Gfx.panel(c, banner, Theme.PLATE_HI, Theme.LINE_HI, 2);
        Rect inner = banner.inset(4, 2);
        App source = app(latest.app());
        String title = source != null ? source.title() : latest.app();
        Gfx.caps(c, title, inner.top(c.lineHeight()), Theme.TEXT_FAINT, Gfx.LEFT);
        Gfx.label(c, latest.text(), inner.belowTop(c.lineHeight()).top(c.lineHeight()), Theme.TEXT);
        k.hits.area("banner", banner, () -> {
            dismissed = latest;
            if (source != null && source.available()) {
                open(source);
            }
        });
    }

    // ------------------------------------------------------------------ the home screen

    private void drawHome(Kit k, Rect content, long now) {
        Canvas c = k.c;
        Rect area = content.inset(5, 3);
        int line = c.lineHeight();
        // A clock, large where there is room.
        String time = LocalTime.now().format(DateTimeFormatter.ofPattern("HH:mm", Locale.ROOT));
        int scale = area.h() >= 170 && c.textWidth(time) * 2 <= area.w() ? 2 : 1;
        int clockHeight = line * scale;
        if (area.h() >= 120) {
            int tw = c.textWidth(time) * scale;
            int x = area.centerX() - tw / 2;
            c.push();
            c.translate(x, area.y() + 2);
            c.scale(scale);
            c.text(time, 0, 0, Theme.TEXT, false);
            c.pop();
            String date = LocalDate.now().format(DateTimeFormatter.ofPattern("EEE d MMM", Locale.ROOT));
            Gfx.label(c, date, new Rect(area.x(), area.y() + 3 + clockHeight, area.w(), line), Theme.TEXT_DIM,
                    Gfx.CENTER, false);
            area = area.belowTop(clockHeight + line + 8);
        }
        List<App> shown = new ArrayList<>();
        for (App app : apps) {
            if (app.available()) {
                shown.add(app);
            }
        }
        if (shown.isEmpty()) {
            String why = store.connected() ? "The launcher announced no apps for this game."
                    : "Not connected to the launcher. The phone works only in a game the launcher started.";
            k.paragraph(area.inset(2, 2), why, Theme.TEXT_DIM);
            return;
        }
        int cols = area.w() >= 150 ? 3 : 2;
        int tileH = 26 + 3 + line + 4;
        int rows = (shown.size() + cols - 1) / cols;
        Rect grid = area.top(rows * tileH);
        for (int i = 0; i < shown.size(); i++) {
            App app = shown.get(i);
            int col = i % cols;
            int row = i / cols;
            Rect tile = grid.belowTop(row * tileH).top(tileH).column(col, cols, 2);
            drawTile(k, tile, app);
        }
        // The latest notices, where there is room.
        Rect rest = area.belowTop(rows * tileH + 4);
        List<Store.Notice> notices = store.notices();
        if (!notices.isEmpty() && rest.h() >= 3 * line) {
            Gfx.hline(c, rest.x(), rest.right(), rest.y(), Theme.LINE);
            k.caption(rest.belowTop(2).top(line + 1), "notices");
            Rect list = rest.belowTop(line + 4);
            int y = list.y();
            for (Store.Notice notice : notices) {
                if (y + 2 * line > list.bottom()) {
                    break;
                }
                App source = app(notice.app());
                Gfx.caps(c, source != null ? source.title() : notice.app(), new Rect(list.x(), y, list.w(), line),
                        Theme.TEXT_FAINT, Gfx.LEFT);
                Gfx.label(c, notice.text(), new Rect(list.x(), y + line, list.w(), line), Theme.TEXT_DIM);
                y += 2 * line + 2;
            }
            store.markRead();
        } else if (notices.isEmpty()) {
            store.markRead();
        }
    }

    private void drawTile(Kit k, Rect tile, App app) {
        Canvas c = k.c;
        String id = "app:" + app.id();
        boolean hot = k.hot(id, tile);
        Rect icon = tile.top(26).center(26, 26);
        c.fill(icon.x(), icon.y(), icon.right(), icon.bottom(), hot ? Theme.PLATE_HI : Theme.PLATE);
        Gfx.frame(c, icon, hot ? Theme.ACCENT : Theme.LINE_HI);
        Icons.drawIn(c, app.icon(), icon.inset(2), 2, hot ? Theme.TEXT : Theme.ACCENT);
        if (app instanceof RealmsApp && store.realms().prompt != null) {
            c.fill(icon.right() - 5, icon.y() + 1, icon.right() - 1, icon.y() + 5, Theme.AMBER);
        }
        Rect label = tile.belowTop(29).top(c.lineHeight());
        Gfx.label(k.c, app.title(), label, hot ? Theme.TEXT : Theme.TEXT_DIM, Gfx.CENTER, false);
        k.hits.button(id, tile, () -> {
            platform.clickSound();
            open(app);
        });
    }

    // ------------------------------------------------------------------ input

    /** Escape, Backspace, the back arrow: one step back; from the home screen, put the phone away. */
    void back() {
        if (hits.focusedField() != null) {
            hits.focus(null);
            return;
        }
        if (current != null) {
            if (!current.back()) {
                goHome();
            }
            return;
        }
        putAway();
    }

    private boolean busy() {
        return closing || !settled;
    }

    @Override
    public boolean keyPressed(int key, int modifiers) {
        if (closing) {
            return true;
        }
        if (key == Keys.ESCAPE) {
            back();
            return true;
        }
        if (current != null && current.capturesKeys()) {
            current.key(key, modifiers);
            return true;
        }
        TextField field = hits.focusedField();
        if (field != null) {
            if (key == Keys.TAB) {
                hits.cycle((modifiers & Keys.MOD_SHIFT) != 0 ? -1 : 1);
            } else if (key == Keys.UP || key == Keys.DOWN) {
                hits.move(0, key == Keys.DOWN ? 1 : -1);
            } else {
                field.key(key, modifiers);
            }
            // Everything else is typing: never a binding, never the phone key.
            return true;
        }
        switch (key) {
            case Keys.UP:
                hits.move(0, -1);
                return true;
            case Keys.DOWN:
                hits.move(0, 1);
                return true;
            case Keys.LEFT:
                hits.move(-1, 0);
                return true;
            case Keys.RIGHT:
                hits.move(1, 0);
                return true;
            case Keys.TAB:
                hits.cycle((modifiers & Keys.MOD_SHIFT) != 0 ? -1 : 1);
                return true;
            case Keys.ENTER:
            case Keys.KEYPAD_ENTER:
            case Keys.SPACE:
                if (!hits.activate() && current != null) {
                    current.key(key, modifiers);
                }
                return true;
            case Keys.BACKSPACE:
                back();
                return true;
            default:
                return current != null && current.key(key, modifiers);
        }
    }

    @Override
    public boolean charTyped(int codePoint, int modifiers) {
        if (closing) {
            return true;
        }
        if (current != null && current.capturesKeys()) {
            current.typed(codePoint);
            return true;
        }
        TextField field = hits.focusedField();
        if (field != null) {
            field.typed(codePoint);
            return true;
        }
        return false;
    }

    @Override
    public boolean mousePressed(double x, double y, int button, int modifiers) {
        if (busy()) {
            return true;
        }
        if (button == Keys.MOUSE_LEFT && hits.press(x, y)) {
            return true;
        }
        if (current != null && current.press(x, y, button)) {
            return true;
        }
        if (lastScreen.contains(x, y)) {
            hits.focus(null);
            return true;
        }
        return false;
    }

    @Override
    public boolean mouseReleased(double x, double y, int button, int modifiers) {
        if (hits.release(x, y)) {
            return true;
        }
        return current != null && !closing && current.release(x, y, button);
    }

    @Override
    public boolean mouseDragged(double x, double y, int button, double dx, double dy) {
        if (hits.drag(x, y)) {
            return true;
        }
        return current != null && !busy() && current.drag(x, y, button);
    }

    @Override
    public boolean mouseScrolled(double x, double y, double horizontal, double vertical) {
        if (busy()) {
            return true;
        }
        return current != null && current.scroll(x, y, vertical);
    }

    // ------------------------------------------------------------------ HudLayer

    /**
     * While the phone is away: a notice that has just arrived, or a Realms sign-in waiting for
     * its code, peeks in at the right edge with the phone key's name.
     */
    @Override
    public void render(Canvas c, float partialTick, boolean screenOpen) {
        if (screenOpen || platform.isOpen(this)) {
            return;
        }
        long now = Clock.now();
        String title;
        String text;
        int tone;
        AppStates.Realms realms = store.realms();
        Store.Notice latest = store.latest();
        if (realms.prompt != null && realms.secondsLeft(now) > 0) {
            title = "Realms";
            text = "Sign-in waiting for your code";
            tone = Theme.AMBER;
        } else if (latest != null && now - latest.nanos() < TOAST_NANOS) {
            App source = app(latest.app());
            title = source != null ? source.title() : latest.app();
            text = latest.text();
            tone = Theme.TEXT_FAINT;
        } else {
            return;
        }
        int sw = c.width();
        int sh = c.height();
        int line = c.lineHeight();
        int w = Math.max(0, Math.min(Math.max(110, Math.round(sw * 0.4f)), Math.min(170, sw - 16)));
        int h = 2 * line + 8;
        Rect toast = new Rect(sw - 8 - w, sh - 52 - h, w, h);
        if (toast.y() < 0 || toast.w() < 40) {
            return;
        }
        Gfx.panel(c, toast, Theme.alpha(Theme.VOID, 0.92f), Theme.LINE_HI, 2);
        Rect inner = toast.inset(5, 3);
        String key = platform.phoneKeyName();
        int keyWidth = 0;
        if (key != null && !key.isEmpty()) {
            keyWidth = Gfx.caps(c, key, inner.top(line).rightCols(Math.min(inner.w() / 3, Gfx.capsWidth(c, key) + 1)),
                    Theme.ACCENT, Gfx.RIGHT);
        }
        Gfx.caps(c, title, inner.top(line).leftOf(keyWidth + 3), tone, Gfx.LEFT);
        Gfx.label(c, text, inner.belowTop(line + 1).top(line), Theme.TEXT);
    }
}
