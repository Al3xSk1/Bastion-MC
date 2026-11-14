package bastion.companion.platform.intermediary;

import bastion.companion.platform.Canvas;
import net.minecraft.class_2583;
import net.minecraft.class_310;
import net.minecraft.class_327;
import net.minecraft.class_332;
import net.minecraft.class_5481;

/**
 * A {@link Canvas} on the game's {@code GuiGraphics} (class_332), for the intermediary
 * families: what they draw alike (rectangles, clipping, measuring text). Text is handed to the
 * game as a {@code FormattedCharSequence} (class_5481) made with {@code forward}, which takes
 * every character literally: a {@code §} is not a formatting code here.
 */
public abstract class BaseCanvas extends Canvas {
    protected final class_332 graphics;
    protected final class_327 font;
    private final int width;
    private final int height;

    protected BaseCanvas(class_332 graphics, int width, int height) {
        this.graphics = graphics;
        this.font = class_310.method_1551().field_1772;
        this.width = width;
        this.height = height;
    }

    @Override
    public int width() {
        return width;
    }

    @Override
    public int height() {
        return height;
    }

    @Override
    public void fill(int x0, int y0, int x1, int y1, int argb) {
        graphics.method_25294(x0, y0, x1, y1, argb);
    }

    /** The text as the game draws it, character for character. */
    protected static class_5481 literal(String text) {
        return class_5481.method_30747(printable(text), class_2583.field_24360);
    }

    @Override
    public int textWidth(String text) {
        return font.method_30880(literal(text));
    }

    @Override
    public int lineHeight() {
        return font.field_2000;
    }

    @Override
    public void clip(int x0, int y0, int x1, int y1) {
        graphics.method_44379(x0, y0, x1, y1);
    }

    @Override
    public void unclip() {
        graphics.method_44380();
    }
}
