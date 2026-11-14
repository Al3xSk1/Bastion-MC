package bastion.companion.platform;

import java.nio.charset.StandardCharsets;

/**
 * The command grammar of the phone protocol, checked on the mod's side before anything is
 * sent: {@code <app> <verb> [arg]... [-- <text>]} — the same rules as the launcher's
 * {@code phone::Command::parse}, which decides in the end.
 *
 * Names are {@code [a-z0-9-]}, 1 to 16 bytes, not starting with {@code -}; at most 8
 * arguments of printable ASCII without spaces, at most 64 bytes each and never {@code --};
 * free text after {@code " -- "} at most 512 bytes of UTF-8 without control characters or
 * bidirectional overrides; the whole line at most 1024 bytes.
 */
public final class Commands {
    public static final int MAX_COMMAND_BYTES = 1024;
    public static final int MAX_ARGS = 8;
    public static final int MAX_ARG_BYTES = 64;
    public static final int MAX_NAME_BYTES = 16;
    public static final int MAX_TEXT_BYTES = 512;

    private Commands() {
    }

    /** Null when {@code line} follows the grammar; otherwise why not (the launcher's word for it). */
    public static String problem(String line) {
        if (line == null || line.isEmpty()) {
            return "empty";
        }
        for (int i = 0; i < line.length(); i++) {
            char c = line.charAt(i);
            if (Character.isHighSurrogate(c)) {
                if (i + 1 >= line.length() || !Character.isLowSurrogate(line.charAt(i + 1))) {
                    return "not UTF-8";
                }
                i++;
            } else if (Character.isLowSurrogate(c)) {
                return "not UTF-8";
            }
        }
        byte[] bytes = line.getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_COMMAND_BYTES) {
            return "too long";
        }
        for (int i = 0; i < line.length(); i++) {
            char c = line.charAt(i);
            if (Character.getType(c) == Character.CONTROL || isBidiControl(c)) {
                return "control character";
            }
        }
        String head = line;
        String text = null;
        int split = line.indexOf(" -- ");
        if (split >= 0) {
            head = line.substring(0, split);
            text = line.substring(split + 4);
        }
        if (text != null && text.getBytes(StandardCharsets.UTF_8).length > MAX_TEXT_BYTES) {
            return "text too long";
        }
        String[] words = head.split(" ", -1);
        if (words[0].isEmpty()) {
            return "empty";
        }
        if (words.length < 2 || !isName(words[0]) || !isName(words[1])) {
            return "bad app or verb name";
        }
        // In order, as the launcher checks: a ninth argument is "too many" even when it is
        // also malformed, a malformed earlier one is "bad".
        for (int i = 2; i < words.length; i++) {
            if (i - 2 == MAX_ARGS) {
                return "too many arguments";
            }
            if (!isArgument(words[i])) {
                return "bad argument";
            }
        }
        return null;
    }

    /** Whether {@code line} follows the grammar. */
    public static boolean valid(String line) {
        return problem(line) == null;
    }

    /** {@code [a-z0-9-]{1,16}}, not starting with {@code -}. */
    public static boolean isName(String word) {
        if (word.isEmpty() || word.length() > MAX_NAME_BYTES || word.charAt(0) == '-') {
            return false;
        }
        for (int i = 0; i < word.length(); i++) {
            char c = word.charAt(i);
            if (!((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-')) {
                return false;
            }
        }
        return true;
    }

    /** Printable ASCII without spaces, 1 to 64 bytes, not {@code --}. */
    public static boolean isArgument(String word) {
        if (word.isEmpty() || word.length() > MAX_ARG_BYTES || word.equals("--")) {
            return false;
        }
        for (int i = 0; i < word.length(); i++) {
            char c = word.charAt(i);
            if (c < 0x21 || c > 0x7e) {
                return false;
            }
        }
        return true;
    }

    /** Characters that change how text is displayed without being visible. */
    public static boolean isBidiControl(char c) {
        return c == '؜' || c == '‎' || c == '‏' || (c >= '‪' && c <= '‮')
                || (c >= '⁦' && c <= '⁩');
    }
}
