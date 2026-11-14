package bastion.companion.phone;

import java.nio.charset.StandardCharsets;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Keys;
import bastion.companion.platform.Texture;
import bastion.companion.theme.Theme;
import bastion.companion.ui.Column;
import bastion.companion.ui.Gfx;
import bastion.companion.ui.Icons;
import bastion.companion.ui.Kit;
import bastion.companion.ui.Rect;
import bastion.companion.ui.Scroller;
import bastion.companion.ui.Clock;

/**
 * The browser — and Discord, which is the browser opened at the configured site named
 * {@code discord}. The page runs in the launcher's separate browser sandbox; the phone shows
 * the pictures it sends (slot 0) and sends back where the mouse went and what was typed.
 *
 * Verbs: open &lt;site&gt;, close, back, forward, reload, size &lt;w&gt; &lt;h&gt;,
 * mouse move|down|up &lt;x&gt; &lt;y&gt; [button], scroll &lt;dx&gt; &lt;dy&gt;, key &lt;name&gt;,
 * text -- &lt;text&gt;, frames on|off.
 *
 * Mouse positions are mapped from the phone to the page picture's own pixels. Clicking the
 * page gives it the keyboard (a frame in the accent says so); Escape takes the keyboard back.
 * Typing is sent in batches (at most every tenth of a second), mouse movement only while a
 * button is held and at most fifteen times a second, the wheel in batches.
 */
final class BrowserApp extends App {
    private static final String APP = "browser";
    static final int MAX_VIEWPORT = 1280;
    static final int MIN_VIEWPORT = 200;
    private static final long TEXT_EVERY_NANOS = 100_000_000L;
    private static final long MOVE_EVERY_NANOS = 66_000_000L;
    private static final long SIZE_SETTLE_NANOS = 300_000_000L;

    /** "discord" for the Discord app; null for the browser with its site list. */
    private final String site;
    private boolean landscape;
    private boolean pageFocus;
    /** Where the last picture was drawn, and its size in pixels. */
    private Rect page = Rect.EMPTY;
    private int frameW;
    private int frameH;
    private boolean framesOn;
    private String sizeSent;
    private String sizeWanted;
    private long sizeWantedSince;
    private final StringBuilder typed = new StringBuilder();
    private long typedSince;
    private double scrollPending;
    private long scrollSince;
    private long lastMove;
    private int heldButton = -1;
    private boolean openAsked;
    private final Scroller siteScroll = new Scroller();

    BrowserApp(Phone phone, String site) {
        super(phone);
        this.site = site;
    }

    @Override
    String id() {
        return site != null ? site : APP;
    }

    @Override
    String title() {
        return site != null ? "Discord" : "Browser";
    }

    @Override
    Icons.Glyph icon() {
        return site != null ? Icons.CHAT : Icons.GLOBE;
    }

    @Override
    boolean available() {
        return phone.store.announced(APP) && (site == null || phone.store.browser().hasSite(site));
    }

    @Override
    boolean landscape() {
        return landscape && showingPage(phone.store.browser());
    }

    @Override
    boolean capturesKeys() {
        return pageFocus;
    }

    @Override
    String badge() {
        AppStates.Browser s = phone.store.browser();
        return showingPage(s) && s.loading ? "loading" : null;
    }

    private boolean showingPage(AppStates.Browser s) {
        return s.open != null && (site == null || site.equals(s.open));
    }

    @Override
    void shown() {
        openAsked = false;
        sizeSent = null;
        if (site != null && !site.equals(phone.store.browser().open) && phone.store.browser().hasSite(site)) {
            openAsked = phone.send.command(APP, "open", site);
        }
    }

    @Override
    void hidden() {
        flushTyping();
        flushScroll();
        if (heldButton >= 0) {
            heldButton = -1;
        }
        if (framesOn) {
            framesOn = false;
            phone.send.command(APP, "frames", "off");
        }
        pageFocus = false;
        page = Rect.EMPTY;
    }

    @Override
    boolean back() {
        if (pageFocus) {
            flushTyping();
            pageFocus = false;
            return true;
        }
        if (landscape && showingPage(phone.store.browser())) {
            landscape = false;
            return true;
        }
        landscape = false;
        return false;
    }

    @Override
    void tick() {
        long now = Clock.now();
        if (typed.length() > 0 && now - typedSince >= TEXT_EVERY_NANOS) {
            flushTyping();
        }
        if (scrollPending != 0 && now - scrollSince >= TEXT_EVERY_NANOS) {
            flushScroll();
        }
    }

    // ------------------------------------------------------------------ drawing

    @Override
    void render(Kit k, Rect area) {
        AppStates.Browser s = phone.store.browser();
        if (showingPage(s)) {
            renderPage(k, area, s);
        } else {
            if (framesOn) {
                framesOn = false;
                phone.send.command(APP, "frames", "off");
            }
            page = Rect.EMPTY;
            pageFocus = false;
            landscape = false;
            if (site != null) {
                renderWaiting(k, area, s);
            } else {
                renderSites(k, area, s);
            }
        }
    }

    private void renderWaiting(Kit k, Rect area, AppStates.Browser s) {
        Canvas c = k.c;
        Column col = new Column(area);
        col.gap(6);
        Icons.drawIn(c, icon(), col.take(24), 2, Theme.TEXT_OFF);
        col.gap(6);
        String text = !s.error.isEmpty() ? s.error
                : openAsked ? "Opening " + title() + "..." : "Opening " + title() + " in the launcher's browser.";
        col.gap(k.paragraph(col.rest().top(3 * c.lineHeight()).inset(2, 0), text,
                s.error.isEmpty() ? Theme.TEXT_DIM : Theme.AMBER));
        col.gap(4);
        k.button("br-open", col.take(14).inset(6, 0), "open", Kit.ACCENT, () -> {
            openAsked = phone.send.command(APP, "open", site);
        });
        hint(k, col.rest().inset(2, 4), s);
    }

    private void hint(Kit k, Rect area, AppStates.Browser s) {
        if (!s.signedInHint.isEmpty()) {
            k.paragraph(area, s.signedInHint, Theme.AMBER);
        }
    }

    private void renderSites(Kit k, Rect area, AppStates.Browser s) {
        Canvas c = k.c;
        int line = c.lineHeight();
        Column col = new Column(area);
        k.caption(col.take(line + 2), "sites");
        col.gap(2);
        Rect bottom = s.error.isEmpty() ? Rect.EMPTY : col.takeBottom(2 * line + 2);
        Rect list = col.middle();
        if (s.sites.isEmpty()) {
            k.paragraph(list.inset(2, 2), "No sites are set up. The launcher's config lists them under [browser] sites.",
                    Theme.TEXT_FAINT);
        } else {
            int rowH = 2 * line + 4;
            siteScroll.measure(s.sites.size() * rowH, list.h());
            String focus = phone.hits.focus();
            k.hits.pushClip(list);
            Gfx.clipped(c, list, () -> {
                for (int i = 0; i < s.sites.size(); i++) {
                    int y = list.y() + i * rowH - siteScroll.offset();
                    if (y + rowH <= list.y() || y >= list.bottom()) {
                        continue;
                    }
                    AppStates.Site entry = s.sites.get(i);
                    String id = "br-site" + i;
                    if (id.equals(focus)) {
                        siteScroll.reveal(i * rowH, (i + 1) * rowH);
                    }
                    k.row(id, new Rect(list.x(), y, list.w() - 3, rowH), entry.title(), entry.name(), false,
                            () -> phone.send.command(APP, "open", entry.name()));
                }
            });
            k.hits.popClip();
            k.scrollbar(list, siteScroll);
        }
        if (!bottom.empty()) {
            k.paragraph(bottom.inset(2, 1), s.error, Theme.RED);
        }
    }

    private void renderPage(Kit k, Rect area, AppStates.Browser s) {
        Canvas c = k.c;
        int line = c.lineHeight();
        Column col = new Column(area);
        Rect bar = col.take(12);
        int b = 12;
        k.iconButton("br-back", bar.left(b), Icons.BACK, false, () -> phone.send.command(APP, "back"));
        k.iconButton("br-forward", bar.rightOf(b + 1).left(b), Icons.FORWARD, false,
                () -> phone.send.command(APP, "forward"));
        k.iconButton("br-reload", bar.rightOf(2 * (b + 1)).left(b), Icons.RELOAD, false,
                () -> phone.send.command(APP, "reload"));
        k.iconButton("br-close", bar.rightCols(b), Icons.CLOSE, false, () -> {
            pageFocus = false;
            phone.send.command(APP, "close");
        });
        k.iconButton("br-rotate", bar.leftOf(b + 1).rightCols(b), Icons.ROTATE, landscape, () -> {
            landscape = !landscape;
            sizeWanted = null;
        });
        Rect title = bar.rightOf(3 * (b + 1) + 2).leftOf(2 * (b + 1) + 2);
        String heading = !s.title.isEmpty() ? s.title : s.host;
        Gfx.label(c, heading, title, Theme.TEXT_DIM, Gfx.LEFT, false);
        col.gap(2);
        String warning = !s.error.isEmpty() ? s.error : s.signedInHint;
        if (!warning.isEmpty()) {
            Gfx.label(c, warning, col.take(line), s.error.isEmpty() ? Theme.AMBER : Theme.RED);
            col.gap(1);
        }
        Rect frame = col.rest();
        if (frame.empty()) {
            return;
        }
        c.fill(frame.x(), frame.y(), frame.right(), frame.bottom(), Theme.WELL);
        Rect inner = frame.inset(1);
        Texture picture = phone.store.picture(APP, 0);
        if (picture != null && picture.width() > 0 && picture.height() > 0 && !inner.empty()) {
            page = SpotifyApp.fitInto(inner, picture.width(), picture.height());
            frameW = picture.width();
            frameH = picture.height();
            c.image(picture, page.x(), page.y(), page.w(), page.h());
        } else {
            page = Rect.EMPTY;
            Gfx.label(c, s.loading ? "Loading..." : "Waiting for the page...", inner.center(inner.w(), line + 2),
                    Theme.TEXT_FAINT, Gfx.CENTER, false);
        }
        if (s.loading && inner.w() > 8) {
            int span = Math.max(8, inner.w() / 4);
            long phase = (k.now / 8_000_000L) % (inner.w() + span);
            int x0 = inner.x() + (int) phase - span;
            c.fill(Math.max(inner.x(), x0), inner.y(), Math.min(inner.right(), x0 + span), inner.y() + 1, Theme.ACCENT);
        }
        Gfx.frame(c, frame, pageFocus ? Theme.ACCENT : Theme.LINE);
        wantViewport(inner, k.now);
        if (!framesOn) {
            framesOn = phone.send.command(APP, "frames", "on");
        }
    }

    /**
     * The viewport the page should have: the page area in real pixels, widened to at least a
     * phone's width (or a small laptop's, turned sideways) so that sites lay themselves out for
     * a screen they know; within [{@link #MIN_VIEWPORT}, {@link #MAX_VIEWPORT}]. Sent once it
     * has stayed the same for a moment.
     */
    static int[] viewport(int areaW, int areaH, double guiScale, boolean landscape) {
        double scale = guiScale > 0 && !Double.isNaN(guiScale) ? guiScale : 2.0;
        double pw = Math.max(1, areaW * scale);
        double ph = Math.max(1, areaH * scale);
        double minW = landscape ? 640 : 360;
        double factor = pw < minW ? minW / pw : 1.0;
        int w = (int) Math.round(Math.max(MIN_VIEWPORT, Math.min(MAX_VIEWPORT, pw * factor)));
        int h = (int) Math.round(Math.max(MIN_VIEWPORT, Math.min(MAX_VIEWPORT, ph * factor)));
        return new int[] {w, h};
    }

    private void wantViewport(Rect inner, long now) {
        int[] size = viewport(inner.w(), inner.h(), phone.platform.guiScale(), landscape);
        String wanted = size[0] + "x" + size[1];
        if (!wanted.equals(sizeWanted)) {
            sizeWanted = wanted;
            sizeWantedSince = now;
        }
        if (!wanted.equals(sizeSent) && now - sizeWantedSince >= SIZE_SETTLE_NANOS) {
            if (phone.send.command(APP, "size", Integer.toString(size[0]), Integer.toString(size[1]))) {
                sizeSent = wanted;
            }
        }
    }

    // ------------------------------------------------------------------ input

    /** The page pixel under (x, y) of the phone, clamped to the picture. */
    int[] toPage(double x, double y) {
        if (page.empty() || frameW <= 0 || frameH <= 0) {
            return null;
        }
        int px = (int) Math.floor((x - page.x()) * frameW / (double) page.w());
        int py = (int) Math.floor((y - page.y()) * frameH / (double) page.h());
        return new int[] {Math.max(0, Math.min(frameW - 1, px)), Math.max(0, Math.min(frameH - 1, py))};
    }

    private static String buttonName(int button) {
        switch (button) {
            case Keys.MOUSE_RIGHT:
                return "right";
            case Keys.MOUSE_MIDDLE:
                return "middle";
            default:
                return "left";
        }
    }

    @Override
    boolean press(double x, double y, int button) {
        if (!page.contains(x, y)) {
            if (pageFocus) {
                flushTyping();
                pageFocus = false;
            }
            return false;
        }
        int[] at = toPage(x, y);
        if (at == null) {
            return false;
        }
        flushTyping();
        pageFocus = true;
        phone.hits.focus(null);
        heldButton = button;
        phone.send.command(APP, "mouse", "down", Integer.toString(at[0]), Integer.toString(at[1]), buttonName(button));
        return true;
    }

    @Override
    boolean release(double x, double y, int button) {
        if (heldButton != button) {
            return false;
        }
        heldButton = -1;
        int[] at = toPage(x, y);
        if (at != null) {
            phone.send.command(APP, "mouse", "up", Integer.toString(at[0]), Integer.toString(at[1]), buttonName(button));
        }
        return true;
    }

    @Override
    boolean drag(double x, double y, int button) {
        if (heldButton < 0) {
            return false;
        }
        long now = Clock.now();
        if (now - lastMove >= MOVE_EVERY_NANOS) {
            int[] at = toPage(x, y);
            if (at != null) {
                lastMove = now;
                phone.send.command(APP, "mouse", "move", Integer.toString(at[0]), Integer.toString(at[1]));
            }
        }
        return true;
    }

    @Override
    boolean scroll(double x, double y, double amount) {
        if (page.contains(x, y)) {
            if (scrollPending == 0) {
                scrollSince = Clock.now();
            }
            // A notch away from the player scrolls the page up: the browser's dy is negative.
            scrollPending -= amount * 100;
            return true;
        }
        if (!showingPage(phone.store.browser())) {
            siteScroll.scroll((int) Math.round(-amount * 12));
            return true;
        }
        return false;
    }

    @Override
    boolean key(int key, int modifiers) {
        if (!pageFocus) {
            return false;
        }
        String name = Keys.browserName(key, modifiers);
        if (name != null && !"Escape".equals(name)) {
            flushTyping();
            phone.send.command(APP, "key", name);
        }
        // Letters and the like come as typed characters.
        return true;
    }

    @Override
    boolean typed(int codePoint) {
        if (!pageFocus) {
            return false;
        }
        if (typed.length() == 0) {
            typedSince = Clock.now();
        }
        typed.appendCodePoint(codePoint);
        if (typed.toString().getBytes(StandardCharsets.UTF_8).length >= 400) {
            flushTyping();
        }
        return true;
    }

    private void flushTyping() {
        if (typed.length() > 0) {
            String text = typed.toString();
            typed.setLength(0);
            phone.send.withText(APP, "text", text);
        }
    }

    private void flushScroll() {
        if (scrollPending != 0) {
            long dy = Math.round(scrollPending);
            scrollPending = 0;
            dy = Math.max(-10_000, Math.min(10_000, dy));
            if (dy != 0) {
                phone.send.command(APP, "scroll", "0", Long.toString(dy));
            }
        }
    }
}
