package bastion.companion;

import java.io.IOException;
import java.net.StandardProtocolFamily;
import java.net.UnixDomainSocketAddress;
import java.nio.ByteBuffer;
import java.nio.channels.SocketChannel;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * The line to the launcher: one UNIX socket the launcher listens on, bound into the sandbox.
 *
 * Version 1 is one-way and purely informative — a hello with the mod's version and active
 * features, then diagnostics. The launcher treats every byte as untrusted (any other mod in
 * this JVM can connect to the same socket and say anything) and only logs what it reads.
 *
 * Nothing here may ever hold up the game: sending is a non-blocking offer to a small queue,
 * a daemon thread does the connecting and writing, and a full queue or a dead socket means
 * the line is dropped. With no channel configured, or none reachable, the mod works the same.
 */
final class Channel {
    static final int PROTOCOL = 1;
    /** The launcher refuses longer lines; stay well below (bytes, including the newline). */
    private static final int MAX_LINE_BYTES = 480;

    private final ArrayBlockingQueue<String> queue = new ArrayBlockingQueue<>(64);
    private final AtomicBoolean open = new AtomicBoolean(true);

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

    /** Queue one line. Never blocks, never throws; false when it was dropped. */
    boolean send(String line) {
        return open.get() && queue.offer(Log.clip(line));
    }

    private void run(String path) {
        try (SocketChannel socket = SocketChannel.open(StandardProtocolFamily.UNIX)) {
            socket.connect(UnixDomainSocketAddress.of(path));
            while (true) {
                byte[] bytes = queue.take().getBytes(StandardCharsets.UTF_8);
                int length = Math.min(bytes.length, MAX_LINE_BYTES - 1);
                ByteBuffer buffer = ByteBuffer.allocate(length + 1);
                buffer.put(bytes, 0, length).put((byte) '\n').flip();
                while (buffer.hasRemaining()) {
                    socket.write(buffer);
                }
            }
        } catch (IOException | RuntimeException e) {
            Log.info("launcher channel unavailable (" + Log.describe(e) + "); continuing without it");
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } finally {
            open.set(false);
            queue.clear();
        }
    }
}
