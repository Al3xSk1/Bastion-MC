package bastion.companion;

import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * The mod's few log lines, through the game's own logging (SLF4J, reached by name because
 * nothing of the game is on the compile-time class path) or, failing that, standard error.
 *
 * The number of lines is capped for the whole session: whatever goes wrong in a feature,
 * and however often its hook is called, this mod cannot flood the log.
 */
public final class Log {
    private static final int MAX_LINES = 120;
    private static final int MAX_CHARS = 400;
    private static final AtomicInteger LINES = new AtomicInteger();
    private static final Object LOGGER;
    private static final MethodHandle INFO;
    private static final MethodHandle WARN;

    static {
        Object logger = null;
        MethodHandle info = null;
        MethodHandle warn = null;
        try {
            ClassLoader loader = Log.class.getClassLoader();
            Class<?> factory = Class.forName("org.slf4j.LoggerFactory", true, loader);
            Class<?> type = Class.forName("org.slf4j.Logger", false, loader);
            MethodHandles.Lookup lookup = MethodHandles.publicLookup();
            MethodType emit = MethodType.methodType(void.class, Object.class, String.class);
            info = lookup.findVirtual(type, "info", MethodType.methodType(void.class, String.class)).asType(emit);
            warn = lookup.findVirtual(type, "warn", MethodType.methodType(void.class, String.class)).asType(emit);
            logger = lookup.findStatic(factory, "getLogger", MethodType.methodType(type, String.class))
                    .invoke("bastion-companion");
        } catch (Throwable unavailable) {
            logger = null;
        }
        LOGGER = logger;
        INFO = info;
        WARN = warn;
    }

    private Log() {
    }

    public static void info(String message) {
        emit(INFO, message);
    }

    public static void warn(String message) {
        emit(WARN, message);
    }

    private static void emit(MethodHandle level, String message) {
        int count = LINES.incrementAndGet();
        if (count > MAX_LINES) {
            return;
        }
        String text = count == MAX_LINES ? "further messages from the companion are not logged" : clip(message);
        if (LOGGER != null && level != null) {
            try {
                level.invokeExact(LOGGER, text);
                return;
            } catch (Throwable ignored) {
                // Fall through to standard error.
            }
        }
        System.err.println("[bastion-companion] " + text);
    }

    /** One bounded line of printable characters, whatever went in. */
    public static String clip(String message) {
        String text = message == null ? "null" : message;
        StringBuilder out = new StringBuilder(Math.min(text.length(), MAX_CHARS));
        for (int i = 0; i < text.length() && out.length() < MAX_CHARS; i++) {
            char c = text.charAt(i);
            out.append(c < 0x20 || c == 0x7f ? ' ' : c);
        }
        return out.toString();
    }

    /** "NoSuchMethodException: …", bounded: what a lookup failure is reported as. */
    public static String describe(Throwable failure) {
        String message = failure.getMessage();
        String text = failure.getClass().getSimpleName() + (message == null ? "" : ": " + message);
        return clip(text.length() > 200 ? text.substring(0, 200) : text);
    }
}
