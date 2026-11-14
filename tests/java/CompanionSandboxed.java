import bastion.companion.Companion;
import bastion.companion.Hooks;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

/**
 * The companion mod in a real JVM inside the launcher's real sandbox, with exactly what an
 * injection adds to it (the jar, the settings, the socket) — and no Minecraft, so a feature
 * that is switched on reports itself off. Driven by `tests/companion.rs`.
 *
 * Before the mod starts, this program does what a mod of an earlier session could have
 * done: it writes settings of its own choosing to a place the game can write (its
 * argument). The test has started the JVM with the mod's settings property naming that
 * file, as a signed profile's JVM options could. The mod must take no notice: where the
 * launcher's file exists, it is the only source.
 *
 * Then it waits to be stopped: the test does that once the launcher has what the mod sent.
 */
public final class CompanionSandboxed {
    private static void show(String name, Object value) {
        System.out.println(name + "=" + value);
        System.out.flush();
    }

    public static void main(String[] args) throws Exception {
        Path planted = Paths.get(args[0]);
        Files.write(planted, "zoom=false\nhud=true\nfullbright=true\n".getBytes(StandardCharsets.UTF_8));
        show("planted", new String(Files.readAllBytes(planted), StandardCharsets.UTF_8).replace('\n', '|'));
        show("property", System.getProperty("bastion.companion.config"));
        Companion.start();
        show("live", Hooks.ZOOM.live() + "," + Hooks.HUD.live() + "," + Hooks.FULLBRIGHT.live());
        show("ready", "ok");
        Thread.sleep(60_000);
        show("timeout", "nobody stopped this program");
    }
}
