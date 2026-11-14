package bastion.companion.ui;

/**
 * Lays rows out top to bottom inside an area: each {@link #take} hands out the next rows, never
 * beyond the area's bottom (what does not fit comes back empty, and drawing helpers draw
 * nothing into an empty rectangle).
 */
public final class Column {
    private final Rect area;
    private int y;

    public Column(Rect area) {
        this.area = area;
        this.y = area.y();
    }

    /** The next {@code height} rows (fewer, or none, at the bottom). */
    public Rect take(int height) {
        int top = Math.min(y, area.bottom());
        int h = Math.max(0, Math.min(height, area.bottom() - top));
        y += Math.max(0, height);
        return new Rect(area.x(), top, area.w(), h);
    }

    /** Skips {@code rows}. */
    public void gap(int rows) {
        y += Math.max(0, rows);
    }

    /** Rows left below what was taken. */
    public int left() {
        return Math.max(0, area.bottom() - y);
    }

    /** Everything below what was taken. */
    public Rect rest() {
        int top = Math.min(y, area.bottom());
        return new Rect(area.x(), top, area.w(), area.bottom() - top);
    }

    /** The bottom {@code height} rows of what is left, taken away from the end. */
    public Rect takeBottom(int height) {
        Rect rest = rest();
        Rect bottom = rest.bottomRows(height);
        bottomLimit = bottom.y();
        return bottom;
    }

    private int bottomLimit = Integer.MAX_VALUE;

    /** What is left between the rows taken from the top and those taken from the bottom. */
    public Rect middle() {
        Rect rest = rest();
        int bottom = Math.min(rest.bottom(), bottomLimit);
        return new Rect(rest.x(), rest.y(), rest.w(), Math.max(0, bottom - rest.y()));
    }
}
