package bastion.companion.ui;

import java.util.Locale;

/**
 * Text that came from outside this mod (a track title, a page title, a notice), made fit to
 * be drawn: one line of printable characters, bounded.
 *
 * Removed: control characters (a line break becomes a space), the characters that change the
 * direction text is drawn in without being visible, and {@code §}, which the game's font
 * would take as the start of a formatting code (so that a title cannot turn into noise or
 * change colour). A lone surrogate becomes U+FFFD.
 */
public final class Text {
    private Text() {
    }

    /** Characters that reorder text invisibly (the same set the launcher refuses). */
    public static boolean isBidiControl(int c) {
        return c == 0x061C || c == 0x200E || c == 0x200F || (c >= 0x202A && c <= 0x202E)
                || (c >= 0x2066 && c <= 0x2069);
    }

    /** {@code text} cleaned as described above and cut to {@code maxCodePoints}; null gives "". */
    public static String clean(String text, int maxCodePoints) {
        if (text == null || maxCodePoints <= 0) {
            return "";
        }
        StringBuilder out = new StringBuilder(Math.min(text.length(), maxCodePoints + 1));
        int count = 0;
        boolean cut = false;
        for (int i = 0; i < text.length();) {
            int c = text.codePointAt(i);
            i += Character.charCount(c);
            if (c == 0xA7 || isBidiControl(c)) {
                continue;
            }
            if (count == maxCodePoints) {
                cut = true;
                break;
            }
            if (Character.isISOControl(c)) {
                c = ' ';
            } else if (c >= Character.MIN_SURROGATE && c <= Character.MAX_SURROGATE) {
                c = 0xFFFD;
            }
            out.appendCodePoint(c);
            count++;
        }
        if (cut && count > 0) {
            // Mark the cut, so that a shortened name never reads as the whole one.
            out.setLength(out.offsetByCodePoints(0, Math.max(0, count - 3)));
            out.append("...");
        }
        return out.toString();
    }

    /** "1:05", "12:00", "1:02:03" for a duration in milliseconds (negative counts as 0). */
    public static String clock(long millis) {
        long seconds = Math.max(0, millis) / 1000;
        long hours = seconds / 3600;
        long minutes = (seconds / 60) % 60;
        long rest = seconds % 60;
        return hours > 0
                ? String.format(Locale.ROOT, "%d:%02d:%02d", hours, minutes, rest)
                : String.format(Locale.ROOT, "%d:%02d", minutes, rest);
    }

    /** "512 B", "3.4 KB", "12.3 MB", "1.2 GB" (powers of 1000, as file managers show). */
    public static String bytes(long bytes) {
        if (bytes < 0) {
            return "?";
        }
        if (bytes < 1000) {
            return bytes + " B";
        }
        String[] units = {"KB", "MB", "GB", "TB"};
        double value = bytes;
        int unit = -1;
        while (value >= 1000 && unit < units.length - 1) {
            value /= 1000;
            unit++;
        }
        return String.format(Locale.ROOT, value < 10 ? "%.1f %s" : "%.0f %s", value, units[unit]);
    }

    /** Upper case, for labels (the theme's tracked capitals are drawn letter by letter). */
    public static String caps(String text) {
        return text == null ? "" : text.toUpperCase(Locale.ROOT);
    }
}
