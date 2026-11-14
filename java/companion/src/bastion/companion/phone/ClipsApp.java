package bastion.companion.phone;

import java.util.HashSet;
import java.util.Set;

import bastion.companion.platform.Canvas;
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

/**
 * Clips: keep the last seconds of the game as a video, list what was kept (with thumbnails in
 * frame slots, one per position in the last list), rename and delete. The recording itself is
 * the launcher's, outside the game. Verbs: status, save [seconds], list, delete &lt;id&gt;,
 * rename &lt;id&gt; -- &lt;name&gt;, thumb &lt;id&gt;.
 */
final class ClipsApp extends App {
    private static final String APP = "clips";
    private static final int[] DURATIONS = {15, 30, 60, 120, 300};
    private static final long THUMB_EVERY_NANOS = 250_000_000L;

    /** The clip shown on its own (its id), or null for the list. */
    private String selected;
    private final TextField rename = TextField.plain(60);
    private boolean confirmDelete;
    private int seconds = 30;
    private final Scroller scroll = new Scroller();
    private final Set<String> thumbsAsked = new HashSet<>();
    private long thumbsFor = -1;
    private long lastThumbAsk;

    ClipsApp(Phone phone) {
        super(phone);
        rename.onSubmit(this::doRename);
    }

    @Override
    String id() {
        return APP;
    }

    @Override
    String title() {
        return "Clips";
    }

    @Override
    Icons.Glyph icon() {
        return Icons.CLIP;
    }

    @Override
    boolean available() {
        return phone.store.announced(APP);
    }

    @Override
    String badge() {
        return phone.store.clips().recording ? "rec" : "off";
    }

    @Override
    int badgeTone() {
        return phone.store.clips().recording ? Kit.ACCENT : Kit.NEUTRAL;
    }

    @Override
    void shown() {
        phone.send.command(APP, "list");
    }

    @Override
    void hidden() {
        confirmDelete = false;
    }

    @Override
    boolean back() {
        if (selected != null) {
            selected = null;
            confirmDelete = false;
            return true;
        }
        return false;
    }

    @Override
    boolean scroll(double x, double y, double amount) {
        if (selected != null) {
            return false;
        }
        scroll.scroll((int) Math.round(-amount * 12));
        return true;
    }

    /** The durations offered: those within the buffer, and the whole buffer. */
    static int[] choices(int buffer) {
        int count = 0;
        int[] out = new int[DURATIONS.length + 1];
        for (int d : DURATIONS) {
            if (d < buffer) {
                out[count++] = d;
            }
        }
        if (buffer > 0) {
            out[count++] = buffer;
        }
        return java.util.Arrays.copyOf(out, count);
    }

    private int index(AppStates.Clips s, String id) {
        for (int i = 0; i < s.clips.size(); i++) {
            if (id != null && id.equals(s.clips.get(i).id())) {
                return i;
            }
        }
        return -1;
    }

    @Override
    void render(Kit k, Rect area) {
        AppStates.Clips s = phone.store.clips();
        long serial = phone.store.serial(APP);
        if (serial != thumbsFor) {
            thumbsFor = serial;
            thumbsAsked.clear();
        }
        int at = index(s, selected);
        if (selected != null && at < 0) {
            selected = null;
        }
        if (selected != null) {
            detail(k, area, s, at);
        } else {
            list(k, area, s);
        }
    }

    private void list(Kit k, Rect area, AppStates.Clips s) {
        Canvas c = k.c;
        int line = c.lineHeight();
        Column col = new Column(area);
        Rect status = col.take(line + 2);
        if (s.recording) {
            Icons.draw(c, Icons.DOT, status.x(), status.y() + (status.h() - 7) / 2, 1, Theme.ACCENT);
            Gfx.label(c, "Last " + Text.clock(s.bufferSeconds * 1000L) + " kept", status.rightOf(10), Theme.TEXT_DIM);
        } else {
            Gfx.label(c, "Not recording", status, Theme.TEXT_DIM);
        }
        col.gap(2);
        int[] options = choices(s.bufferSeconds);
        if (s.recording && options.length > 0) {
            if (indexOf(options, seconds) < 0) {
                seconds = options[Math.min(1, options.length - 1)];
            }
            Rect row = col.take(14);
            int chip = Gfx.capsWidth(c, Text.clock(options[options.length - 1] * 1000L)) + 8;
            Rect save = row.leftOf(chip + 2);
            String label = "save last " + seconds + " s";
            if (Gfx.capsWidth(c, label) > save.w() - 6) {
                label = "save " + seconds + " s";
            }
            k.button("cl-save", save, label, Kit.ACCENT,
                    () -> phone.send.command(APP, "save", Integer.toString(seconds)));
            k.button("cl-length", row.rightCols(chip), Text.clock(seconds * 1000L), Kit.NEUTRAL, () -> {
                int i = indexOf(options, seconds);
                seconds = options[(i + 1) % options.length];
            });
        } else {
            k.paragraph(col.take(2 * line).inset(1, 0),
                    "Clips are off. The launcher's config turns them on ([clips]).", Theme.TEXT_FAINT);
        }
        col.gap(4);
        Rect bottom = Rect.EMPTY;
        if (!s.error.isEmpty() || !s.folder.isEmpty()) {
            bottom = col.takeBottom(line + 2);
        }
        Rect list = col.middle();
        if (s.clips.isEmpty()) {
            k.paragraph(list.inset(2, 2), "No clips yet.", Theme.TEXT_FAINT);
        } else {
            int rowH = Math.max(2 * line + 4, 22);
            scroll.measure(s.clips.size() * rowH, list.h());
            String focus = phone.hits.focus();
            long now = k.now;
            k.hits.pushClip(list);
            Gfx.clipped(c, list, () -> {
                for (int i = 0; i < s.clips.size(); i++) {
                    int y = list.y() + i * rowH - scroll.offset();
                    if (y + rowH <= list.y() || y >= list.bottom()) {
                        continue;
                    }
                    AppStates.Clip clip = s.clips.get(i);
                    String id = "cl-clip" + i;
                    if (id.equals(focus)) {
                        scroll.reveal(i * rowH, (i + 1) * rowH);
                    }
                    Rect row = new Rect(list.x(), y, list.w() - 3, rowH);
                    int thumbW = Math.min(32, row.w() / 3);
                    Rect thumb = row.left(thumbW).inset(0, 2).center(thumbW, thumbW * 9 / 16);
                    drawThumb(c, i, thumb);
                    askThumb(clip, i, now);
                    String info = Text.clock(clip.seconds() * 1000L) + (clip.bytes() >= 0 ? " · " + Text.bytes(clip.bytes()) : "");
                    String chosen = clip.id();
                    k.row(id, row.rightOf(thumbW + 2), clip.name().isEmpty() ? "(unnamed)" : clip.name(), info, false,
                            chosen == null ? null : () -> {
                                selected = chosen;
                                confirmDelete = false;
                                rename.set(clip.name());
                            });
                }
            });
            k.hits.popClip();
            k.scrollbar(list, scroll);
        }
        if (!bottom.empty()) {
            if (!s.error.isEmpty()) {
                Gfx.label(c, s.error, bottom, Theme.RED);
            } else {
                Gfx.label(c, "Saved in " + s.folder, bottom, Theme.TEXT_FAINT);
            }
        }
    }

    private static int indexOf(int[] values, int value) {
        for (int i = 0; i < values.length; i++) {
            if (values[i] == value) {
                return i;
            }
        }
        return -1;
    }

    private void drawThumb(Canvas c, int slot, Rect box) {
        if (box.empty()) {
            return;
        }
        Texture thumb = slot < Store.MAX_SLOTS ? phone.store.picture(APP, slot) : null;
        if (thumb != null && thumb.width() > 0 && thumb.height() > 0) {
            Rect fit = SpotifyApp.fitInto(box, thumb.width(), thumb.height());
            c.fill(box.x(), box.y(), box.right(), box.bottom(), Theme.WELL);
            c.image(thumb, fit.x(), fit.y(), fit.w(), fit.h());
        } else {
            c.fill(box.x(), box.y(), box.right(), box.bottom(), Theme.PLATE);
            Icons.drawIn(c, Icons.CLIP, box.inset(1), 1, Theme.LINE_HI);
        }
        Gfx.frame(c, box, Theme.LINE);
    }

    /** Asks for a visible clip's thumbnail once per list, at most four a second. */
    private void askThumb(AppStates.Clip clip, int slot, long now) {
        if (clip.id() == null || slot >= 32 || phone.store.picture(APP, slot) != null || thumbsAsked.contains(clip.id())
                || now - lastThumbAsk < THUMB_EVERY_NANOS) {
            return;
        }
        thumbsAsked.add(clip.id());
        lastThumbAsk = now;
        phone.send.command(APP, "thumb", clip.id());
    }

    private void detail(Kit k, Rect area, AppStates.Clips s, int at) {
        Canvas c = k.c;
        int line = c.lineHeight();
        AppStates.Clip clip = s.clips.get(at);
        Column col = new Column(area);
        int thumbH = Math.min(area.w() * 9 / 16, Math.max(0, area.h() - (5 * line + 14 + 14 + 16)));
        if (thumbH >= 18) {
            Rect box = col.take(thumbH).center(thumbH * 16 / 9, thumbH);
            drawThumb(c, at, box);
            askThumb(clip, at, k.now);
            col.gap(3);
        }
        Gfx.label(c, clip.name().isEmpty() ? "(unnamed)" : clip.name(), col.take(line), Theme.TEXT);
        String info = Text.clock(clip.seconds() * 1000L) + (clip.bytes() >= 0 ? " · " + Text.bytes(clip.bytes()) : "");
        Gfx.label(c, info, col.take(line), Theme.TEXT_DIM);
        if (!clip.created().isEmpty()) {
            Gfx.label(c, clip.created(), col.take(line), Theme.TEXT_FAINT);
        }
        col.gap(4);
        k.caption(col.take(line + 1), "rename");
        Rect row = col.take(14);
        k.field("cl-name", row.leftOf(16), rename, "New name");
        k.iconButton("cl-rename", row.rightCols(14), Icons.CHECK, false, this::doRename);
        col.gap(6);
        String id = clip.id();
        k.button("cl-delete", col.take(14), confirmDelete ? "really delete?" : "delete", Kit.DANGER, () -> {
            if (!confirmDelete) {
                confirmDelete = true;
            } else {
                confirmDelete = false;
                phone.send.command(APP, "delete", id);
                selected = null;
            }
        });
    }

    private void doRename() {
        String name = rename.text().strip();
        if (selected != null && !name.isEmpty()) {
            phone.send.withText(APP, "rename", name, selected);
            phone.hits.focus(null);
        }
    }
}
