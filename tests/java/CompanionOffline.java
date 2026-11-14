import bastion.companion.Companion;
import bastion.companion.Hooks;

/**
 * The companion mod in a JVM that has no Minecraft, no Fabric Loader and no Mixin: every
 * lookup the mod makes fails. What must still hold: starting it throws nothing, each
 * feature that was switched on reports once that it is off, and the hooks — called here as
 * often as a game would call them in a few seconds — return the game's own values.
 *
 * Driven by `tests/companion.rs`, which reads what arrives on the channel and closes this
 * program's standard input when it has seen enough.
 */
public final class CompanionOffline {
    public static void main(String[] args) throws Exception {
        Companion.start();
        Companion.start();
        Object anything = new Object();
        float fov = 0.0f;
        boolean wheel = false;
        for (int frame = 0; frame < 2000; frame++) {
            fov = Hooks.fov(70.0f);
            wheel |= Hooks.wheel(1.0);
            Hooks.turn(anything);
            Hooks.hud(anything, anything);
            Hooks.lightmap(anything);
        }
        System.out.println("fov=" + fov);
        System.out.println("wheel=" + wheel);
        System.out.println("live=" + Hooks.ZOOM.live() + "," + Hooks.HUD.live() + "," + Hooks.FULLBRIGHT.live());
        System.out.flush();
        // Until the test has read the channel (or given up).
        while (System.in.read() != -1) {
            continue;
        }
        System.out.println("done");
    }
}
