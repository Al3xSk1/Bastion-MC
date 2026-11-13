import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.io.FileDescriptor;
import java.io.FileInputStream;
import java.io.IOException;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

/**
 * Bastion launch bootstrap.
 *
 * The JVM is started as {@code java <flags> -cp ... BastionBootstrap} with NO program
 * arguments. The real main class and the complete program argument vector (including
 * {@code --accessToken}, the player name and UUID) arrive as one length-prefixed frame on
 * file descriptor 0, which the launcher has connected to an AF_UNIX socketpair.
 *
 * Consequences:
 *  - /proc/PID/cmdline, /proc/PID/environ, the sun.java.command system property, hsperfdata
 *    and hs_err "Command Line:" sections never contain anything session- or user-specific.
 *  - A socket cannot be re-opened through /proc/PID/fd/0 (unlike a pipe or a memfd), so the
 *    frame cannot be read by a process that merely shares our UID.
 *
 * Honest limit: Minecraft keeps the access token in an immutable java.lang.String for the
 * whole session because it needs it to join servers. That copy cannot be zeroized; only the
 * transport buffers here are. See docs/TOKEN_INJECTION.md.
 *
 * Wire format (big-endian): u32 magic "BSTN", u32 version, u32 count, then count times
 * { u32 byteLength, UTF-8 bytes }. strings[0] is the main class, the rest are its arguments.
 * The peer must close its end after the frame; trailing bytes are a protocol error.
 *
 * Deliberately a single class in the default package, Java 8 compatible, no dependencies.
 */
public final class BastionBootstrap {
    private static final int MAGIC = 0x4253544E;
    private static final int VERSION = 1;
    private static final int MAX_STRINGS = 4096;
    private static final int MAX_TOTAL_BYTES = 1 << 20;

    private BastionBootstrap() {
    }

    public static void main(String[] unused) throws Throwable {
        // Read fd 0 directly and unbuffered: System.in is a BufferedInputStream whose internal
        // 8 KiB buffer would keep a second, un-wipeable copy of the frame alive.
        final FileInputStream channel = new FileInputStream(FileDescriptor.in);
        // From here on, nothing else in this JVM can read the channel through System.in.
        System.setIn(new ByteArrayInputStream(new byte[0]));

        final String[] frame;
        try {
            frame = readFrame(new DataInputStream(channel));
        } catch (IOException e) {
            // Only our own fixed messages are printed; never frame contents.
            System.err.println("bastion-bootstrap: launch channel error: " + e.getMessage());
            System.exit(70);
            return;
        }

        final String mainClass = frame[0];
        final String[] args = Arrays.copyOfRange(frame, 1, frame.length);
        Arrays.fill(frame, null);

        final Method main;
        try {
            final Class<?> cls = Class.forName(mainClass, false, ClassLoader.getSystemClassLoader());
            main = cls.getMethod("main", String[].class);
            if (!Modifier.isStatic(main.getModifiers())) {
                throw new NoSuchMethodException("main is not static");
            }
        } catch (ReflectiveOperationException | LinkageError e) {
            System.err.println("bastion-bootstrap: cannot resolve main class: " + e);
            System.exit(71);
            return;
        }

        try {
            main.invoke(null, (Object) args);
        } catch (InvocationTargetException e) {
            throw e.getCause() != null ? e.getCause() : e;
        }
    }

    private static String[] readFrame(DataInputStream in) throws IOException {
        if (in.readInt() != MAGIC) {
            throw new IOException("bad magic");
        }
        if (in.readInt() != VERSION) {
            throw new IOException("unsupported protocol version");
        }
        final int count = in.readInt();
        if (count < 1 || count > MAX_STRINGS) {
            throw new IOException("bad string count");
        }
        final String[] out = new String[count];
        int total = 0;
        for (int i = 0; i < count; i++) {
            final int len = in.readInt();
            if (len < 0 || len > MAX_TOTAL_BYTES - total) {
                throw new IOException("bad string length");
            }
            total += len;
            final byte[] buf = new byte[len];
            try {
                in.readFully(buf);
                out[i] = new String(buf, StandardCharsets.UTF_8);
            } finally {
                Arrays.fill(buf, (byte) 0);
            }
        }
        if (in.read() != -1) {
            throw new IOException("trailing data after frame");
        }
        return out;
    }
}
