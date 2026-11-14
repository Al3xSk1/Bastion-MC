package bastion.companion.platform;

import java.util.Map;

/**
 * The phone's line to the launcher: protocol 2 of the companion channel
 * (docs/notes/round4-contract.md). Commands go out; each app's state, its pictures and
 * notices come in. Safe to use from any thread; nothing here blocks.
 *
 * <p>The launcher decides everything: a command is a request it may refuse (unknown verbs,
 * over a rate limit, outside the app's vocabulary), and silence is a possible answer. What
 * comes in is what the launcher chose to show: it replaces what was there before, app by app
 * (state), slot by slot (pictures).
 */
public interface Link {
    /** The launcher speaks protocol 2 on this connection, and it is open. */
    boolean connected();

    /**
     * Queues one command line, {@code <app> <verb> [arg]... [-- <text>]} (the contract's
     * grammar, checked here too). False — and nothing is sent — when the line is outside the
     * grammar, the link is down, or too many commands are waiting.
     */
    boolean send(String command);

    /** The newest state of {@code app}, or null while the launcher has sent none. */
    State state(String app);

    /** The newest picture of {@code app} in {@code slot}, or null. */
    Picture picture(String app, int slot);

    /** The oldest notice not yet taken, or null. At most the last 32 are kept. */
    Notice nextNotice();

    /**
     * A number that grows whenever anything arrives (a state, a picture, a notice), so that
     * a screen can see cheaply whether there is something new.
     */
    long changes();

    /**
     * Have what arrives delivered to {@code listener} as well, on the render thread (once per
     * frame, in the order it arrived); {@code null}: none. On being set, the listener is told
     * the current connection state and the newest state of every app, so that it starts
     * complete. Pictures and notices that arrived before it was set are not repeated.
     */
    void setListener(Listener listener);

    /** What arrives on the link, delivered on the render thread ({@link #setListener}). */
    interface Listener {
        /** Protocol 2 is up (true), or the connection is gone (false). */
        void link(boolean connected);

        /** An app's state: the launcher's JSON object, as text (at most 64 KiB). */
        void state(String app, String json);

        /** One line for the notification area. */
        void notice(String app, String text);

        /** A picture, decoded. */
        void frame(String app, int slot, Pixels pixels);
    }

    /** One app's state: the launcher's JSON object. */
    final class State {
        public final String app;
        /** Grows with every state of this app. */
        public final long serial;
        /** The object as the launcher sent it. */
        public final String raw;
        /** The object, parsed and unmodifiable. */
        public final Map<String, Object> json;

        public State(String app, long serial, String raw, Map<String, Object> json) {
            this.app = app;
            this.serial = serial;
            this.raw = raw;
            this.json = json;
        }
    }

    /** One picture of an app (album art, a browser page, a clip's thumbnail), decoded. */
    final class Picture {
        public final String app;
        public final int slot;
        /** Grows with every picture of this app and slot. */
        public final long serial;
        public final Pixels pixels;

        public Picture(String app, int slot, long serial, Pixels pixels) {
            this.app = app;
            this.slot = slot;
            this.serial = serial;
            this.pixels = pixels;
        }
    }

    /** One line for the phone's notification area: printable text, at most 256 bytes. */
    final class Notice {
        public final String app;
        public final String text;
        /** When it arrived ({@link System#currentTimeMillis()}). */
        public final long millis;

        public Notice(String app, String text, long millis) {
            this.app = app;
            this.text = text;
            this.millis = millis;
        }
    }
}
