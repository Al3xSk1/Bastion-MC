package bastion.companion.ui;

/**
 * A rectangle in GUI units: (x, y) its top-left corner, w × h its size. Layout is done by
 * cutting rectangles out of rectangles, so that whatever is placed inside a panel is inside it
 * by construction; nothing here ever grows a rectangle beyond the one it was cut from.
 */
public record Rect(int x, int y, int w, int h) {
    public static final Rect EMPTY = new Rect(0, 0, 0, 0);

    public int right() {
        return x + w;
    }

    public int bottom() {
        return y + h;
    }

    public int centerX() {
        return x + w / 2;
    }

    public int centerY() {
        return y + h / 2;
    }

    public boolean empty() {
        return w <= 0 || h <= 0;
    }

    public boolean contains(double px, double py) {
        return !empty() && px >= x && py >= y && px < x + w && py < y + h;
    }

    /** Whether this rectangle lies entirely inside {@code outer} (an empty one lies anywhere). */
    public boolean within(Rect outer) {
        return empty() || (x >= outer.x && y >= outer.y && right() <= outer.right() && bottom() <= outer.bottom());
    }

    /** Smaller by {@code d} on every side (never negative in size). */
    public Rect inset(int d) {
        return inset(d, d);
    }

    public Rect inset(int dx, int dy) {
        int nw = Math.max(0, w - 2 * dx);
        int nh = Math.max(0, h - 2 * dy);
        return new Rect(x + Math.min(dx, w / 2), y + Math.min(dy, h / 2), nw, nh);
    }

    /** The top {@code height} rows (at most all of it). */
    public Rect top(int height) {
        return new Rect(x, y, w, clamp(height, h));
    }

    /** Everything below the top {@code height} rows. */
    public Rect belowTop(int height) {
        int cut = clamp(height, h);
        return new Rect(x, y + cut, w, h - cut);
    }

    /** The bottom {@code height} rows. */
    public Rect bottomRows(int height) {
        int cut = clamp(height, h);
        return new Rect(x, y + h - cut, w, cut);
    }

    /** Everything above the bottom {@code height} rows. */
    public Rect aboveBottom(int height) {
        return new Rect(x, y, w, h - clamp(height, h));
    }

    /** The left {@code width} columns. */
    public Rect left(int width) {
        return new Rect(x, y, clamp(width, w), h);
    }

    /** Everything right of the left {@code width} columns. */
    public Rect rightOf(int width) {
        int cut = clamp(width, w);
        return new Rect(x + cut, y, w - cut, h);
    }

    /** The right {@code width} columns. */
    public Rect rightCols(int width) {
        int cut = clamp(width, w);
        return new Rect(x + w - cut, y, cut, h);
    }

    /** Everything left of the right {@code width} columns. */
    public Rect leftOf(int width) {
        return new Rect(x, y, w - clamp(width, w), h);
    }

    /** A rectangle of at most w × h centred in this one. */
    public Rect center(int width, int height) {
        int cw = clamp(width, w);
        int ch = clamp(height, h);
        return new Rect(x + (w - cw) / 2, y + (h - ch) / 2, cw, ch);
    }

    /** Column {@code i} of {@code n} equal columns with {@code gap} between them. */
    public Rect column(int i, int n, int gap) {
        if (n <= 0) {
            return EMPTY;
        }
        int total = w - gap * (n - 1);
        int x0 = x + (total * i) / n + gap * i;
        int x1 = x + (total * (i + 1)) / n + gap * i;
        return new Rect(x0, y, Math.max(0, x1 - x0), h);
    }

    /** The overlap of the two (possibly empty). */
    public Rect intersect(Rect o) {
        int x0 = Math.max(x, o.x);
        int y0 = Math.max(y, o.y);
        int x1 = Math.min(right(), o.right());
        int y1 = Math.min(bottom(), o.bottom());
        return x1 <= x0 || y1 <= y0 ? new Rect(x0, y0, 0, 0) : new Rect(x0, y0, x1 - x0, y1 - y0);
    }

    public Rect moved(int dx, int dy) {
        return new Rect(x + dx, y + dy, w, h);
    }

    private static int clamp(int value, int max) {
        return Math.max(0, Math.min(value, max));
    }
}
