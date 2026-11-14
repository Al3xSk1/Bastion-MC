package bastion.companion.phone;

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
import bastion.companion.ui.Text;
import bastion.companion.ui.TextField;
import bastion.companion.ui.Clock;

/**
 * Spotify: what is playing (with the album art the launcher sends in slot 0), the controls,
 * search and devices. Verbs (the contract's vocabulary): play, pause, next, previous,
 * seek &lt;ms&gt;, volume &lt;0-100&gt;, shuffle on|off, repeat off|track|context,
 * open &lt;uri&gt;, queue &lt;uri&gt;, search -- &lt;text&gt;, devices, transfer &lt;id&gt;, art.
 */
final class SpotifyApp extends App {
    private static final String APP = "spotify";
    private static final String[] TABS = {"now", "search", "devices"};
    private static final Icons.Glyph[] TAB_GLYPHS = {Icons.MUSIC_SMALL, Icons.SEARCH, Icons.DEVICE};
    private static final long VOLUME_EVERY_NANOS = 250_000_000L;
    private static final long PENDING_NANOS = 2_000_000_000L;

    private int tab;
    private final TextField query = TextField.plain(120);
    private final Scroller resultScroll = new Scroller();
    private final Scroller deviceScroll = new Scroller();
    private String searchedFor;
    // While dragging a slider: the value under the pointer (-1: not dragging).
    private double seekDrag = -1;
    private double volumeDrag = -1;
    private long volumeSentNanos;
    private int volumeSent = -1;
    // What was sent last and is shown until the launcher's next state (or a while).
    private int pendingVolume = -1;
    private long pendingVolumeSerial;
    private long pendingVolumeNanos;
    private long pendingSeek = -1;
    private long pendingSeekSerial;
    private long pendingSeekNanos;
    private long artAskedFor = -1;

    SpotifyApp(Phone phone) {
        super(phone);
        query.onSubmit(this::search);
    }

    @Override
    String id() {
        return APP;
    }

    @Override
    String title() {
        return "Spotify";
    }

    @Override
    Icons.Glyph icon() {
        return Icons.MUSIC;
    }

    @Override
    boolean available() {
        return phone.store.announced(APP);
    }

    @Override
    String badge() {
        switch (phone.store.spotify().source) {
            case "mpris":
                return "app";
            case "web":
                return "web";
            default:
                return "off";
        }
    }

    @Override
    int badgeTone() {
        return phone.store.spotify().connected() ? Kit.ACCENT : Kit.NEUTRAL;
    }

    @Override
    void shown() {
        askForArt();
        if (tab == 1) {
            phone.hits.focus("sp-query");
        }
    }

    private void askForArt() {
        AppStates.Spotify s = phone.store.spotify();
        long serial = phone.store.serial(APP);
        if (s.connected() && !s.track.isEmpty() && phone.store.picture(APP, 0) == null && artAskedFor != serial) {
            artAskedFor = serial;
            phone.send.command(APP, "art");
        }
    }

    private void choose(int index) {
        tab = index;
        if (index == 1) {
            phone.hits.focus("sp-query");
        } else if (index == 2) {
            phone.send.command(APP, "devices");
        }
    }

    private void search() {
        String text = query.text().strip();
        if (!text.isEmpty()) {
            searchedFor = text;
            resultScroll.reset();
            phone.send.withText(APP, "search", text);
        }
    }

    @Override
    boolean back() {
        if (tab != 0) {
            tab = 0;
            return true;
        }
        return false;
    }

    @Override
    boolean key(int key, int modifiers) {
        if (key == Keys.SPACE && tab == 0) {
            playPause();
            return true;
        }
        return false;
    }

    private void playPause() {
        phone.send.command(APP, phone.store.spotify().playing ? "pause" : "play");
    }

    @Override
    boolean scroll(double x, double y, double amount) {
        Scroller target = tab == 1 ? resultScroll : tab == 2 ? deviceScroll : null;
        if (target == null) {
            return false;
        }
        target.scroll((int) Math.round(-amount * 12));
        return true;
    }

    @Override
    void render(Kit k, Rect area) {
        askForArt();
        AppStates.Spotify s = phone.store.spotify();
        Column col = new Column(area);
        k.tabs("sp-tab", col.take(12), TABS, TAB_GLYPHS, tab, this::choose);
        col.gap(3);
        switch (tab) {
            case 1:
                searchTab(k, col.rest(), s);
                break;
            case 2:
                devicesTab(k, col.rest(), s);
                break;
            default:
                nowTab(k, col.rest(), s);
        }
    }

    // ------------------------------------------------------------------ now playing

    private void nowTab(Kit k, Rect area, AppStates.Spotify s) {
        Canvas c = k.c;
        int line = c.lineHeight();
        if (!s.connected()) {
            Column col = new Column(area);
            col.gap(6);
            Icons.drawIn(c, Icons.MUSIC, col.take(24), 2, Theme.TEXT_OFF);
            col.gap(6);
            String why = !s.error.isEmpty() ? s.error
                    : "Spotify is not playing on this computer. Start the Spotify app, or sign in to the Web API with "
                            + "bastion-mc spotify login.";
            k.paragraph(col.rest().inset(2, 0), why, s.error.isEmpty() ? Theme.TEXT_DIM : Theme.AMBER);
            return;
        }
        long now = k.now;
        long serial = phone.store.serial(APP);
        int nonArt = 3 * line + 14 + 16 + 9 + 14 + (s.error.isEmpty() ? 0 : line);
        int art = Math.min(Math.min(area.w() - 16, 128), area.h() - nonArt);
        Column col = new Column(area);
        if (art >= 24) {
            drawArt(k, col.take(art).center(art, art));
            col.gap(4);
        }
        Gfx.label(c, s.track.isEmpty() ? "Nothing playing" : s.track, col.take(line), Theme.TEXT, Gfx.CENTER, false);
        Gfx.label(c, s.artist, col.take(line), Theme.TEXT_DIM, Gfx.CENTER, false);
        Gfx.label(c, s.album, col.take(line), Theme.TEXT_FAINT, Gfx.CENTER, false);
        col.gap(2);

        // Where the song is; drag to seek (sent when let go).
        long duration = s.durationMs;
        long position = s.positionAt(now);
        if (pendingSeek >= 0 && pendingSeekSerial == serial && now - pendingSeekNanos < PENDING_NANOS) {
            position = pendingSeek;
        }
        if (seekDrag >= 0) {
            position = (long) (seekDrag * duration);
        }
        Rect seek = col.take(5);
        double fraction = duration > 0 ? (double) position / duration : 0.0;
        Rect track = seek.center(seek.w(), 2);
        Gfx.bar(c, track, fraction, Theme.LINE_HI, Theme.ACCENT);
        if (duration > 0 && !seek.empty()) {
            int knob = track.x() + (int) Math.round((track.w() - 1) * Math.max(0, Math.min(1, fraction)));
            boolean hot = k.hot("sp-seek", seek);
            c.fill(Math.max(seek.x(), knob - 1), seek.y(), Math.min(seek.right(), knob + 2), seek.bottom(),
                    hot ? Theme.TEXT : Theme.ACCENT);
            k.hits.slider("sp-seek", seek, (x, y, released) -> {
                double t = Math.max(0, Math.min(1, (x - track.x()) / Math.max(1.0, track.w() - 1)));
                if (released) {
                    long ms = (long) (t * duration);
                    seekDrag = -1;
                    pendingSeek = ms;
                    pendingSeekSerial = phone.store.serial(APP);
                    pendingSeekNanos = Clock.now();
                    phone.send.command(APP, "seek", Long.toString(ms));
                } else {
                    seekDrag = t;
                }
            });
        }
        Gfx.pair(c, col.take(line), Text.clock(position), Theme.TEXT_FAINT, duration > 0 ? Text.clock(duration) : "",
                Theme.TEXT_FAINT);
        col.gap(2);

        // The controls.
        Rect row = col.take(16);
        k.iconButton("sp-shuffle", row.column(0, 5, 1), Icons.SHUFFLE, s.shuffle,
                () -> phone.send.command(APP, "shuffle", s.shuffle ? "off" : "on"));
        k.iconButton("sp-previous", row.column(1, 5, 1), Icons.PREVIOUS, false,
                () -> phone.send.command(APP, "previous"));
        k.iconButton("sp-play", row.column(2, 5, 1), s.playing ? Icons.PAUSE : Icons.PLAY, true, this::playPause);
        k.iconButton("sp-next", row.column(3, 5, 1), Icons.NEXT, false, () -> phone.send.command(APP, "next"));
        String nextRepeat = "off".equals(s.repeat) ? "context" : "context".equals(s.repeat) ? "track" : "off";
        k.iconButton("sp-repeat", row.column(4, 5, 1), "track".equals(s.repeat) ? Icons.REPEAT_ONE : Icons.REPEAT,
                !"off".equals(s.repeat), () -> phone.send.command(APP, "repeat", nextRepeat));
        col.gap(3);

        // Volume.
        Rect vol = col.take(9);
        int shown = s.volume;
        if (pendingVolume >= 0 && pendingVolumeSerial == serial && now - pendingVolumeNanos < PENDING_NANOS) {
            shown = pendingVolume;
        }
        if (volumeDrag >= 0) {
            shown = (int) Math.round(volumeDrag * 100);
        }
        Icons.draw(c, Icons.SPEAKER, vol.x(), vol.y() + (vol.h() - 7) / 2, 1, shown < 0 ? Theme.TEXT_OFF : Theme.TEXT_DIM);
        String value = shown < 0 ? "" : Integer.toString(shown);
        int valueWidth = c.textWidth("100");
        Gfx.label(c, value, vol.rightCols(valueWidth), Theme.TEXT_FAINT, Gfx.RIGHT, false);
        Rect slider = vol.rightOf(10).leftOf(valueWidth + 3);
        if (shown >= 0) {
            k.slider("sp-volume", slider, shown / 100.0, this::volume);
        } else {
            Gfx.hline(c, slider.x(), slider.right(), slider.centerY(), Theme.LINE);
        }
        if (!s.error.isEmpty()) {
            col.gap(2);
            Gfx.label(c, s.error, col.take(line), Theme.AMBER);
        }
    }

    private void volume(double value, boolean released) {
        int level = (int) Math.round(Math.max(0, Math.min(1, value)) * 100);
        long now = Clock.now();
        volumeDrag = released ? -1 : value;
        if (level != volumeSent && (released || now - volumeSentNanos >= VOLUME_EVERY_NANOS)) {
            volumeSent = level;
            volumeSentNanos = now;
            pendingVolume = level;
            pendingVolumeSerial = phone.store.serial(APP);
            pendingVolumeNanos = now;
            phone.send.command(APP, "volume", Integer.toString(level));
        }
    }

    private void drawArt(Kit k, Rect box) {
        Canvas c = k.c;
        Texture art = phone.store.picture(APP, 0);
        if (art != null && art.width() > 0 && art.height() > 0) {
            Rect fit = fitInto(box, art.width(), art.height());
            c.image(art, fit.x(), fit.y(), fit.w(), fit.h());
        } else {
            c.fill(box.x(), box.y(), box.right(), box.bottom(), Theme.PLATE);
            Icons.drawIn(c, Icons.MUSIC, box.inset(box.w() / 4), 4, Theme.LINE_HI);
        }
        Gfx.frame(c, box, Theme.LINE);
    }

    /** The largest rectangle of the picture's proportions centred in {@code box}. */
    static Rect fitInto(Rect box, int width, int height) {
        if (width <= 0 || height <= 0 || box.empty()) {
            return Rect.EMPTY;
        }
        double scale = Math.min((double) box.w() / width, (double) box.h() / height);
        int w = Math.max(1, (int) Math.floor(width * scale));
        int h = Math.max(1, (int) Math.floor(height * scale));
        return box.center(w, h);
    }

    // ------------------------------------------------------------------ search

    private void searchTab(Kit k, Rect area, AppStates.Spotify s) {
        Canvas c = k.c;
        int line = c.lineHeight();
        Column col = new Column(area);
        Rect bar = col.take(14);
        k.field("sp-query", bar.leftOf(16), query, "Search Spotify");
        k.iconButton("sp-go", bar.rightCols(14), Icons.SEARCH, false, this::search);
        col.gap(3);
        Rect list = col.rest();
        if (!s.connected()) {
            k.paragraph(list, "Spotify is not connected.", Theme.TEXT_DIM);
            return;
        }
        if (s.results.isEmpty()) {
            String why = searchedFor == null ? "Type, then press Enter." : !s.error.isEmpty() ? s.error : "No results yet.";
            k.paragraph(list.inset(2, 2), why, s.error.isEmpty() ? Theme.TEXT_FAINT : Theme.AMBER);
            return;
        }
        int rowH = 2 * line + 4;
        resultScroll.measure(s.results.size() * rowH, list.h());
        String focus = phone.hits.focus();
        k.hits.pushClip(list);
        Gfx.clipped(c, list, () -> {
            for (int i = 0; i < s.results.size(); i++) {
                int y = list.y() + i * rowH - resultScroll.offset();
                if (y + rowH <= list.y() || y >= list.bottom()) {
                    continue;
                }
                AppStates.Result result = s.results.get(i);
                Rect row = new Rect(list.x(), y, list.w() - 3, rowH);
                boolean queueable = AppStates.isQueueable(result.uri());
                Rect main = queueable ? row.leftOf(15) : row;
                String id = "sp-result" + i;
                if (id.equals(focus)) {
                    resultScroll.reveal(i * rowH, (i + 1) * rowH);
                }
                k.row(id, main, result.title(), result.subtitle(), false,
                        result.uri() == null ? null : () -> phone.send.command(APP, "open", result.uri()));
                if (queueable) {
                    Rect plus = row.rightCols(14).center(12, 12);
                    k.iconButton("sp-queue" + i, plus, Icons.PLUS, false,
                            () -> phone.send.command(APP, "queue", result.uri()));
                }
            }
        });
        k.hits.popClip();
        k.scrollbar(list, resultScroll);
    }

    // ------------------------------------------------------------------ devices

    private void devicesTab(Kit k, Rect area, AppStates.Spotify s) {
        Canvas c = k.c;
        int line = c.lineHeight();
        Column col = new Column(area);
        Rect top = col.take(12);
        k.caption(top.leftOf(16), "play on");
        k.iconButton("sp-refresh", top.rightCols(12), Icons.RELOAD, false, () -> phone.send.command(APP, "devices"));
        col.gap(2);
        Rect list = col.rest();
        if (s.devices.isEmpty()) {
            k.paragraph(list.inset(2, 2), s.connected() ? "No devices reported." : "Spotify is not connected.",
                    Theme.TEXT_FAINT);
            return;
        }
        int rowH = 2 * line + 4;
        deviceScroll.measure(s.devices.size() * rowH, list.h());
        String focus = phone.hits.focus();
        k.hits.pushClip(list);
        Gfx.clipped(c, list, () -> {
            for (int i = 0; i < s.devices.size(); i++) {
                int y = list.y() + i * rowH - deviceScroll.offset();
                if (y + rowH <= list.y() || y >= list.bottom()) {
                    continue;
                }
                AppStates.Device device = s.devices.get(i);
                String id = "sp-device" + i;
                if (id.equals(focus)) {
                    deviceScroll.reveal(i * rowH, (i + 1) * rowH);
                }
                Rect row = new Rect(list.x(), y, list.w() - 3, rowH);
                k.row(id, row, device.name().isEmpty() ? "(unnamed)" : device.name(),
                        device.active() ? "playing here" : "", device.active(),
                        device.id() == null || device.active() ? null
                                : () -> phone.send.command(APP, "transfer", device.id()));
            }
        });
        k.hits.popClip();
        k.scrollbar(list, deviceScroll);
    }
}
