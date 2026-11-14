package bastion.companion.phone;

import java.time.Instant;
import java.time.LocalDate;
import java.time.ZoneId;
import java.time.ZonedDateTime;
import java.time.format.DateTimeFormatter;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.regex.Pattern;

import bastion.companion.platform.Commands;
import bastion.companion.platform.Json;
import bastion.companion.ui.Text;

/**
 * Each app's state as the phone shows it, read from the launcher's JSON object
 * (docs/notes/round4-contract.md, "App vocabularies").
 *
 * Reading never fails: a field that is missing, of another type or out of range takes its
 * default, unknown fields are ignored, text is cleaned and bounded ({@link Text#clean}), lists
 * are cut to {@link #MAX_ITEMS}. Anything the phone may send back later — a Spotify URI, a
 * device id, a site name, a clip id — is kept only when it is something the phone could send
 * in a command (an argument of the grammar, a URI of the documented kinds); otherwise the entry
 * is shown but cannot be chosen.
 */
public final class AppStates {
    /** Longest text kept from one field (code points). */
    public static final int MAX_TEXT = 200;
    /** Most entries kept from one list. */
    public static final int MAX_ITEMS = 64;
    /** Most apps the phone shows. */
    public static final int MAX_APPS = 16;

    private static final Pattern SPOTIFY_URI =
            Pattern.compile("spotify:(track|album|playlist|artist|episode|show):[0-9A-Za-z]{1,40}");

    private AppStates() {
    }

    static String text(Map<String, Object> m, String key) {
        return Text.clean(Json.string(m, key, ""), MAX_TEXT);
    }

    /** A string that the phone may send back as one argument, or null. */
    static String argument(Map<String, Object> m, String key) {
        String value = Json.string(m, key, null);
        return value != null && Commands.isArgument(value) ? value : null;
    }

    static long integer(Map<String, Object> m, String key, long fallback, long min, long max) {
        double value = Json.number(m, key, Double.NaN);
        if (Double.isNaN(value) || Double.isInfinite(value)) {
            return fallback;
        }
        return (long) Math.max(min, Math.min(max, value));
    }

    static List<Map<String, Object>> items(Map<String, Object> m, String key) {
        List<Map<String, Object>> all = Json.objects(Json.list(m, key));
        return all.size() > MAX_ITEMS ? all.subList(0, MAX_ITEMS) : all;
    }

    /** Whether {@code uri} is a Spotify URI of the documented kinds. */
    public static boolean isSpotifyUri(String uri) {
        return uri != null && SPOTIFY_URI.matcher(uri).matches();
    }

    /** A track or an episode (what `queue` accepts). */
    public static boolean isQueueable(String uri) {
        return isSpotifyUri(uri) && (uri.startsWith("spotify:track:") || uri.startsWith("spotify:episode:"));
    }

    /**
     * A time the launcher reported, for display: a string as it is (cleaned), a number as
     * seconds (or milliseconds, if it is that large) since 1970 in the local time; "" when
     * absent.
     */
    static String when(Object value) {
        if (value instanceof String) {
            return Text.clean((String) value, 40);
        }
        if (!(value instanceof Number)) {
            return "";
        }
        double number = ((Number) value).doubleValue();
        if (Double.isNaN(number) || number <= 0 || number > 1e15) {
            return "";
        }
        long seconds = (long) (number > 1e11 ? number / 1000 : number);
        try {
            ZonedDateTime at = Instant.ofEpochSecond(seconds).atZone(ZoneId.systemDefault());
            boolean today = at.toLocalDate().equals(LocalDate.now(ZoneId.systemDefault()));
            return at.format(DateTimeFormatter.ofPattern(today ? "HH:mm" : "d MMM HH:mm", Locale.ROOT));
        } catch (RuntimeException e) {
            return "";
        }
    }

    /** A count or a list, for display: a number as it is, a list as its length, text as text. */
    static String amount(Object value) {
        if (value instanceof Number) {
            double number = ((Number) value).doubleValue();
            return Double.isNaN(number) || Double.isInfinite(number) ? "" : String.valueOf((long) number);
        }
        if (value instanceof List) {
            return String.valueOf(((List<?>) value).size());
        }
        if (value instanceof String) {
            return Text.clean((String) value, 60);
        }
        return "";
    }

    /** The strings among a list's first entries, cleaned (for names shown after a count). */
    static List<String> names(Object value, int max) {
        List<String> out = new ArrayList<>();
        if (value instanceof List) {
            for (Object item : (List<?>) value) {
                if (out.size() == max) {
                    break;
                }
                if (item instanceof String) {
                    out.add(Text.clean((String) item, 40));
                }
            }
        }
        return out;
    }

    // ------------------------------------------------------------------ phone

    /** The dispatcher's `phone` state: which apps this launch has. */
    public static final class PhoneInfo {
        public final List<String> apps;
        public final String theme;

        PhoneInfo(List<String> apps, String theme) {
            this.apps = apps;
            this.theme = theme;
        }

        public static final PhoneInfo NONE = new PhoneInfo(Collections.emptyList(), "dark");

        public static PhoneInfo read(Map<String, Object> m) {
            List<String> apps = new ArrayList<>();
            for (Object item : Json.list(m, "apps")) {
                if (item instanceof String && Commands.isName((String) item) && !apps.contains(item)
                        && apps.size() < MAX_APPS) {
                    apps.add((String) item);
                }
            }
            return new PhoneInfo(Collections.unmodifiableList(apps), text(m, "theme"));
        }
    }

    // ------------------------------------------------------------------ spotify

    public record Result(String uri, String title, String subtitle) {
    }

    public record Device(String id, String name, boolean active) {
    }

    public static final class Spotify {
        /** "mpris", "web" or "none". */
        public final String source;
        public final boolean playing;
        public final String track;
        public final String artist;
        public final String album;
        public final long durationMs;
        public final long positionMs;
        /** 0 to 100, or -1 when unknown. */
        public final int volume;
        public final boolean shuffle;
        /** "off", "track" or "context". */
        public final String repeat;
        public final List<Result> results;
        public final List<Device> devices;
        public final String error;
        /** When this state arrived ({@link System#nanoTime()}), to advance the position while playing. */
        public final long receivedNanos;

        private Spotify(Map<String, Object> m, long receivedNanos) {
            String src = Json.string(m, "source", "none");
            source = "mpris".equals(src) || "web".equals(src) ? src : "none";
            playing = Json.flag(m, "playing", false);
            track = text(m, "track");
            artist = text(m, "artist");
            album = text(m, "album");
            durationMs = integer(m, "duration_ms", 0, 0, 24L * 3600 * 1000);
            positionMs = Math.min(integer(m, "position_ms", 0, 0, 24L * 3600 * 1000),
                    durationMs > 0 ? durationMs : Long.MAX_VALUE);
            volume = (int) integer(m, "volume", -1, 0, 100);
            shuffle = Json.flag(m, "shuffle", false);
            String rep = Json.string(m, "repeat", "off");
            repeat = "track".equals(rep) || "context".equals(rep) ? rep : "off";
            List<Result> found = new ArrayList<>();
            for (Map<String, Object> item : items(m, "results")) {
                String uri = Json.string(item, "uri", null);
                found.add(new Result(isSpotifyUri(uri) ? uri : null, text(item, "title"), text(item, "subtitle")));
            }
            results = Collections.unmodifiableList(found);
            List<Device> devs = new ArrayList<>();
            for (Map<String, Object> item : items(m, "devices")) {
                devs.add(new Device(argument(item, "id"), text(item, "name"), Json.flag(item, "active", false)));
            }
            devices = Collections.unmodifiableList(devs);
            error = text(m, "error");
            this.receivedNanos = receivedNanos;
        }

        public static final Spotify NONE = new Spotify(Collections.emptyMap(), 0);

        public static Spotify read(Map<String, Object> m, long receivedNanos) {
            return new Spotify(m, receivedNanos);
        }

        /** Where playback is now: the reported position, advanced by the time since, while playing. */
        public long positionAt(long nowNanos) {
            long position = positionMs;
            if (playing && receivedNanos != 0) {
                position += Math.max(0, (nowNanos - receivedNanos) / 1_000_000L);
            }
            return durationMs > 0 ? Math.min(position, durationMs) : position;
        }

        public boolean connected() {
            return !"none".equals(source);
        }
    }

    // ------------------------------------------------------------------ browser

    public record Site(String name, String title) {
    }

    public static final class Browser {
        public final List<Site> sites;
        /** The open site's name, or null. */
        public final String open;
        public final String host;
        public final boolean loading;
        public final String title;
        public final String error;
        /** What the launcher says about being signed in (text; "" for nothing). */
        public final String signedInHint;

        private Browser(Map<String, Object> m) {
            List<Site> list = new ArrayList<>();
            for (Map<String, Object> item : items(m, "sites")) {
                String name = argument(item, "name");
                if (name != null) {
                    String title = text(item, "title");
                    list.add(new Site(name, title.isEmpty() ? Text.clean(name, MAX_TEXT) : title));
                }
            }
            sites = Collections.unmodifiableList(list);
            String opened = Json.string(m, "open", null);
            open = opened != null && Commands.isArgument(opened) ? opened : null;
            host = text(m, "host");
            loading = Json.flag(m, "loading", false);
            title = text(m, "title");
            error = text(m, "error");
            Object hint = m.get("signed_in_hint");
            if (hint instanceof Boolean) {
                signedInHint = (Boolean) hint ? "" : "Not signed in";
            } else {
                signedInHint = text(m, "signed_in_hint");
            }
        }

        public static final Browser NONE = new Browser(Collections.emptyMap());

        public static Browser read(Map<String, Object> m) {
            return new Browser(m);
        }

        public boolean hasSite(String name) {
            for (Site site : sites) {
                if (site.name().equals(name)) {
                    return true;
                }
            }
            return false;
        }
    }

    // ------------------------------------------------------------------ clips

    public record Clip(String id, String name, String created, long seconds, long bytes) {
    }

    public static final class Clips {
        public final boolean recording;
        public final int bufferSeconds;
        public final List<Clip> clips;
        public final String folder;
        public final String error;

        private Clips(Map<String, Object> m) {
            recording = Json.flag(m, "recording", false);
            bufferSeconds = (int) integer(m, "buffer_seconds", 0, 0, 24 * 3600);
            List<Clip> list = new ArrayList<>();
            for (Map<String, Object> item : items(m, "clips")) {
                String name = text(item, "name");
                String id = argument(item, "id");
                list.add(new Clip(id, name.isEmpty() && id != null ? Text.clean(id, MAX_TEXT) : name,
                        when(item.get("created")), integer(item, "seconds", 0, 0, 24 * 3600),
                        integer(item, "bytes", -1, -1, Long.MAX_VALUE / 2)));
            }
            clips = Collections.unmodifiableList(list);
            folder = text(m, "folder");
            error = text(m, "error");
        }

        public static final Clips NONE = new Clips(Collections.emptyMap());

        public static Clips read(Map<String, Object> m) {
            return new Clips(m);
        }
    }

    // ------------------------------------------------------------------ realms

    public record Prompt(String world, String address, long expiresInSeconds, int attemptsLeft) {
    }

    public static final class Realms {
        /** "code", "totp" or "off". */
        public final String mode;
        /** The question waiting for an answer, or null. */
        public final Prompt prompt;
        /** "approved", "refused", "expired", or null. */
        public final String last;
        public final long receivedNanos;

        private Realms(Map<String, Object> m, long receivedNanos) {
            String md = Json.string(m, "mode", "off");
            mode = "code".equals(md) || "totp".equals(md) ? md : "off";
            Map<String, Object> p = Json.child(m, "prompt");
            prompt = p == null ? null
                    : new Prompt(text(p, "world"), text(p, "address"), integer(p, "expires_in", 0, 0, 24 * 3600),
                            (int) integer(p, "attempts_left", 0, 0, 1000));
            String result = Json.string(m, "last", null);
            last = "approved".equals(result) || "refused".equals(result) || "expired".equals(result) ? result : null;
            this.receivedNanos = receivedNanos;
        }

        public static final Realms NONE = new Realms(Collections.emptyMap(), 0);

        public static Realms read(Map<String, Object> m, long receivedNanos) {
            return new Realms(m, receivedNanos);
        }

        /** Seconds left to answer the prompt at {@code nowNanos} (0 when expired or none). */
        public long secondsLeft(long nowNanos) {
            if (prompt == null) {
                return 0;
            }
            long elapsed = receivedNanos == 0 ? 0 : Math.max(0, (nowNanos - receivedNanos) / 1_000_000_000L);
            return Math.max(0, prompt.expiresInSeconds() - elapsed);
        }
    }

    // ------------------------------------------------------------------ settings

    public static final class Settings {
        public final boolean sync;
        public final String lastSync;
        public final String keys;
        public final String instances;
        public final List<String> instanceNames;
        public final String error;

        private Settings(Map<String, Object> m) {
            sync = Json.flag(m, "sync", false);
            lastSync = when(m.get("last_sync"));
            keys = amount(m.get("keys"));
            instances = amount(m.get("instances"));
            instanceNames = names(m.get("instances"), 8);
            error = text(m, "error");
        }

        public static final Settings NONE = new Settings(Collections.emptyMap());

        public static Settings read(Map<String, Object> m) {
            return new Settings(m);
        }
    }
}
