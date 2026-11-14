package bastion.companion.platform.mojang;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Texture;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.Font;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.network.chat.Style;
import net.minecraft.util.FormattedCharSequence;

/**
 * A {@link Canvas} on 26.x's {@code GuiGraphicsExtractor}: what is drawn is recorded into the
 * frame's render state, which the game draws afterwards. Text goes in as a
 * {@code FormattedCharSequence} made with {@code forward}: every character literally.
 */
public final class FamilyCanvas extends Canvas {
    private final GuiGraphicsExtractor graphics;
    private final Font font;
    private final int width;
    private final int height;

    public FamilyCanvas(GuiGraphicsExtractor graphics, int width, int height) {
        this.graphics = graphics;
        this.font = Minecraft.getInstance().font;
        this.width = width;
        this.height = height;
    }

    private static FormattedCharSequence literal(String text) {
        return FormattedCharSequence.forward(printable(text), Style.EMPTY);
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
        graphics.fill(x0, y0, x1, y1, argb);
    }

    @Override
    public void text(String text, int x, int y, int argb, boolean shadow) {
        graphics.text(font, literal(text), x, y, argb, shadow);
    }

    @Override
    public int textWidth(String text) {
        return font.width(literal(text));
    }

    @Override
    public int lineHeight() {
        return font.lineHeight;
    }

    /** blit(texture, x0, y0, x1, y1, u0, u1, v0, v1): the whole texture into the rectangle. */
    @Override
    public void image(Texture texture, int x, int y, int w, int h) {
        if (!(texture instanceof FamilyTexture) || w <= 0 || h <= 0) {
            return;
        }
        FamilyTexture t = (FamilyTexture) texture;
        if (t.id() != null) {
            graphics.blit(t.id(), x, y, x + w, y + h, 0.0f, 1.0f, 0.0f, 1.0f);
        }
    }

    @Override
    public void clip(int x0, int y0, int x1, int y1) {
        graphics.enableScissor(x0, y0, x1, y1);
    }

    @Override
    public void unclip() {
        graphics.disableScissor();
    }

    @Override
    public void push() {
        graphics.pose().pushMatrix();
    }

    @Override
    public void pop() {
        graphics.pose().popMatrix();
    }

    @Override
    public void translate(float dx, float dy) {
        graphics.pose().translate(dx, dy);
    }

    @Override
    public void scale(float factor) {
        graphics.pose().scale(factor);
    }
}
