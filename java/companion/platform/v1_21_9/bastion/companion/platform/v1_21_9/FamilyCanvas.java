package bastion.companion.platform.v1_21_9;

import bastion.companion.platform.Texture;
import bastion.companion.platform.intermediary.BaseCanvas;
import net.minecraft.class_332;

/** From 1.21.6 on: the GUI is a render state; a 2-D pose stack; blit by texture id and corners. */
final class FamilyCanvas extends BaseCanvas {
    FamilyCanvas(class_332 graphics, int width, int height) {
        super(graphics, width, height);
    }

    /** drawString(Font, FormattedCharSequence, x, y, colour, shadow), which returns nothing from 1.21.6 on */
    @Override
    public void text(String text, int x, int y, int argb, boolean shadow) {
        graphics.method_51430(font, literal(text), x, y, argb, shadow);
    }

    /** pose().pushMatrix(): a 2-D matrix stack (JOML) from 1.21.6 on */
    @Override
    public void push() {
        graphics.method_51448().pushMatrix();
    }

    /** pose().popMatrix() */
    @Override
    public void pop() {
        graphics.method_51448().popMatrix();
    }

    /** pose().translate(dx, dy) */
    @Override
    public void translate(float dx, float dy) {
        graphics.method_51448().translate(dx, dy);
    }

    /** pose().scale(f) */
    @Override
    public void scale(float factor) {
        graphics.method_51448().scale(factor);
    }

    /** blit(id, x0, y0, x1, y1, u0, u1, v0, v1): the whole texture into the rectangle (from 1.21.6 on) */
    @Override
    public void image(Texture texture, int x, int y, int w, int h) {
        if (!(texture instanceof FamilyTexture) || w <= 0 || h <= 0) {
            return;
        }
        FamilyTexture t = (FamilyTexture) texture;
        if (t.id() != null) {
            graphics.method_70845(t.id(), x, y, x + w, y + h, 0.0f, 1.0f, 0.0f, 1.0f);
        }
    }
}
