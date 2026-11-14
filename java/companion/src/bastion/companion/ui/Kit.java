package bastion.companion.ui;

import java.util.function.IntConsumer;

import bastion.companion.platform.Canvas;
import bastion.companion.theme.Theme;

/**
 * The controls the phone and the home screen are built from, for one frame: each draws itself
 * inside the rectangle it is given and registers that rectangle with {@link Hits}. One look for
 * all of them — flat surfaces, hairline outlines, tracked capitals, the accent for what has the
 * pointer or the keyboard focus.
 */
public final class Kit {
    /** What a control is saying, by its colour. */
    public static final int NEUTRAL = 0;
    public static final int ACCENT = 1;
    public static final int CAUTION = 2;
    public static final int DANGER = 3;

    public final Canvas c;
    public final Hits hits;
    public final int mouseX;
    public final int mouseY;
    public final long now;
    private final Runnable clickSound;

    public Kit(Canvas canvas, Hits hits, int mouseX, int mouseY, long nowNanos, Runnable clickSound) {
        this.c = canvas;
        this.hits = hits;
        this.mouseX = mouseX;
        this.mouseY = mouseY;
        this.now = nowNanos;
        this.clickSound = clickSound == null ? () -> {
        } : clickSound;
    }

    public static int toneColor(int tone) {
        switch (tone) {
            case ACCENT:
                return Theme.ACCENT;
            case CAUTION:
                return Theme.AMBER;
            case DANGER:
                return Theme.RED;
            default:
                return Theme.TEXT;
        }
    }

    public boolean hover(Rect r) {
        return r.contains(mouseX, mouseY);
    }

    /** Hovered or focused: the control the player is about to use. */
    public boolean hot(String id, Rect r) {
        return hover(r) || hits.focused(id);
    }

    private Runnable clicking(Runnable action) {
        return () -> {
            clickSound.run();
            action.run();
        };
    }

    // ------------------------------------------------------------------ buttons

    /**
     * A button with a label in tracked capitals. {@code action} null: shown, not usable
     * (faint, not registered).
     */
    public void button(String id, Rect r, String label, int tone, Runnable action) {
        if (r.empty()) {
            return;
        }
        boolean enabled = action != null;
        boolean hot = enabled && hot(id, r);
        int color = enabled ? toneColor(tone) : Theme.TEXT_OFF;
        c.fill(r.x(), r.y(), r.right(), r.bottom(), hot ? Theme.PLATE_HI : Theme.PLATE);
        Gfx.frame(c, r, hot ? Theme.ACCENT : enabled && tone != NEUTRAL ? Theme.alpha(color, 0.6f) : Theme.LINE_HI);
        Gfx.caps(c, label, r.inset(3, 1), color, Gfx.CENTER);
        if (enabled) {
            hits.button(id, r, clicking(action));
        }
    }

    /** A square button showing a pictogram. {@code on}: drawn in the accent (a toggle that is on). */
    public void iconButton(String id, Rect r, Icons.Glyph glyph, boolean on, Runnable action) {
        if (r.empty()) {
            return;
        }
        boolean enabled = action != null;
        boolean hot = enabled && hot(id, r);
        if (hot) {
            c.fill(r.x(), r.y(), r.right(), r.bottom(), Theme.PLATE_HI);
            Gfx.frame(c, r, Theme.ACCENT);
        }
        int color = !enabled ? Theme.TEXT_OFF : on ? Theme.ACCENT : hot ? Theme.TEXT : Theme.TEXT_DIM;
        Icons.drawIn(c, glyph, r.inset(2), 2, color);
        if (enabled) {
            hits.button(id, r, clicking(action));
        }
    }

    /**
     * A row of a list: a title and, below it if there is room, a subtitle. {@code selected}
     * gets the accent bar on the left. Returns nothing; the row is the rectangle.
     */
    public void row(String id, Rect r, String title, String subtitle, boolean selected, Runnable action) {
        if (r.empty()) {
            return;
        }
        boolean hot = action != null && hot(id, r);
        if (hot) {
            c.fill(r.x(), r.y(), r.right(), r.bottom(), Theme.PLATE_HI);
        }
        if (selected) {
            c.fill(r.x(), r.y() + 1, r.x() + 2, r.bottom() - 1, Theme.ACCENT);
        }
        Gfx.hline(c, r.x(), r.right(), r.bottom() - 1, Theme.LINE);
        Rect text = r.inset(4, 1).aboveBottom(1);
        int line = c.lineHeight();
        boolean two = subtitle != null && !subtitle.isEmpty() && text.h() >= 2 * line;
        if (two) {
            int top = text.y() + (text.h() - 2 * line) / 2;
            Gfx.label(c, title, new Rect(text.x(), top, text.w(), line), selected ? Theme.ACCENT : Theme.TEXT);
            Gfx.label(c, subtitle, new Rect(text.x(), top + line, text.w(), line), Theme.TEXT_FAINT);
        } else {
            Gfx.label(c, title, text, selected ? Theme.ACCENT : Theme.TEXT);
        }
        if (hot && hits.focused(id)) {
            Gfx.frame(c, r, Theme.alpha(Theme.ACCENT, 0.7f));
        }
        if (action != null) {
            hits.button(id, r, clicking(action));
        }
    }

    /** Equal tabs across r; the selected one underlined in the accent. */
    public void tabs(String idPrefix, Rect r, String[] labels, int selected, IntConsumer choose) {
        tabs(idPrefix, r, labels, null, selected, choose);
    }

    /**
     * Equal tabs with a pictogram each: pictogram and label where both fit, the pictogram alone
     * where they do not (never a shortened label).
     */
    public void tabs(String idPrefix, Rect r, String[] labels, Icons.Glyph[] glyphs, int selected, IntConsumer choose) {
        if (r.empty() || labels.length == 0) {
            return;
        }
        if (glyphs != null) {
            Gfx.hline(c, r.x(), r.right(), r.bottom() - 1, Theme.LINE);
            // The selected tab is as wide as its pictogram and label need, where the others
            // keep room for their pictograms; they share the rest.
            int n = labels.length;
            int sel = Math.max(0, Math.min(n - 1, selected));
            int wanted = glyphs[sel].width() + 3 + Gfx.capsWidth(c, labels[sel]) + 10;
            int share = r.w() / n;
            int selW = Math.max(share, Math.min(wanted, r.w() - (n - 1) * 16));
            int restW = n > 1 ? (r.w() - selW) / (n - 1) : 0;
            int x = r.x();
            for (int i = 0; i < labels.length; i++) {
                int tw = i == sel ? selW : (i == n - 1 ? r.right() - x : restW);
                if (i == n - 1) {
                    tw = r.right() - x;
                }
                Rect tab = new Rect(x, r.y(), Math.max(0, tw), r.h());
                x += tw;
                String id = idPrefix + i;
                boolean hot = hot(id, tab);
                boolean on = i == selected;
                if (hot) {
                    c.fill(tab.x(), tab.y(), tab.right(), tab.bottom() - 1, Theme.PLATE_HI);
                }
                int color = on ? Theme.ACCENT : hot ? Theme.TEXT : Theme.TEXT_DIM;
                Rect inner = tab.inset(2, 0).aboveBottom(2);
                Icons.Glyph glyph = glyphs[i];
                int labelWidth = Gfx.capsWidth(c, labels[i]);
                int both = glyph.width() + 3 + labelWidth;
                if (on && both <= inner.w() && Gfx.fitsLine(c, inner)) {
                    int gx = inner.x() + (inner.w() - both) / 2;
                    Icons.draw(c, glyph, gx, inner.y() + (inner.h() - glyph.height()) / 2, 1, color);
                    Gfx.caps(c, labels[i], new Rect(gx + glyph.width() + 3, inner.y(), labelWidth, inner.h()), color,
                            Gfx.LEFT);
                } else {
                    Icons.drawIn(c, glyph, inner, 1, color);
                }
                if (on) {
                    c.fill(tab.x() + 2, tab.bottom() - 2, tab.right() - 2, tab.bottom(), Theme.ACCENT);
                }
                int index = i;
                hits.button(id, tab, clicking(() -> choose.accept(index)));
            }
            return;
        }
        Gfx.hline(c, r.x(), r.right(), r.bottom() - 1, Theme.LINE);
        for (int i = 0; i < labels.length; i++) {
            Rect tab = r.column(i, labels.length, 0);
            String id = idPrefix + i;
            boolean hot = hot(id, tab);
            boolean on = i == selected;
            if (hot) {
                c.fill(tab.x(), tab.y(), tab.right(), tab.bottom() - 1, Theme.PLATE_HI);
            }
            Gfx.caps(c, labels[i], tab.inset(2, 0).aboveBottom(2), on ? Theme.ACCENT : hot ? Theme.TEXT : Theme.TEXT_DIM,
                    Gfx.CENTER);
            if (on) {
                c.fill(tab.x() + 2, tab.bottom() - 2, tab.right() - 2, tab.bottom(), Theme.ACCENT);
            }
            int index = i;
            hits.button(id, tab, clicking(() -> choose.accept(index)));
        }
    }

    /** A text field; clicking it gives it the focus. */
    public void field(String id, Rect r, TextField field, String placeholder) {
        if (r.empty()) {
            return;
        }
        field.render(c, r, hits.focused(id), now, placeholder);
        hits.field(id, r, field);
    }

    /**
     * A horizontal slider: the track across r, the thumb at {@code value} (0 to 1).
     * {@code change} gets each new value while dragging and once more on release
     * ({@code released} true) — the caller decides what to send when.
     */
    public void slider(String id, Rect r, double value, Slide change) {
        if (r.empty()) {
            return;
        }
        boolean hot = hot(id, r);
        int mid = r.centerY();
        double v = Double.isNaN(value) ? 0.0 : Math.max(0.0, Math.min(1.0, value));
        int x = r.x() + 2 + (int) Math.round((r.w() - 5) * v);
        c.fill(r.x() + 1, mid, r.right() - 1, mid + 1, Theme.LINE_HI);
        c.fill(r.x() + 1, mid, x, mid + 1, Theme.ACCENT);
        int top = Math.max(r.y(), mid - 3);
        int bottom = Math.min(r.bottom(), mid + 4);
        c.fill(x - 1, top, x + 2, bottom, hot ? Theme.TEXT : Theme.ACCENT);
        hits.slider(id, r, (px, py, released) -> {
            double t = (px - (r.x() + 2)) / Math.max(1.0, r.w() - 5);
            change.to(Math.max(0.0, Math.min(1.0, t)), released);
        });
    }

    /** What a slider reports. */
    public interface Slide {
        void to(double value, boolean released);
    }

    /** A caption: faint tracked capitals. */
    public void caption(Rect r, String text) {
        Gfx.caps(c, text, r, Theme.TEXT_FAINT, Gfx.LEFT);
    }

    /** A line of ordinary text in {@code color}. */
    public void text(Rect r, String text, int color) {
        Gfx.label(c, text, r, color);
    }

    /**
     * Text wrapped into lines that fit r's width (at spaces where it can, anywhere where a
     * word is too long), as many lines as fit r's height; the last shown line ends in an
     * ellipsis when text is left over. Returns the height used.
     */
    public int paragraph(Rect r, String text, int color) {
        int line = c.lineHeight();
        int lines = r.h() / line;
        if (lines <= 0 || r.w() <= 0 || text == null || text.isEmpty()) {
            return 0;
        }
        java.util.List<String> out = wrap(c, text, r.w(), lines);
        for (int i = 0; i < out.size(); i++) {
            c.text(out.get(i), r.x(), r.y() + i * line, color, false);
        }
        return out.size() * line;
    }

    /** {@code text} broken into at most {@code maxLines} lines of at most {@code width}. */
    public static java.util.List<String> wrap(Canvas c, String text, int width, int maxLines) {
        java.util.List<String> lines = new java.util.ArrayList<>();
        String rest = text.strip();
        while (!rest.isEmpty() && lines.size() < maxLines) {
            if (c.textWidth(rest) <= width) {
                lines.add(rest);
                rest = "";
                break;
            }
            // The longest prefix that fits, preferably ending at a space.
            int fit = 0;
            int lastSpace = -1;
            for (int i = 0; i < rest.length();) {
                int next = i + Character.charCount(rest.codePointAt(i));
                if (c.textWidth(rest.substring(0, next)) > width) {
                    break;
                }
                if (rest.charAt(i) == ' ') {
                    lastSpace = i;
                }
                fit = next;
                i = next;
            }
            if (fit == 0) {
                break;
            }
            int cut = lastSpace > 0 ? lastSpace : fit;
            lines.add(rest.substring(0, cut).stripTrailing());
            rest = rest.substring(cut).strip();
        }
        if (!rest.isEmpty() && !lines.isEmpty()) {
            int last = lines.size() - 1;
            lines.set(last, Gfx.fit(c, lines.get(last) + " " + rest, width));
        }
        return lines;
    }

    /** A thin vertical scroll bar at the right edge of r for {@code scroller}. */
    public void scrollbar(Rect r, Scroller scroller) {
        if (!scroller.scrolls() || r.h() < 6) {
            return;
        }
        int[] thumb = scroller.thumb(r.h());
        c.fill(r.right() - 2, r.y(), r.right() - 1, r.bottom(), Theme.LINE);
        c.fill(r.right() - 2, r.y() + thumb[0], r.right(), r.y() + thumb[0] + thumb[1], Theme.LINE_HI);
    }
}
