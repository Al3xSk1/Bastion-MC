package bastion.companion;

import java.util.ArrayList;
import java.util.List;

/**
 * The bastion-mc companion: a few client-side conveniences that the launcher adds to Fabric
 * instances.
 *
 * What it is not: a security component. It runs in the same JVM as every other mod, any of
 * which can change or imitate it, so it enforces nothing and the launcher believes nothing
 * it says. Everything that protects the player lives in the launcher, outside the sandbox.
 *
 * Start-up, in order: read the launcher's read-only settings; open the channel to the
 * launcher (if there is one); switch on the configured features, each of which resolves the
 * game members it needs and stays off if one is missing; say hello.
 */
public final class Companion {
    private static boolean started;
    private static volatile Channel channel;
    /** Diagnostics raised before the hello has gone out; null afterwards. */
    private static List<String> early = new ArrayList<>();

    private Companion() {
    }

    /** Never throws: whatever fails here, the game starts as if the mod were not there. */
    public static synchronized void start() {
        if (started) {
            return;
        }
        started = true;
        try {
            begin();
        } catch (Throwable failure) {
            Log.warn("could not start (" + Log.describe(failure) + "); the game is not affected");
        }
    }

    private static void begin() {
        Config config = Config.load();
        if (config.problem != null) {
            Log.warn(config.problem + ": every feature stays off");
        }
        channel = Channel.connect(config.text("channel", null));
        Game game = new Game(Companion.class.getClassLoader());

        StringBuilder active = new StringBuilder();
        for (Feature feature : Hooks.ALL) {
            if (config.flag(feature.id())) {
                feature.start(config, game);
                if (feature.live()) {
                    active.append(active.length() == 0 ? "" : ",").append(feature.id());
                }
            }
        }
        String features = active.length() == 0 ? "-" : active.toString();
        Log.info("version " + BuildInfo.VERSION + ", features: " + features
                + " (conveniences only: this mod enforces nothing)");

        List<String> pending;
        synchronized (Companion.class) {
            pending = early;
            early = null;
        }
        Channel line = channel;
        line.send("hello " + Channel.PROTOCOL + " " + BuildInfo.VERSION + " " + features);
        for (String diagnostic : pending) {
            line.send(diagnostic);
        }
        SelfTest.start(config);
    }

    /** One log line and one diagnostic to the launcher: `feature` is off from now on. */
    static void disabled(Feature feature, String reason) {
        Log.warn(feature.id() + " disabled: " + reason);
        report("diag " + feature.id() + " disabled: " + reason);
    }

    private static void report(String line) {
        synchronized (Companion.class) {
            if (early != null) {
                early.add(line);
                return;
            }
        }
        Channel current = channel;
        if (current != null) {
            current.send(line);
        }
    }
}
