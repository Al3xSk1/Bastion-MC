package bastion.companion.ui;

/**
 * The scroll position of one list: how many units of its content are above the visible part.
 * Always kept inside [0, content − visible].
 */
public final class Scroller {
    private int offset;
    private int content;
    private int visible;

    /** Called every frame with the list's current sizes. */
    public void measure(int contentHeight, int visibleHeight) {
        content = Math.max(0, contentHeight);
        visible = Math.max(0, visibleHeight);
        offset = clamp(offset);
    }

    public int offset() {
        return offset;
    }

    /** Scrolls by {@code units} (positive: further down the list). */
    public void scroll(int units) {
        offset = clamp(offset + units);
    }

    /** Scrolls just enough that content rows top to bottom are in view. */
    public void reveal(int top, int bottom) {
        if (top < offset) {
            offset = clamp(top);
        } else if (bottom > offset + visible) {
            offset = clamp(bottom - visible);
        }
    }

    public void reset() {
        offset = 0;
    }

    /** Whether there is more content than fits. */
    public boolean scrolls() {
        return content > visible;
    }

    /** The part of a scroll bar of {@code length} units the thumb covers: {start, length}. */
    public int[] thumb(int length) {
        if (!scrolls() || length <= 0) {
            return new int[] {0, length};
        }
        int size = Math.max(4, length * visible / content);
        int start = (length - size) * offset / Math.max(1, content - visible);
        return new int[] {start, size};
    }

    private int clamp(int value) {
        return Math.max(0, Math.min(value, Math.max(0, content - visible)));
    }
}
