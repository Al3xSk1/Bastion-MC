package bastion.companion.platform.mojang;

import java.util.concurrent.atomic.AtomicInteger;

import bastion.companion.platform.Pixels;
import bastion.companion.platform.Texture;
import com.mojang.blaze3d.platform.NativeImage;
import net.minecraft.client.Minecraft;
import net.minecraft.client.renderer.texture.DynamicTexture;
import net.minecraft.resources.Identifier;

/** A texture from {@link Pixels} for 26.x: a NativeImage (pixels set as ARGB) in a DynamicTexture with a label. */
final class FamilyTexture implements Texture {
    private static final AtomicInteger NEXT = new AtomicInteger();

    private Identifier id;
    private DynamicTexture texture;
    private int width;
    private int height;

    FamilyTexture(Pixels pixels) {
        make(pixels);
    }

    private void make(Pixels pixels) {
        NativeImage image = new NativeImage(pixels.width(), pixels.height(), false);
        copy(pixels, image);
        int number = NEXT.incrementAndGet();
        texture = new DynamicTexture(() -> "bastion-companion picture " + number, image);
        id = Identifier.tryBuild("bastion-companion", "picture/" + number);
        Minecraft.getInstance().getTextureManager().register(id, texture);
        width = pixels.width();
        height = pixels.height();
    }

    private static void copy(Pixels pixels, NativeImage image) {
        int[] argb = pixels.shared();
        int w = pixels.width();
        for (int i = 0; i < argb.length; i++) {
            image.setPixel(i % w, i / w, argb[i]);
        }
    }

    Identifier id() {
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
            copy(pixels, texture.getPixels());
            texture.upload();
            return;
        }
        close();
        make(pixels);
    }

    @Override
    public void close() {
        if (id != null) {
            Minecraft.getInstance().getTextureManager().release(id);
            id = null;
            texture = null;
        }
    }
}
