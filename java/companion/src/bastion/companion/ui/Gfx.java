package bastion.companion.ui;

import bastion.companion.platform.Canvas;
import bastion.companion.theme.Theme;

/**
 * Drawing helpers on the platform's {@link Canvas}, all of which keep to the rectangle they are
 * given: text is shortened to fit (with an ellipsis), never drawn when the rectangle is lower
 * than a line, and centred in the rectangle's height.
 *
 * Text metrics: a line drawn at y occupies rows y to y + lineHeight - 1 (the glyphs and their
 * shadow); a string occupies {@code textWidth} columns from x.
 */
public final class Gfx {
    public static final int LEFT = -1;
    public static final int CENTER = 0;
    public static final int RIGHT = 1;

    /** The extra columns between the letters of a tracked label. */
    private static final int TRACKING = 1;
    private static final String ELLIPSIS = "...";

    private Gfx() {
    }

    /** The y at which a line of text is centred in {@code r}. */
    public static int textY(Canvas c, Rect r) {
        return r.y() + Math.max(0, (r.h() - c.lineHeight()) / 2) + (r.h() > c.lineHeight() ? 1 : 0);
    }

    /** Whether a line of text fits in r's height. */
    public static boolean fitsLine(Canvas c, Rect r) {
        return r.h() >= c.lineHeight() && r.w() > 0;
    }

    /** {@code text} shortened (with an ellipsis) to at most {@code width} columns. */
    public static String fit(Canvas c, String text, int width) {
        if (text == null || text.isEmpty() || width <= 0) {
            return "";
        }
        if (c.textWidth(text) <= width) {
            return text;
        }
        int room = width - c.textWidth(ELLIPSIS);
        if (room <= 0) {
            return "";
        }
        // Binary search on the code point count that fits.
        int total = text.codePointCount(0, text.length());
        int lo = 0;
        int hi = total;
        while (lo < hi) {
            int mid = (lo + hi + 1) / 2;
            String head = text.substring(0, text.offsetByCodePoints(0, mid));
            if (c.textWidth(head) <= room) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        String head = text.substring(0, text.offsetByCodePoints(0, lo)).stripTrailing();
        return head.isEmpty() ? "" : head + ELLIPSIS;
    }

    /**
     * One line of text in {@code r}: shortened to its width, centred in its height, aligned
     * left, centred or right. Draws nothing when r is lower than a line. Returns the width
     * drawn.
     */
    public static int label(Canvas c, String text, Rect r, int argb, int align, boolean shadow) {
        if (!fitsLine(c, r)) {
            return 0;
        }
        String shown = fit(c, text, r.w());
        if (shown.isEmpty()) {
            return 0;
        }
        int width = c.textWidth(shown);
        int x = align == LEFT ? r.x() : align == CENTER ? r.x() + (r.w() - width) / 2 : r.right() - width;
        c.text(shown, x, textY(c, r), argb, shadow);
        return width;
    }

    public static int label(Canvas c, String text, Rect r, int argb) {
        return label(c, text, r, argb, LEFT, false);
    }

    /**
     * The first of {@code variants} that fits {@code width} as tracked capitals, or null: for
     * labels that have a shorter wording rather than an ellipsis.
     */
    public static String capsThatFits(Canvas c, int width, String... variants) {
        for (String variant : variants) {
            if (capsWidth(c, variant) <= width) {
                return variant;
            }
        }
        return null;
    }

    /** The width of {@code text} as tracked capitals ({@link #caps}). */
    public static int capsWidth(Canvas c, String text) {
        String upper = Text.caps(text);
        int width = 0;
        int count = 0;
        for (int i = 0; i < upper.length();) {
            int cp = upper.codePointAt(i);
            int next = i + Character.charCount(cp);
            width += c.textWidth(upper.substring(i, next));
            count++;
            i = next;
        }
        return width + Math.max(0, count - 1) * TRACKING;
    }

    /**
     * A label in tracked capitals (the theme's form for captions and buttons), drawn letter by
     * letter with a column between letters; letters that do not fit are left off, the last
     * shown one replaced by an ellipsis. Returns the width drawn.
     */
    public static int caps(Canvas c, String text, Rect r, int argb, int align) {
        if (!fitsLine(c, r)) {
            return 0;
        }
        String upper = Text.caps(text);
        if (capsWidth(c, upper) > r.w()) {
            // Shorten: fewest letters removed so that letters plus an ellipsis fit.
            int count = upper.codePointCount(0, upper.length());
            String shown = "";
            for (int n = count - 1; n > 0; n--) {
                String candidate = upper.substring(0, upper.offsetByCodePoints(0, n)).stripTrailing() + ELLIPSIS;
                if (capsWidth(c, candidate) <= r.w()) {
                    shown = candidate;
                    break;
                }
            }
            upper = shown;
        }
        if (upper.isEmpty()) {
            return 0;
        }
        int width = capsWidth(c, upper);
        int x = align == LEFT ? r.x() : align == CENTER ? r.x() + (r.w() - width) / 2 : r.right() - width;
        int y = textY(c, r);
        for (int i = 0; i < upper.length();) {
            int cp = upper.codePointAt(i);
            int next = i + Character.charCount(cp);
            String letter = upper.substring(i, next);
            c.text(letter, x, y, argb, false);
            x += c.textWidth(letter) + TRACKING;
            i = next;
        }
        return width;
    }

    public static void hline(Canvas c, int x0, int x1, int y, int argb) {
        if (x1 > x0) {
            c.fill(x0, y, x1, y + 1, argb);
        }
    }

    public static void vline(Canvas c, int x, int y0, int y1, int argb) {
        if (y1 > y0) {
            c.fill(x, y0, x + 1, y1, argb);
        }
    }

    /** A one-unit outline inside r (a plain fill where r is thinner than two units). */
    public static void frame(Canvas c, Rect r, int argb) {
        if (r.empty()) {
            return;
        }
        if (r.w() < 2 || r.h() < 2) {
            c.fill(r.x(), r.y(), r.right(), r.bottom(), argb);
            return;
        }
        c.outline(r.x(), r.y(), r.w(), r.h(), argb);
    }

    /**
     * Two labels in one row, one at each end, never over each other: the right one keeps its
     * width (up to half the row), the left one gets what is left less a gap and is shortened
     * if it must be.
     */
    public static void pair(Canvas c, Rect r, String left, int leftColor, String right, int rightColor) {
        if (!fitsLine(c, r)) {
            return;
        }
        int gap = 6;
        int rw = Math.min(right == null ? 0 : c.textWidth(right), r.w() / 2);
        if (rw > 0) {
            label(c, right, r.rightCols(rw), rightColor, RIGHT, false);
        }
        label(c, left, r.leftOf(rw > 0 ? rw + gap : 0), leftColor, LEFT, false);
    }

    /**
     * A panel in the theme's manner: flat {@code fill}, a hairline {@code line} around it, the
     * two top corners cut by {@code cut} units (the front end's cut-corner frames). Inside r.
     */
    public static void panel(Canvas c, Rect r, int fill, int line, int cut) {
        if (r.empty()) {
            return;
        }
        int k = Math.max(0, Math.min(cut, Math.min(r.w(), r.h()) / 3));
        // Body: full rows below the cut, and the cut rows narrowing towards the top.
        for (int row = 0; row < k; row++) {
            int inset = k - row;
            c.fill(r.x() + inset, r.y() + row, r.right() - inset, r.y() + row + 1, fill);
        }
        c.fill(r.x(), r.y() + k, r.right(), r.bottom(), fill);
        if ((line >>> 24) == 0) {
            return;
        }
        // Outline: the diagonal of each cut as single cells, then the straight edges.
        for (int i = 0; i < k; i++) {
            c.fill(r.x() + k - i, r.y() + i, r.x() + k - i + 1, r.y() + i + 1, line);
            c.fill(r.right() - k + i - 1, r.y() + i, r.right() - k + i, r.y() + i + 1, line);
        }
        hline(c, r.x() + k, r.right() - k, r.y(), line);
        vline(c, r.x(), r.y() + k, r.bottom(), line);
        vline(c, r.right() - 1, r.y() + k, r.bottom(), line);
        hline(c, r.x() + 1, r.right() - 1, r.bottom() - 1, line);
    }

    /** A horizontal bar: {@code track} across r, {@code fill} for the first {@code fraction} of it. */
    public static void bar(Canvas c, Rect r, double fraction, int track, int fill) {
        if (r.empty()) {
            return;
        }
        double f = Double.isNaN(fraction) ? 0.0 : Math.max(0.0, Math.min(1.0, fraction));
        int split = r.x() + (int) Math.round(r.w() * f);
        c.fill(r.x(), r.y(), r.right(), r.bottom(), track);
        if (split > r.x()) {
            c.fill(r.x(), r.y(), split, r.bottom(), fill);
        }
    }

    /**
     * A vertical gradient from {@code top} to {@code bottom} in bands of {@code band} rows
     * (the platform has plain fills only; bands of two or three rows read as smooth).
     */
    public static void gradient(Canvas c, Rect r, int top, int bottom, int band) {
        if (r.empty()) {
            return;
        }
        int step = Math.max(1, band);
        for (int y = r.y(); y < r.bottom(); y += step) {
            int y1 = Math.min(r.bottom(), y + step);
            float t = r.h() <= 1 ? 0.0f : (float) (y - r.y()) / (float) (r.h() - 1);
            c.fill(r.x(), y, r.right(), y1, Theme.mix(top, bottom, t));
        }
    }

    /** Clips to r while {@code body} draws; nothing at all when r is empty. */
    public static void clipped(Canvas c, Rect r, Runnable body) {
        if (r.empty()) {
            return;
        }
        c.clip(r.x(), r.y(), r.right(), r.bottom());
        try {
            body.run();
        } finally {
            c.unclip();
        }
    }
}
