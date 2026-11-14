package bastion.companion.platform.v1_21_9;

import java.util.concurrent.atomic.AtomicInteger;

import bastion.companion.platform.Pixels;
import bastion.companion.platform.Texture;
import net.minecraft.class_1011;
import net.minecraft.class_1043;
import net.minecraft.class_2960;
import net.minecraft.class_310;

/**
 * A texture made from {@link Pixels}, from 1.21.2: a NativeImage (class_1011) whose pixels
 * are set as ARGB, in a DynamicTexture (class_1043, with a label since 1.21.5) registered under an id of this mod's.
 */
final class FamilyTexture implements Texture {
    private static final AtomicInteger NEXT = new AtomicInteger();

    private class_2960 id;
    private class_1043 texture;
    private int width;
    private int height;

    FamilyTexture(Pixels pixels) {
        make(pixels);
    }

    private void make(Pixels pixels) {
        class_1011 image = new class_1011(pixels.width(), pixels.height(), false);
        copy(pixels, image);
        int number = NEXT.incrementAndGet();
        texture = new class_1043(() -> "bastion-companion picture " + number, image);
        id = class_2960.method_43902("bastion-companion", "picture/" + number);
        class_310.method_1551().method_1531().method_4616(id, texture);
        width = pixels.width();
        height = pixels.height();
    }

    /** NativeImage.setPixel(x, y, ARGB) for every pixel (from 1.21.2 on). */
    private static void copy(Pixels pixels, class_1011 image) {
        int[] argb = pixels.shared();
        int w = pixels.width();
        for (int i = 0; i < argb.length; i++) {
            image.method_61941(i % w, i / w, argb[i]);
        }
    }

    class_2960 id() {
        return id;
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
    public void replace(Pixels pixels) {
        if (texture != null && pixels.width() == width && pixels.height() == height) {
            copy(pixels, texture.method_4525());
            texture.method_4524();
            return;
        }
        close();
        make(pixels);
    }

    @Override
    public void close() {
        if (id != null) {
            class_310.method_1551().method_1531().method_4615(id);
            id = null;
            texture = null;
        }
    }
}
