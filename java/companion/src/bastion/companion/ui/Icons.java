package bastion.companion.ui;

import bastion.companion.platform.Canvas;

/**
 * The mod's pictograms, drawn as pixel art from the bitmaps below — original drawings, no
 * artwork of the game or of any product. A '#' is one cell; a cell is drawn as a square of
 * {@code scale} GUI units, and every horizontal run of cells as one rectangle.
 *
 * App glyphs are 11 cells wide; control glyphs 7 × 7.
 */
public final class Icons {
    private Icons() {
    }

    /** A bitmap: equal-length rows of '#' and '.'. */
    public record Glyph(String... rows) {
        public int width() {
            return rows.length == 0 ? 0 : rows[0].length();
        }

        public int height() {
            return rows.length;
        }
    }

    // ------------------------------------------------------------------ apps (11 wide)

    /** Music: two beamed notes. */
    public static final Glyph MUSIC = new Glyph(
            "...########",
            "...########",
            "...#......#",
            "...#......#",
            "...#......#",
            "...#......#",
            ".###....###",
            "####...####",
            "####...####",
            ".##.....##.");

    /** Chat: a speech bubble with three dots. */
    public static final Glyph CHAT = new Glyph(
            ".#########.",
            "###########",
            "###########",
            "##.##.##.##",
            "###########",
            "###########",
            ".#########.",
            "..###......",
            "..##.......",
            "..#........");

    /** Web: a globe with its equator and a meridian. */
    public static final Glyph GLOBE = new Glyph(
            "...#####...",
            "..#..#..#..",
            ".#..#.#..#.",
            "#...#.#...#",
            "###########",
            "#...#.#...#",
            "#...#.#...#",
            "###########",
            ".#..#.#..#.",
            "..#..#..#..",
            "...#####...");

    /** Clips: a viewfinder around a recording dot. */
    public static final Glyph CLIP = new Glyph(
            "###.....###",
            "#.........#",
            "#...###...#",
            "...#####...",
            "...#####...",
            "...#####...",
            "#...###...#",
            "#.........#",
            "###.....###");

    /** Realms: a tower with battlements and a gate. */
    public static final Glyph TOWER = new Glyph(
            "#.#.#.#.#.#",
            "###########",
            ".#########.",
            ".#########.",
            ".####.####.",
            ".###...###.",
            ".###...###.",
            ".###...###.",
            ".###...###.",
            "###########");

    /** Settings: a gear. */
    public static final Glyph GEAR = new Glyph(
            "....###....",
            ".#..###..#.",
            "..#######..",
            "..##...##..",
            "####...####",
            "###.....###",
            "####...####",
            "..##...##..",
            "..#######..",
            ".#..###..#.",
            "....###....");

    // ------------------------------------------------------------------ controls (7 × 7)

    /** A note, small (tabs). */
    public static final Glyph MUSIC_SMALL = new Glyph(
            "..#####",
            "..#...#",
            "..#...#",
            "..#...#",
            ".##..##",
            "###.###",
            ".#...#.");

    public static final Glyph PLAY = new Glyph(
            "##.....",
            "####...",
            "######.",
            "#######",
            "######.",
            "####...",
            "##.....");

    public static final Glyph PAUSE = new Glyph(
            "##...##",
            "##...##",
            "##...##",
            "##...##",
            "##...##",
            "##...##",
            "##...##");

    public static final Glyph NEXT = new Glyph(
            "#....##",
            "##...##",
            "###..##",
            "####.##",
            "###..##",
            "##...##",
            "#....##");

    public static final Glyph PREVIOUS = new Glyph(
            "##....#",
            "##...##",
            "##..###",
            "##.####",
            "##..###",
            "##...##",
            "##....#");

    /** Two crossing arrows. */
    public static final Glyph SHUFFLE = new Glyph(
            ".....#.",
            "##...##",
            "..#.##.",
            "...#...",
            "..#.##.",
            "##...##",
            ".....#.");

    /** A loop with an arrowhead. */
    public static final Glyph REPEAT = new Glyph(
            "....#..",
            "######.",
            "#...#..",
            "#.....#",
            "..#...#",
            ".######",
            "..#....");

    /** The loop with a "1" in it: repeat one track. */
    public static final Glyph REPEAT_ONE = new Glyph(
            "....#..",
            "######.",
            "#..##..",
            "#..#..#",
            "..##..#",
            ".######",
            "..#....");

    public static final Glyph SPEAKER = new Glyph(
            "...#...",
            "..##..#",
            "####.#.",
            "####.#.",
            "####.#.",
            "..##..#",
            "...#...");

    public static final Glyph BACK = new Glyph(
            "....#..",
            "...##..",
            "..###..",
            ".####..",
            "..###..",
            "...##..",
            "....#..");

    public static final Glyph FORWARD = new Glyph(
            "..#....",
            "..##...",
            "..###..",
            "..####.",
            "..###..",
            "..##...",
            "..#....");

    public static final Glyph RELOAD = new Glyph(
            "..###.#",
            ".#...##",
            "#...###",
            "#......",
            "#.....#",
            ".#...#.",
            "..###..");

    public static final Glyph CLOSE = new Glyph(
            "#.....#",
            ".#...#.",
            "..#.#..",
            "...#...",
            "..#.#..",
            ".#...#.",
            "#.....#");

    /** Turn the phone: a wide rectangle. */
    public static final Glyph ROTATE = new Glyph(
            ".......",
            "#######",
            "#.....#",
            "#.....#",
            "#.....#",
            "#######",
            ".......");

    public static final Glyph SEARCH = new Glyph(
            ".###...",
            "#...#..",
            "#...#..",
            "#...#..",
            ".####..",
            "....##.",
            ".....##");

    /** Devices: a speaker cabinet. */
    public static final Glyph DEVICE = new Glyph(
            "#######",
            "#.....#",
            "#..#..#",
            "#.....#",
            "#.###.#",
            "#.###.#",
            "#######");

    public static final Glyph CHECK = new Glyph(
            ".......",
            "......#",
            ".....#.",
            "#...#..",
            ".#.#...",
            "..#....",
            ".......");

    public static final Glyph TRASH = new Glyph(
            "..###..",
            "#######",
            ".#####.",
            ".#.#.#.",
            ".#.#.#.",
            ".#.#.#.",
            "..###..");

    public static final Glyph PENCIL = new Glyph(
            ".....##",
            "....###",
            "...###.",
            "..###..",
            ".###...",
            "##.....",
            "#......");

    public static final Glyph DOT = new Glyph(
            ".......",
            "..###..",
            ".#####.",
            ".#####.",
            ".#####.",
            "..###..",
            ".......");

    public static final Glyph PLUS = new Glyph(
            "...#...",
            "...#...",
            "...#...",
            "#######",
            "...#...",
            "...#...",
            "...#...");

    public static final Glyph BELL = new Glyph(
            "...#...",
            "..###..",
            ".#####.",
            ".#####.",
            ".#####.",
            "#######",
            "...#...");

    /** Draws {@code glyph} with its top-left at (x, y), each cell {@code scale} units. */
    public static void draw(Canvas canvas, Glyph glyph, int x, int y, int scale, int argb) {
        String[] rows = glyph.rows();
        for (int row = 0; row < rows.length; row++) {
            String line = rows[row];
            int start = -1;
            for (int col = 0; col <= line.length(); col++) {
                boolean set = col < line.length() && line.charAt(col) == '#';
                if (set && start < 0) {
                    start = col;
                } else if (!set && start >= 0) {
                    canvas.fill(x + start * scale, y + row * scale, x + col * scale, y + (row + 1) * scale, argb);
                    start = -1;
                }
            }
        }
    }

    /** Draws {@code glyph} centred in {@code box} at the largest whole scale that fits (at most {@code maxScale}). */
    public static void drawIn(Canvas canvas, Glyph glyph, Rect box, int maxScale, int argb) {
        int scale = fitScale(glyph, box.w(), box.h(), maxScale);
        if (scale == 0) {
            return;
        }
        int w = glyph.width() * scale;
        int h = glyph.height() * scale;
        draw(canvas, glyph, box.x() + (box.w() - w) / 2, box.y() + (box.h() - h) / 2, scale, argb);
    }

    /** The largest whole scale at which {@code glyph} fits w × h (0: it does not fit at all). */
    public static int fitScale(Glyph glyph, int w, int h, int maxScale) {
        int scale = Math.max(0, maxScale);
        while (scale > 0 && (glyph.width() * scale > w || glyph.height() * scale > h)) {
            scale--;
        }
        return scale;
    }
}
