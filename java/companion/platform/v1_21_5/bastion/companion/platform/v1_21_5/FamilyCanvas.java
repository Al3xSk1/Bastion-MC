package bastion.companion.platform.v1_21_5;

import bastion.companion.platform.Texture;
import bastion.companion.platform.intermediary.BaseCanvas;
import net.minecraft.class_1921;
import net.minecraft.class_2960;
import net.minecraft.class_332;

/** 1.21.2 to 1.21.5: text through drawString (which returns the width), a 3-D pose stack, blit through a render type. */
final class FamilyCanvas extends BaseCanvas {
    FamilyCanvas(class_332 graphics, int width, int height) {
        super(graphics, width, height);
    }

    /** drawString(Font, FormattedCharSequence, x, y, colour, shadow) */
    @Override
    public void text(String text, int x, int y, int argb, boolean shadow) {
        graphics.method_51430(font, literal(text), x, y, argb, shadow);
    }

    /** pose().pushPose() */
    @Override
    public void push() {
        graphics.method_51448().method_22903();
    }

    /** pose().popPose() */
    @Override
    public void pop() {
        graphics.method_51448().method_22909();
    }

    /** pose().translate(dx, dy, 0) */
    @Override
    public void translate(float dx, float dy) {
        graphics.method_51448().method_46416(dx, dy, 0.0f);
    }

    /** pose().scale(f, f, 1) */
    @Override
    public void scale(float factor) {
        graphics.method_51448().method_22905(factor, factor, 1.0f);
    }

    /**
     * blit(RenderType::guiTextured, id, x, y, u, v, width, height, regionWidth, regionHeight,
     * textureWidth, textureHeight): 1.21.2 to 1.21.5 name the render type through a function.
     */
    @Override
    public void image(Texture texture, int x, int y, int w, int h) {
        if (!(texture instanceof FamilyTexture) || w <= 0 || h <= 0) {
            return;
        }
        FamilyTexture t = (FamilyTexture) texture;
        if (t.id() == null) {
            return;
        }
        graphics.method_25302(id -> class_1921.method_62277((class_2960) id), t.id(), x, y, 0.0f, 0.0f, w, h, t.width(),
                t.height(), t.width(), t.height());
    }
}
