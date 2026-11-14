package bastion.companion;

import java.io.IOException;
import java.net.StandardProtocolFamily;
import java.net.UnixDomainSocketAddress;
import java.nio.ByteBuffer;
import java.nio.channels.SocketChannel;
import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

import bastion.companion.platform.Commands;
import bastion.companion.platform.Core;
import bastion.companion.platform.Json;
import bastion.companion.platform.Link;
import bastion.companion.platform.Pixels;

/**
 * The line to the launcher: one UNIX socket the launcher listens on, bound into the sandbox.
 *
 * Protocol 2 (docs/notes/round4-contract.md). The mod starts with one text line,
 * {@code hello 2 <mod version> <features>}; after it both directions carry frames:
 * {@code u32 length (big-endian; the kind byte and the payload) | u8 kind | payload}. The mod
 * sends COMMAND (0x01: a phone command line) and DIAG (0x02: {@code diag <feature> <text>}, as
 * protocol 1's line); the launcher sends STATE (0x81), FRAME (0x82) and NOTICE (0x83), each
 * starting with the app's name and a 0 byte.
 *
 * Everything the launcher sends is checked against the contract's limits before it is kept:
 * a frame longer than its kind allows, an unknown kind, an app name outside
 * {@code [a-z0-9-]{1,16}}, a state that is not a JSON object, a picture that does not decode
 * within {@link Pixels}' limits, a notice with control characters — any of it ends the
 * connection (said once in the log). Memory is bounded by the largest frame (4 MiB and a few
 * bytes), the newest state per app, the newest picture per app and slot, 32 notices and 256
 * undelivered events.
 *
 * Nothing here may ever hold up the game: sending is a non-blocking offer to a small queue, a
 * daemon thread writes, another reads; a full queue, a dead socket or a rate over the
 * launcher's bound (50 messages, 200 KiB a second) means the message is dropped. Any other mod
 * in this JVM can connect to the same socket and say anything; the launcher believes nothing
 * it reads and enforces every limit itself.
 */
final class Channel implements Link, Core.Delivering {
    static final int PROTOCOL = 2;
    /** The launcher refuses longer text lines; stay well below (bytes, including the newline). */
    private static final int MAX_LINE_BYTES = 480;
    static final int COMMAND = 0x01;
    static final int DIAG = 0x02;
    static final int STATE = 0x81;
    static final int FRAME = 0x82;
    static final int NOTICE = 0x83;
    /** Largest payloads per kind (the app name and its 0 byte included). */
    static final int MAX_STATE = 17 + 64 * 1024;
    static final int MAX_FRAME = 17 + 6 + 4 * 1024 * 1024;
    static final int MAX_NOTICE = 17 + 256;
    static final int MAX_COMMAND = 1024;
    static final int MAX_DIAG = 512;
    /** What the mod sends at most, per second (the launcher's bound on this side of the socket). */
    private static final int MESSAGES_PER_SECOND = 50;
    private static final int BYTES_PER_SECOND = 200 * 1024;
    private static final int MAX_NOTICES = 32;
    private static final int MAX_PENDING = 256;

    private final ArrayBlockingQueue<byte[]> queue = new ArrayBlockingQueue<>(64);
    private final AtomicBoolean open = new AtomicBoolean(true);
    private final AtomicBoolean helloSent = new AtomicBoolean(false);
    private volatile boolean connected;
    private final AtomicLong changes = new AtomicLong();
    private final Map<String, State> states = new HashMap<>();
    private final Map<String, Picture> pictures = new HashMap<>();
    private final ArrayDeque<Notice> notices = new ArrayDeque<>();
    /** What arrived for the listener, delivered on the render thread. */
    private final ArrayDeque<Runnable> pending = new ArrayDeque<>();
    private volatile Listener listener;
    private long serial;
    /** Sending budget: the second it is for, and what was sent in it. */
    private long budgetSecond;
    private int budgetMessages;
    private int budgetBytes;

    private Channel() {
    }

    /** `path` is the socket inside the sandbox; null or empty: no channel. */
    static Channel connect(String path) {
        Channel channel = new Channel();
        if (path == null || path.isEmpty()) {
            channel.open.set(false);
            Log.info("no launcher channel configured");
            return channel;
        }
        Thread writer = new Thread(() -> channel.run(path), "bastion-companion-channel");
        writer.setDaemon(true);
        writer.start();
        return channel;
    }

    /** A channel that is not connected to anything, for the tests' harness. */
    static Channel detached() {
        Channel channel = new Channel();
        channel.open.set(false);
        return channel;
    }

    // ------------------------------------------------------------------ what the mod sends

    /** The one text line that opens protocol 2. Never blocks; false when dropped. */
    boolean hello(String features) {
        if (!open.get() || !helloSent.compareAndSet(false, true)) {
            return false;
        }
        String line = Log.clip("hello " + PROTOCOL + " " + BuildInfo.VERSION + " " + features);
        byte[] bytes = line.getBytes(StandardCharsets.UTF_8);
        int length = Math.min(bytes.length, MAX_LINE_BYTES - 1);
        byte[] out = new byte[length + 1];
        System.arraycopy(bytes, 0, out, 0, length);
        out[length] = '\n';
        return queue.offer(out);
    }

    /** A diagnostic ({@code diag <feature> <text>}), as a DIAG frame after the hello. */
    boolean diag(String line) {
        if (!open.get() || !helloSent.get()) {
            return false;
        }
        byte[] text = Log.clip(line).getBytes(StandardCharsets.UTF_8);
        int length = Math.min(text.length, MAX_DIAG);
        return offerFrame(DIAG, text, length);
    }

    @Override
    public boolean send(String command) {
        if (!connected || !Commands.valid(command)) {
            return false;
        }
        byte[] text = command.getBytes(StandardCharsets.UTF_8);
        return text.length <= MAX_COMMAND && offerFrame(COMMAND, text, text.length);
    }

    private synchronized boolean withinBudget(int bytes) {
        long second = System.nanoTime() / 1_000_000_000L;
        if (second != budgetSecond) {
            budgetSecond = second;
            budgetMessages = 0;
            budgetBytes = 0;
        }
        if (budgetMessages + 1 > MESSAGES_PER_SECOND || budgetBytes + bytes > BYTES_PER_SECOND) {
            return false;
        }
        budgetMessages++;
        budgetBytes += bytes;
        return true;
    }

    private boolean offerFrame(int kind, byte[] payload, int length) {
        if (!withinBudget(length + 5)) {
            return false;
        }
        ByteBuffer frame = ByteBuffer.allocate(5 + length);
        frame.putInt(1 + length).put((byte) kind).put(payload, 0, length);
        return queue.offer(frame.array());
    }

    // ------------------------------------------------------------------ the socket

    private void run(String path) {
        try (SocketChannel socket = SocketChannel.open(StandardProtocolFamily.UNIX)) {
            socket.connect(UnixDomainSocketAddress.of(path));
            Thread reader = new Thread(() -> read(socket), "bastion-companion-channel-reader");
            reader.setDaemon(true);
            boolean readerStarted = false;
            while (true) {
                ByteBuffer buffer = ByteBuffer.wrap(queue.take());
                while (buffer.hasRemaining()) {
                    socket.write(buffer);
                }
                if (!readerStarted) {
                    // The hello is out: from now on the launcher may answer.
                    readerStarted = true;
                    connected = true;
                    changed(l -> l.link(true));
                    reader.start();
                }
            }
        } catch (IOException | RuntimeException e) {
            Log.info("launcher channel unavailable (" + Log.describe(e) + "); continuing without it");
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } finally {
            close();
        }
    }

    private void close() {
        open.set(false);
        queue.clear();
        if (connected) {
            connected = false;
            changed(l -> l.link(false));
        }
    }

    /** Reads frames until the connection ends or the launcher breaks the protocol. */
    private void read(SocketChannel socket) {
        ByteBuffer header = ByteBuffer.allocate(5);
        try {
            while (true) {
                header.clear();
                if (!fill(socket, header)) {
                    break;
                }
                header.flip();
                int length = header.getInt();
                int kind = header.get() & 0xff;
                check(kind, length);
                ByteBuffer payload = ByteBuffer.allocate(length - 1);
                if (!fill(socket, payload)) {
                    throw new IOException("the connection ended inside a frame");
                }
                received(kind, payload.array());
            }
        } catch (IOException | RuntimeException e) {
            Log.info("launcher channel closed: " + Log.describe(e));
        } finally {
            try {
                socket.close();
            } catch (IOException ignored) {
                // Closing is all that is left to do.
            }
            close();
        }
    }

    private static boolean fill(SocketChannel socket, ByteBuffer buffer) throws IOException {
        while (buffer.hasRemaining()) {
            if (socket.read(buffer) < 0) {
                if (buffer.position() == 0) {
                    return false;
                }
                throw new IOException("the connection ended inside a frame");
            }
        }
        return true;
    }

    /** Whether a frame header announces something the launcher may send; throws if not. */
    static void check(int kind, int length) throws IOException {
        int max = maxPayload(kind);
        if (max < 0) {
            throw new IOException("a frame of unknown kind " + kind);
        }
        if (length < 1 || length - 1 > max) {
            throw new IOException("a frame of kind " + kind + " with a length of " + Integer.toUnsignedString(length));
        }
    }

    /** The largest payload the launcher may send for {@code kind}; -1 for a kind it never sends. */
    static int maxPayload(int kind) {
        switch (kind) {
            case STATE:
                return MAX_STATE;
            case FRAME:
                return MAX_FRAME;
            case NOTICE:
                return MAX_NOTICE;
            default:
                return -1;
        }
    }

    /**
     * One frame from the launcher, checked and kept. Throws (and the connection ends) for
     * anything outside the contract.
     */
    void received(int kind, byte[] payload) throws IOException {
        if (payload.length > Math.max(0, maxPayload(kind))) {
            throw new IOException("a frame of kind " + kind + " that is too long");
        }
        int zero = -1;
        for (int i = 0; i < payload.length && i <= Commands.MAX_NAME_BYTES; i++) {
            if (payload[i] == 0) {
                zero = i;
                break;
            }
        }
        if (zero < 1) {
            throw new IOException("a frame without an app name");
        }
        String app = new String(payload, 0, zero, StandardCharsets.US_ASCII);
        if (!Commands.isName(app)) {
            throw new IOException("a frame for an app with an impossible name");
        }
        int at = zero + 1;
        switch (kind) {
            case STATE: {
                String text = utf8(payload, at, payload.length - at);
                Map<String, Object> json;
                try {
                    json = Json.object(text);
                } catch (Json.Malformed e) {
                    throw new IOException("a state that is not a JSON object (" + e.getMessage() + ")");
                }
                synchronized (this) {
                    states.put(app, new State(app, ++serial, text, json));
                }
                changed(l -> l.state(app, text));
                break;
            }
            case FRAME: {
                if (payload.length - at < 6) {
                    throw new IOException("a picture without its header");
                }
                int slot = payload[at] & 0xff;
                int width = ((payload[at + 1] & 0xff) << 8) | (payload[at + 2] & 0xff);
                int height = ((payload[at + 3] & 0xff) << 8) | (payload[at + 4] & 0xff);
                int format = payload[at + 5] & 0xff;
                Pixels pixels = Pixels.decode(format, width, height, payload, at + 6, payload.length - at - 6);
                synchronized (this) {
                    pictures.put(app + "/" + slot, new Picture(app, slot, ++serial, pixels));
                }
                changed(l -> l.frame(app, slot, pixels));
                break;
            }
            case NOTICE: {
                String text = utf8(payload, at, payload.length - at);
                for (int i = 0; i < text.length(); i++) {
                    char c = text.charAt(i);
                    if (Character.getType(c) == Character.CONTROL || Commands.isBidiControl(c)) {
                        throw new IOException("a notice with control characters");
                    }
                }
                Notice notice = new Notice(app, text, System.currentTimeMillis());
                synchronized (this) {
                    if (notices.size() == MAX_NOTICES) {
                        notices.poll();
                    }
                    notices.add(notice);
                }
                changed(l -> l.notice(app, text));
                break;
            }
            default:
                throw new IOException("a frame of unknown kind " + kind);
        }
    }

    /** Strict UTF-8: malformed input is a protocol error, not replacement characters. */
    private static String utf8(byte[] bytes, int offset, int length) throws IOException {
        try {
            return StandardCharsets.UTF_8.newDecoder().decode(ByteBuffer.wrap(bytes, offset, length)).toString();
        } catch (java.nio.charset.CharacterCodingException e) {
            throw new IOException("text that is not UTF-8");
        }
    }

    // ------------------------------------------------------------------ Link

    private interface Delivery {
        void to(Listener listener);
    }

    private void changed(Delivery delivery) {
        changes.incrementAndGet();
        Listener target = listener;
        if (target == null) {
            return;
        }
        synchronized (pending) {
            if (pending.size() < MAX_PENDING) {
                pending.add(() -> delivery.to(target));
            }
        }
    }

    @Override
    public void deliver() {
        while (true) {
            Runnable next;
            synchronized (pending) {
                next = pending.poll();
            }
            if (next == null) {
                return;
            }
            next.run();
        }
    }

    @Override
    public boolean connected() {
        return connected;
    }

    @Override
    public synchronized State state(String app) {
        return states.get(app);
    }

    @Override
    public synchronized Picture picture(String app, int slot) {
        return pictures.get(app + "/" + slot);
    }

    @Override
    public synchronized Notice nextNotice() {
        return notices.poll();
    }

    @Override
    public long changes() {
        return changes.get();
    }

    @Override
    public void setListener(Listener next) {
        synchronized (pending) {
            pending.clear();
        }
        listener = next;
        if (next == null) {
            return;
        }
        boolean up = connected;
        List<State> now;
        synchronized (this) {
            now = new ArrayList<>(states.values());
        }
        synchronized (pending) {
            pending.add(() -> next.link(up));
            for (State state : now) {
                pending.add(() -> next.state(state.app, state.raw));
            }
        }
    }
}
