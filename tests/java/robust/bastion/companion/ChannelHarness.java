package bastion.companion;

import java.awt.image.BufferedImage;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Random;

import javax.imageio.ImageIO;

import bastion.companion.platform.Json;
import bastion.companion.platform.Link;

/**
 * The mod's reader of what the launcher sends (protocol 2), outside the game: well-formed
 * frames are kept as they were meant, malformed ones are refused with an IOException and
 * leave nothing behind, and generated and mutated frames — whatever they hold — never raise
 * anything else. The same for the JSON reader. (In the package of the class under test:
 * `Channel.received` is package-private.)
 *
 * Arguments: a seed, a number of rounds. Prints `accepted=N refused=M` and `ok`, or throws.
 */
public final class ChannelHarness {
    private static int accepted;
    private static int refused;

    private static byte[] join(byte[]... parts) {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        for (byte[] part : parts) {
            out.writeBytes(part);
        }
        return out.toByteArray();
    }

    private static byte[] text(String s) {
        return s.getBytes(StandardCharsets.UTF_8);
    }

    private static byte[] picture(String app, int slot, int w, int h, int format, byte[] bytes) {
        return join(text(app), new byte[] {0, (byte) slot, (byte) (w >> 8), (byte) w, (byte) (h >> 8), (byte) h,
            (byte) format}, bytes);
    }

    private static byte[] encode(String kind, int w, int h) throws IOException {
        BufferedImage image = new BufferedImage(w, h, BufferedImage.TYPE_INT_RGB);
        for (int y = 0; y < h; y++) {
            for (int x = 0; x < w; x++) {
                image.setRGB(x, y, (x * 255 / w) << 16 | (y * 255 / h) << 8 | 0x40);
            }
        }
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        if (!ImageIO.write(image, kind, out)) {
            throw new IllegalStateException("no " + kind + " writer");
        }
        return out.toByteArray();
    }

    private static void must(boolean condition, String what) {
        if (!condition) {
            throw new AssertionError(what);
        }
    }

    private static boolean refuse(Channel channel, int kind, byte[] payload) {
        try {
            channel.received(kind, payload);
            return false;
        } catch (IOException expected) {
            return true;
        }
    }

    public static void main(String[] args) throws Exception {
        long seed = Long.parseLong(args[0]);
        int rounds = Integer.parseInt(args[1]);
        Random random = new Random(seed);

        // Well-formed frames are kept as meant.
        Channel channel = Channel.detached();
        channel.received(Channel.STATE, text("spotify\0{\"playing\":true,\"track\":\"Lantern § Light\",\"n\":3}"));
        Link.State state = channel.state("spotify");
        must(state != null && Boolean.TRUE.equals(state.json.get("playing")), "a state");
        must("Lantern § Light".equals(state.json.get("track")) && Long.valueOf(3).equals(state.json.get("n")), "fields");
        byte[] png = encode("png", 40, 30);
        byte[] jpeg = encode("jpeg", 64, 48);
        byte[] rgba = new byte[2 * 3 * 4];
        for (int i = 0; i < rgba.length; i++) {
            rgba[i] = (byte) (i * 11);
        }
        channel.received(Channel.FRAME, picture("spotify", 0, 40, 30, 1, png));
        channel.received(Channel.FRAME, picture("browser", 0, 64, 48, 2, jpeg));
        channel.received(Channel.FRAME, picture("clips", 7, 2, 3, 3, rgba));
        must(channel.picture("spotify", 0).pixels.width() == 40, "a PNG");
        must(channel.picture("browser", 0).pixels.height() == 48, "a JPEG");
        must(channel.picture("clips", 7).pixels.at(1, 0) == ((rgba[7] & 0xff) << 24 | (rgba[4] & 0xff) << 16
                | (rgba[5] & 0xff) << 8 | (rgba[6] & 0xff)), "RGBA8 as ARGB");
        channel.received(Channel.NOTICE, text("realms\0approved — ☕"));
        Link.Notice notice = channel.nextNotice();
        must(notice != null && notice.text.equals("approved — ☕") && notice.app.equals("realms"), "a notice");

        // Malformed ones are refused, and nothing of them is kept.
        Channel clean = Channel.detached();
        byte[][][] bad = {
            {{(byte) Channel.STATE}, text("spotify{}")},
            {{(byte) Channel.STATE}, text("\0{}")},
            {{(byte) Channel.STATE}, text("Spotify\0{}")},
            {{(byte) Channel.STATE}, text("seventeen-letters\0{}")},
            {{(byte) Channel.STATE}, text("spotify\0[1,2]")},
            {{(byte) Channel.STATE}, text("spotify\0{\"a\":")},
            {{(byte) Channel.STATE}, text("spotify\0" + "[".repeat(40) + "]".repeat(40))},
            {{(byte) Channel.STATE}, join(text("spotify\0{\"a\":\""), new byte[] {(byte) 0xc3, 0x28}, text("\"}"))},
            {{(byte) Channel.FRAME}, picture("spotify", 0, 41, 30, 1, png)},
            {{(byte) Channel.FRAME}, picture("spotify", 0, 40, 30, 2, png)},
            {{(byte) Channel.FRAME}, picture("clips", 0, 2, 3, 3, new byte[23])},
            {{(byte) Channel.FRAME}, picture("clips", 0, 4096, 1, 3, new byte[16384])},
            {{(byte) Channel.FRAME}, picture("clips", 0, 0, 0, 3, new byte[0])},
            {{(byte) Channel.FRAME}, picture("clips", 0, 2, 2, 9, new byte[16])},
            {{(byte) Channel.FRAME}, text("clips\0\0\0")},
            {{(byte) Channel.NOTICE}, text("realms\0line\nbreak")},
            {{(byte) Channel.NOTICE}, text("realms\0‮evil")},
            {{(byte) 0x01}, text("spotify\0play")},
        };
        for (byte[][] frame : bad) {
            must(refuse(clean, frame[0][0] & 0xff, frame[1]), "refused: kind " + (frame[0][0] & 0xff));
        }
        must(clean.state("spotify") == null && clean.picture("spotify", 0) == null && clean.picture("clips", 0) == null
                && clean.nextNotice() == null, "nothing kept");
        for (int[] header : new int[][] {{0x81, 0}, {0x81, Channel.MAX_STATE + 2}, {0x82, Channel.MAX_FRAME + 2},
            {0x83, Channel.MAX_NOTICE + 2}, {0x01, 10}, {0x02, 10}, {0x7f, 10}, {0x81, -1}}) {
            try {
                Channel.check(header[0], header[1]);
                throw new AssertionError("header accepted: " + header[0] + " " + header[1]);
            } catch (IOException expected) {
                // As it must be.
            }
        }
        Channel.check(0x81, 1 + 20);

        // Generated and mutated frames: only IOException, whatever they hold.
        List<byte[][]> seeds = new ArrayList<>();
        seeds.add(new byte[][] {{(byte) Channel.STATE}, text("spotify\0{\"playing\":true,\"list\":[1,2,{\"a\":null}]}")});
        seeds.add(new byte[][] {{(byte) Channel.FRAME}, picture("spotify", 0, 40, 30, 1, png)});
        seeds.add(new byte[][] {{(byte) Channel.FRAME}, picture("browser", 0, 64, 48, 2, jpeg)});
        seeds.add(new byte[][] {{(byte) Channel.FRAME}, picture("clips", 7, 2, 3, 3, rgba)});
        seeds.add(new byte[][] {{(byte) Channel.NOTICE}, text("realms\0approved")});
        for (int round = 0; round < rounds; round++) {
            byte[][] base = seeds.get(random.nextInt(seeds.size()));
            byte[] payload = base[1].clone();
            int kind = base[0][0] & 0xff;
            int mutations = 1 + random.nextInt(6);
            for (int m = 0; m < mutations && payload.length > 0; m++) {
                switch (random.nextInt(5)) {
                    case 0:
                        payload[random.nextInt(payload.length)] = (byte) random.nextInt(256);
                        break;
                    case 1:
                        payload = java.util.Arrays.copyOf(payload, random.nextInt(payload.length + 1));
                        break;
                    case 2: {
                        byte[] noise = new byte[random.nextInt(64)];
                        random.nextBytes(noise);
                        payload = join(payload, noise);
                        break;
                    }
                    case 3:
                        kind = random.nextBoolean() ? kind : 0x81 + random.nextInt(4);
                        break;
                    default: {
                        int at = random.nextInt(payload.length);
                        String specials = "{}[]\",:0\\u";
                        payload[at] = (byte) specials.charAt(random.nextInt(specials.length()));
                        break;
                    }
                }
            }
            try {
                Channel.check(kind, payload.length + 1);
                channel.received(kind, payload);
                accepted++;
            } catch (IOException expected) {
                refused++;
            } catch (Throwable other) {
                StringBuilder hex = new StringBuilder();
                for (int i = 0; i < Math.min(payload.length, 256); i++) {
                    hex.append(String.format("%02x", payload[i] & 0xff));
                }
                throw new AssertionError("round " + round + " (seed " + seed + "), kind " + kind + ": " + other + "\n"
                        + hex, other);
            }
        }

        // The JSON reader: only Json.Malformed, however the text is mangled.
        String[] jsonSeeds = {
            "{\"a\":[1,2.5,-3e2,true,false,null,\"x\\u00e9\\n\"],\"b\":{\"c\":{}}}", "[]", "\"\\ud83d\\ude00\"",
            "{\"deep\":" + "[".repeat(31) + "]".repeat(31) + "}",
        };
        String alphabet = "{}[]\",:.-+eE0123456789tfnul\\ \t\nabcé";
        for (int round = 0; round < rounds; round++) {
            StringBuilder text = new StringBuilder(jsonSeeds[random.nextInt(jsonSeeds.length)]);
            int mutations = 1 + random.nextInt(5);
            for (int m = 0; m < mutations; m++) {
                int at = text.length() == 0 ? 0 : random.nextInt(text.length() + 1);
                char c = alphabet.charAt(random.nextInt(alphabet.length()));
                if (random.nextBoolean() && at < text.length()) {
                    text.setCharAt(at, c);
                } else if (random.nextBoolean() && at < text.length()) {
                    text.deleteCharAt(at);
                } else {
                    text.insert(at, c);
                }
            }
            try {
                Json.parse(text.toString());
            } catch (Json.Malformed expected) {
                // Refused, as it may be.
            } catch (Throwable other) {
                throw new AssertionError("JSON round " + round + " (seed " + seed + "): " + other + " on " + text, other);
            }
        }
        try {
            Json.parse("[".repeat(100000));
            throw new AssertionError("a hundred thousand brackets accepted");
        } catch (Json.Malformed expected) {
            // Too long and too deep: refused, not a stack overflow.
        }
        System.out.println("accepted=" + accepted + " refused=" + refused);
        System.out.println("ok");
    }
}
