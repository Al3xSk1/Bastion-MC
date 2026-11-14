package bastion.companion.phone;

import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.Set;

import bastion.companion.Log;
import bastion.companion.platform.Commands;
import bastion.companion.platform.Link;
import bastion.companion.ui.Text;

/**
 * How the phone talks to the launcher: one command line per action, built here and nowhere
 * else, in the grammar of the launcher's {@code phone::Command::parse}:
 * {@code <app> <verb> [arg]... [-- <text>]}.
 *
 * A line that would be outside the grammar is not sent (and logged once per app and verb):
 * the launcher would refuse it anyway, and a phone that never sends one is easier to reason
 * about. What a person typed becomes {@link #sendable} text first.
 */
public final class Send {
    private final Link link;
    private final Set<String> complained = new HashSet<>();
    /** Every line handed to the link, for the self-test; bounded. */
    private final java.util.ArrayDeque<String> recent = new java.util.ArrayDeque<>();

    public Send(Link link) {
        this.link = link;
    }

    /** {@code app verb args...}; false when it was not sent. */
    public boolean command(String app, String verb, String... args) {
        return deliver(app, verb, line(app, verb, args));
    }

    /** {@code app verb args... -- text}, the text made {@link #sendable}; false when not sent. */
    public boolean withText(String app, String verb, String text, String... args) {
        String head = line(app, verb, args);
        return deliver(app, verb, head == null ? null : head + " -- " + sendable(text));
    }

    private static String line(String app, String verb, String... args) {
        StringBuilder out = new StringBuilder(app).append(' ').append(verb);
        for (String arg : args) {
            if (arg == null || !Commands.isArgument(arg)) {
                return null;
            }
            out.append(' ').append(arg);
        }
        return out.toString();
    }

    private boolean deliver(String app, String verb, String line) {
        String problem = line == null ? "bad argument" : Commands.problem(line);
        if (problem != null) {
            if (complained.add(app + " " + verb)) {
                Log.warn("phone: not sending a " + app + " " + verb + " command (" + problem + ")");
            }
            return false;
        }
        synchronized (recent) {
            if (recent.size() == 64) {
                recent.poll();
            }
            recent.add(line);
        }
        return link.send(line);
    }

    /** The last lines handed to the link (oldest first). */
    public java.util.List<String> recent() {
        synchronized (recent) {
            return new java.util.ArrayList<>(recent);
        }
    }

    /**
     * Typed text as the grammar allows it after {@code --}: control characters (a line break
     * included) and direction-changing characters removed, a lone surrogate replaced by
     * U+FFFD, then cut at a character boundary to {@link Commands#MAX_TEXT_BYTES} bytes of
     * UTF-8.
     */
    public static String sendable(String typed) {
        if (typed == null) {
            return "";
        }
        StringBuilder out = new StringBuilder();
        int bytes = 0;
        for (int i = 0; i < typed.length();) {
            int c = typed.codePointAt(i);
            i += Character.charCount(c);
            if (Character.isISOControl(c) || Text.isBidiControl(c)) {
                continue;
            }
            if (c >= Character.MIN_SURROGATE && c <= Character.MAX_SURROGATE) {
                c = 0xFFFD;
            }
            int size = new String(Character.toChars(c)).getBytes(StandardCharsets.UTF_8).length;
            if (bytes + size > Commands.MAX_TEXT_BYTES) {
                break;
            }
            out.appendCodePoint(c);
            bytes += size;
        }
        return out.toString();
    }
}
