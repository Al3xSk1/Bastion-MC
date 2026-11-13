import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

/** Stand-in for net.minecraft.client.main.Main in the token-injection test. */
public final class EchoMain {
    public static void main(String[] args) throws Exception {
        System.out.println("argc=" + args.length);
        for (int i = 0; i < args.length; i++) {
            System.out.println("arg" + i + "=" + args[i]);
        }
        String token = args.length > 1 ? args[1] : "";
        String cmdline = new String(Files.readAllBytes(Paths.get("/proc/self/cmdline")), StandardCharsets.ISO_8859_1);
        String environ = new String(Files.readAllBytes(Paths.get("/proc/self/environ")), StandardCharsets.ISO_8859_1);
        System.out.println("token_in_cmdline=" + cmdline.contains(token));
        System.out.println("token_in_environ=" + environ.contains(token));
        System.out.println("token_in_sun_java_command=" + String.valueOf(System.getProperty("sun.java.command")).contains(token));
        System.out.println("sun_java_command=" + System.getProperty("sun.java.command"));
        System.out.println("stdin_after=" + System.in.read());
        System.out.println("user_name=" + System.getProperty("user.name"));
        System.out.println("user_home=" + System.getProperty("user.home"));
        System.out.println("java_version=" + System.getProperty("java.version"));
        // A deserialisation attempt must be rejected by the JVM-wide filter.
        try {
            java.io.ByteArrayOutputStream bos = new java.io.ByteArrayOutputStream();
            new java.io.ObjectOutputStream(bos).writeObject(new java.util.ArrayList<String>());
            new java.io.ObjectInputStream(new java.io.ByteArrayInputStream(bos.toByteArray())).readObject();
            System.out.println("deserialize=allowed");
        } catch (java.io.InvalidClassException e) {
            System.out.println("deserialize=rejected");
        }
        // Spawning a shell must fail (Landlock: no execve outside the JVM).
        try {
            new ProcessBuilder("/usr/bin/sh", "-c", "echo pwned").start().waitFor();
            System.out.println("exec_shell=allowed");
        } catch (java.io.IOException e) {
            System.out.println("exec_shell=denied");
        }
        // Threads, JIT, GC and name resolution paths must work under the syscall allowlist.
        Thread t = new Thread(() -> { long x = 0; for (int i = 0; i < 2_000_000; i++) x += i; });
        t.start(); t.join();
        System.gc();
        System.out.println("localhost=" + java.net.InetAddress.getByName("localhost").getHostAddress());
        int n = 0;
        java.util.Enumeration<java.net.NetworkInterface> e = java.net.NetworkInterface.getNetworkInterfaces();
        while (e != null && e.hasMoreElements()) { e.nextElement(); n++; }
        System.out.println("interfaces=" + n);
        System.out.println("done=ok");
    }
}
