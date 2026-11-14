package bastion.companion.platform;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.util.Iterator;

import javax.imageio.ImageIO;
import javax.imageio.ImageReader;
import javax.imageio.stream.ImageInputStream;
import javax.imageio.stream.MemoryCacheImageInputStream;

/**
 * A decoded picture: {@code width × height} pixels, row by row from the top, each
 * {@code 0xAARRGGBB}. Immutable once made; safe to hand from one thread to another.
 *
 * {@link #decode} turns the launcher's image bytes (PNG, JPEG, or raw RGBA8) into this, with
 * hard limits on size, on any thread — so that the render thread only uploads. Decoding uses
 * the JDK's own image readers on bytes in memory (no file cache), and refuses a picture whose
 * declared size is over the limit before decoding a single pixel.
 */
public final class Pixels {
    /** The longest side accepted, in pixels. */
    public static final int MAX_SIDE = 2048;
    /** The most pixels accepted (sixteen megabytes once decoded). */
    public static final int MAX_PIXELS = 2048 * 2048;
    /** The most encoded bytes accepted: the protocol's limit for one frame. */
    public static final int MAX_BYTES = 4 * 1024 * 1024;

    /** The launcher's format numbers (protocol 2, FRAME). */
    public static final int PNG = 1;
    public static final int JPEG = 2;
    public static final int RGBA8 = 3;

    private final int width;
    private final int height;
    private final int[] argb;

    private Pixels(int width, int height, int[] argb) {
        this.width = width;
        this.height = height;
        this.argb = argb;
    }

    /** A picture from {@code argb} (copied), which must hold exactly width × height pixels. */
    public static Pixels of(int width, int height, int[] argb) {
        check(width, height);
        if (argb == null || argb.length != width * height) {
            throw new IllegalArgumentException("expected " + width * height + " pixels");
        }
        return new Pixels(width, height, argb.clone());
    }

    /** A picture of one colour. */
    public static Pixels filled(int width, int height, int argb) {
        check(width, height);
        int[] pixels = new int[width * height];
        java.util.Arrays.fill(pixels, argb);
        return new Pixels(width, height, pixels);
    }

    private static void check(int width, int height) {
        if (width <= 0 || height <= 0 || width > MAX_SIDE || height > MAX_SIDE || (long) width * height > MAX_PIXELS) {
            throw new IllegalArgumentException("a picture of " + width + "x" + height + " is outside the limits");
        }
    }

    public int width() {
        return width;
    }

    public int height() {
        return height;
    }

    /** The pixel at (x, y), {@code 0xAARRGGBB}. */
    public int at(int x, int y) {
        return argb[y * width + x];
    }

    /** All pixels (a copy). */
    public int[] argb() {
        return argb.clone();
    }

    /** For adapters: the pixels without a copy. Must not be changed. */
    public int[] shared() {
        return argb;
    }

    /**
     * Decodes the launcher's bytes. {@code width} and {@code height} are what the launcher
     * declared: for RGBA8 they are the picture's size; for PNG and JPEG the picture must have
     * exactly that size too. Throws {@link IOException} for anything that is not a picture
     * within the limits.
     */
    public static Pixels decode(int format, int width, int height, byte[] bytes, int offset, int length)
            throws IOException {
        if (bytes == null || offset < 0 || length < 0 || offset > bytes.length - length) {
            throw new IOException("no picture");
        }
        if (length == 0 || length > MAX_BYTES) {
            throw new IOException("a picture of " + length + " bytes is outside the limits");
        }
        try {
            check(width, height);
        } catch (IllegalArgumentException e) {
            throw new IOException(e.getMessage());
        }
        switch (format) {
            case RGBA8:
                return rgba(width, height, bytes, offset, length);
            case PNG:
                return encoded("png", width, height, bytes, offset, length);
            case JPEG:
                return encoded("jpeg", width, height, bytes, offset, length);
            default:
                throw new IOException("unknown picture format " + format);
        }
    }

    private static Pixels rgba(int width, int height, byte[] bytes, int offset, int length) throws IOException {
        if ((long) width * height * 4 != length) {
            throw new IOException("RGBA8 of " + width + "x" + height + " needs " + (long) width * height * 4
                    + " bytes, not " + length);
        }
        int[] argb = new int[width * height];
        for (int i = 0, at = offset; i < argb.length; i++, at += 4) {
            int r = bytes[at] & 0xff;
            int g = bytes[at + 1] & 0xff;
            int b = bytes[at + 2] & 0xff;
            int a = bytes[at + 3] & 0xff;
            argb[i] = (a << 24) | (r << 16) | (g << 8) | b;
        }
        return new Pixels(width, height, argb);
    }

    private static Pixels encoded(String kind, int width, int height, byte[] bytes, int offset, int length)
            throws IOException {
        Iterator<ImageReader> readers = ImageIO.getImageReadersByFormatName(kind);
        if (!readers.hasNext()) {
            throw new IOException("this Java has no " + kind + " reader");
        }
        ImageReader reader = readers.next();
        try (ImageInputStream input = new MemoryCacheImageInputStream(new ByteArrayInputStream(bytes, offset, length))) {
            reader.setInput(input, true, true);
            int declaredWidth = reader.getWidth(0);
            int declaredHeight = reader.getHeight(0);
            if (declaredWidth != width || declaredHeight != height) {
                throw new IOException("the " + kind + " is " + declaredWidth + "x" + declaredHeight + ", announced as "
                        + width + "x" + height);
            }
            java.awt.image.BufferedImage image = reader.read(0);
            int[] argb = image.getRGB(0, 0, width, height, null, 0, width);
            return new Pixels(width, height, argb);
        } catch (RuntimeException e) {
            // Image readers throw unchecked exceptions for some corrupt inputs.
            throw new IOException("not a readable " + kind + " (" + e.getClass().getSimpleName() + ")");
        } finally {
            reader.dispose();
        }
    }
}
