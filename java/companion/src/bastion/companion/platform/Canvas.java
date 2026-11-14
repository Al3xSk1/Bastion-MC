package bastion.companion.platform;

/**
 * Drawing in one frame: what an {@link Overlay} or a {@link HudLayer} is handed while the game
 * renders. Valid only during that call, on the render thread.
 *
 * Adapters implement the primitives (the abstract methods); the rest is built on them here,
 * the same on every version. Things are drawn in the order they are called, later on top.
 * Coordinates are GUI units, colours {@code 0xAARRGGBB}.
 */
public abstract class Canvas {
    /** The width of the drawing area (the game's scaled window width), in GUI units. */
    public abstract int width();

    /** The height of the drawing area, in GUI units. */
    public abstract int height();

    /** Fills the rectangle from (x0, y0) inclusive to (x1, y1) exclusive. */
    public abstract void fill(int x0, int y0, int x1, int y1, int argb);

    /**
     * Draws one line of text with its top-left corner at (x, y), in the game's font, exactly
     * as given: a {@code §} is a character like any other, never a formatting code (a title
     * containing {@code §k} does not turn into noise). Control characters are drawn as
     * spaces; nothing is wrapped or cut.
     */
    public abstract void text(String text, int x, int y, int argb, boolean shadow);

    /** How wide {@link #text} would draw {@code text} (literally, as there). */
    public abstract int textWidth(String text);

    /** The height of a line of text (the game's font: 9). */
    public abstract int lineHeight();

    /** Draws all of {@code texture}, stretched to the rectangle at (x, y) of w by h. */
    public abstract void image(Texture texture, int x, int y, int w, int h);

    /**
     * Restricts drawing to the rectangle (x0, y0)–(x1, y1) until the matching
     * {@link #unclip()}. Clips nest (the visible part is the intersection). The rectangle is in
     * GUI units of the screen, unaffected by {@link #translate} and {@link #scale}.
     */
    public abstract void clip(int x0, int y0, int x1, int y1);

    /** Ends the innermost {@link #clip}. */
    public abstract void unclip();

    /** Saves the current transformation; {@link #pop()} restores it. */
    public abstract void push();

    /** Restores the transformation saved by the matching {@link #push()}. */
    public abstract void pop();

    /** Moves everything drawn afterwards by (dx, dy), until {@link #pop()}. */
    public abstract void translate(float dx, float dy);

    /** Scales everything drawn afterwards by {@code factor} around the current origin, until {@link #pop()}. */
    public abstract void scale(float factor);

    // ------------------------------------------------------------------ built on the above

    /** Fills the rectangle at (x, y) of w by h. */
    public void rect(int x, int y, int w, int h, int argb) {
        if (w > 0 && h > 0) {
            fill(x, y, x + w, y + h, argb);
        }
    }

    /** A one-unit frame around the rectangle at (x, y) of w by h, inside it. */
    public void outline(int x, int y, int w, int h, int argb) {
        if (w <= 0 || h <= 0) {
            return;
        }
        if (w < 3 || h < 3) {
            // Nothing inside: the frame is the whole rectangle (and no side is drawn inside out).
            fill(x, y, x + w, y + h, argb);
            return;
        }
        fill(x, y, x + w, y + 1, argb);
        fill(x, y + h - 1, x + w, y + h, argb);
        fill(x, y + 1, x + 1, y + h - 1, argb);
        fill(x + w - 1, y + 1, x + w, y + h - 1, argb);
    }

    /**
     * Fills the rectangle at (x, y) of w by h with its corners rounded to {@code radius} (at
     * most half the shorter side). Built from horizontal runs: each row of a corner is as wide
     * as the circle allows, so the edge is stepped at GUI-unit resolution.
     */
    public void roundRect(int x, int y, int w, int h, int radius, int argb) {
        if (w <= 0 || h <= 0) {
            return;
        }
        int r = Math.max(0, Math.min(radius, Math.min(w, h) / 2));
        if (r == 0) {
            fill(x, y, x + w, y + h, argb);
            return;
        }
        for (int row = 0; row < r; row++) {
            // The circle's horizontal extent at the middle of this row.
            double dy = r - row - 0.5;
            int inset = r - (int) Math.round(Math.sqrt(Math.max(0.0, (double) r * r - dy * dy)));
            fill(x + inset, y + row, x + w - inset, y + row + 1, argb);
            fill(x + inset, y + h - row - 1, x + w - inset, y + h - row, argb);
        }
        fill(x, y + r, x + w, y + h - r, argb);
    }

    /** {@link #text} centred horizontally on {@code centreX}. */
    public void centeredText(String text, int centreX, int y, int argb, boolean shadow) {
        text(text, centreX - textWidth(text) / 2, y, argb, shadow);
    }

    /**
     * {@code text} shortened with a trailing ellipsis so that it is at most {@code maxWidth}
     * wide; unchanged when it fits. (For drawing with {@link #text}.)
     */
    public String fit(String text, int maxWidth) {
        if (text == null) {
            return "";
        }
        if (textWidth(text) <= maxWidth) {
            return text;
        }
        String ellipsis = "...";
        int room = maxWidth - textWidth(ellipsis);
        if (room <= 0) {
            return "";
        }
        int end = text.length();
        while (end > 0 && textWidth(text.substring(0, end)) > room) {
            end--;
            if (end > 0 && Character.isLowSurrogate(text.charAt(end)) && Character.isHighSurrogate(text.charAt(end - 1))) {
                end--;
            }
        }
        return text.substring(0, end) + ellipsis;
    }

    /** {@code text} with every control character replaced by a space: what {@link #text} draws. */
    public static String printable(String text) {
        if (text == null) {
            return "";
        }
        StringBuilder out = null;
        for (int i = 0; i < text.length(); i++) {
            char c = text.charAt(i);
            boolean control = c < 0x20 || (c >= 0x7f && c < 0xa0) || (c >= 0x2066 && c <= 0x2069)
                    || (c >= 0x202a && c <= 0x202e) || c == 0x200e || c == 0x200f || c == 0x061c;
            if (control) {
                if (out == null) {
                    out = new StringBuilder(text);
                }
                out.setCharAt(i, ' ');
            }
        }
        return out == null ? text : out.toString();
    }
}
