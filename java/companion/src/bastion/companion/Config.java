package bastion.companion;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashMap;
import java.util.Map;

/**
 * The settings the launcher wrote for this launch.
 *
 * They come from the launcher's own configuration (`[companion]` in config.toml and the
 * instance's instance.toml) and reach the game as a read-only file inside the sandbox:
 * `key=value` lines, `#` comments. Nothing here is ever written back; there is no settings
 * screen. A missing or unreadable file means "every feature off".
 *
 * Where the launcher's file exists it is the only source. A system property can name
 * another file only where there is none — the mod running without a launcher, which is how
 * the launcher's tests run it. The order matters: a signed profile may carry JVM options of
 * its author's choosing, and one that pointed this mod at a file inside the game directory
 * would let whatever runs in the game rewrite the settings for the next session.
 */
public final class Config {
    /** Where the launcher mounts the file. */
    static final String DEFAULT_PATH = "/bastion/companion/config.properties";
    /** Names the settings file where the launcher has not provided one. */
    static final String PATH_PROPERTY = "bastion.companion.config";
    private static final int MAX_BYTES = 16 * 1024;

    private final Map<String, String> values;
    /** Why there are no settings, or null. */
    final String problem;

    private Config(Map<String, String> values, String problem) {
        this.values = values;
        this.problem = problem;
    }

    static Config load() {
        try {
            return read(DEFAULT_PATH);
        } catch (NoSuchFileException absent) {
            // Not a session the launcher set up; see below.
        } catch (IOException | RuntimeException e) {
            // It is there and cannot be used: no other file is looked for.
            return none("no settings file (" + Log.describe(e) + ")");
        }
        String elsewhere = System.getProperty(PATH_PROPERTY);
        if (elsewhere == null || elsewhere.isEmpty()) {
            return none("no settings file (" + DEFAULT_PATH + ")");
        }
        try {
            return read(elsewhere);
        } catch (IOException | RuntimeException e) {
            return none("no settings file (" + Log.describe(e) + ")");
        }
    }

    private static Config none(String problem) {
        return new Config(new HashMap<>(), problem);
    }

    private static Config read(String location) throws IOException {
        Path path = Paths.get(location);
        if (Files.size(path) > MAX_BYTES) {
            return none("the settings file is implausibly large");
        }
        return parse(new String(Files.readAllBytes(path), StandardCharsets.UTF_8));
    }

    static Config parse(String text) {
        Map<String, String> values = new HashMap<>();
        for (String raw : text.split("\n")) {
            String line = raw.trim();
            int equals = line.indexOf('=');
            if (line.isEmpty() || line.charAt(0) == '#' || equals <= 0) {
                continue;
            }
            values.put(line.substring(0, equals).trim(), line.substring(equals + 1).trim());
        }
        return new Config(values, null);
    }

    public boolean flag(String key) {
        return "true".equals(values.get(key));
    }

    /** A number inside [min, max]; the fallback when it is absent, malformed or outside. */
    public double number(String key, double fallback, double min, double max) {
        String value = values.get(key);
        if (value == null) {
            return fallback;
        }
        try {
            double parsed = Double.parseDouble(value);
            return parsed >= min && parsed <= max ? parsed : fallback;
        } catch (NumberFormatException e) {
            return fallback;
        }
    }

    public String text(String key, String fallback) {
        String value = values.get(key);
        return value == null || value.isEmpty() ? fallback : value;
    }
}
