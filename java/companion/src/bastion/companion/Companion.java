package bastion.companion;

import java.util.ArrayList;
import java.util.List;

import bastion.companion.platform.Core;
import bastion.companion.platform.Families;
import bastion.companion.platform.Glue;
import bastion.companion.platform.Platforms;
import bastion.companion.ui.Ui;

/**
 * The bastion-mc companion: a few client-side conveniences that the launcher adds to Fabric
 * instances.
 *
 * What it is not: a security component. It runs in the same JVM as every other mod, any of
 * which can change or imitate it, so it enforces nothing and the launcher believes nothing
 * it says. Everything that protects the player lives in the launcher, outside the sandbox.
 *
 * Start-up, in order: read the launcher's read-only settings; open the channel to the
 * launcher (if there is one); start the platform of this game's family of Minecraft versions
 * (the adapter the phone and the home screen are drawn through, see {@code platform}); switch
 * on the configured features — the conveniences and the UI's (phone, home screen, dark
 * menus) — each of which resolves what it needs and stays off if something is missing; say
 * hello.
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
        Platforms.installLink(channel);
        startPlatform(config);
        Game game = new Game(Companion.class.getClassLoader());

        List<Feature> features = new ArrayList<>(List.of(Hooks.ALL));
        try {
            features.addAll(List.of(Ui.features()));
        } catch (Throwable failure) {
            Log.warn("the phone, home screen and dark menus could not be set up (" + Log.describe(failure) + ")");
        }
        StringBuilder active = new StringBuilder();
        String family = Families.of(config.text("minecraft", null));
        for (Feature feature : features) {
            if (config.flag(feature.id())) {
                // On a known family other than 26.3's, the features written for 26.3 are not
                // tried at all; without a version (the launcher's tests run the mod in a plain
                // JVM) they try, and their lookups decide.
                if (feature.writtenFor26_3() && family != null && !"v26_3".equals(family)) {
                    unavailable(feature, "not available on Minecraft " + config.text("minecraft", "?")
                            + " (this feature is written for 26.3's classes)");
                    continue;
                }
                feature.start(config, game);
                if (feature.live()) {
                    active.append(active.length() == 0 ? "" : ",").append(feature.id());
                }
            }
        }
        String names = active.length() == 0 ? "-" : active.toString();
        Log.info("version " + BuildInfo.VERSION + ", features: " + names
                + " (conveniences only: this mod enforces nothing)");

        List<String> pending;
        synchronized (Companion.class) {
            pending = early;
            early = null;
        }
        Channel line = channel;
        line.hello(names);
        for (String diagnostic : pending) {
            line.diag(diagnostic);
        }
        SelfTest.start(config);
    }

    /**
     * The adapter of this game's family, by the version the launcher started
     * ({@code minecraft=} in the settings). Without one the platform stays inert: the phone and
     * the home screen then have nothing to draw with, and say so as features.
     */
    private static void startPlatform(Config config) {
        String version = config.text("minecraft", null);
        String family = Families.of(version);
        if (family == null) {
            Log.info("platform: Minecraft " + version + " is in no family this mod has an adapter for ("
                    + Families.supported() + ")");
            return;
        }
        Glue glue = Families.glue(family);
        if (glue != null) {
            Core.start(glue, version, config);
            Log.info("platform: Minecraft " + version + ", family " + family + " (" + Families.groupOf(family) + ")");
        }
    }

    /** One log line and one diagnostic to the launcher: `feature` is off from now on. */
    static void disabled(Feature feature, String reason) {
        Log.warn(feature.id() + " disabled: " + reason);
        report("diag " + feature.id() + " disabled: " + reason);
    }

    /**
     * As {@link #disabled}, for a feature that does not exist on this version at all: an
     * expected state, said at information level (it is not a failure of anything).
     */
    static void unavailable(Feature feature, String reason) {
        Log.info(feature.id() + " disabled: " + reason);
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
            current.diag(line);
        }
    }
}
