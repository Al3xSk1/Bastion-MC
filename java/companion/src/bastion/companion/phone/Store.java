package bastion.companion.phone;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Set;

import bastion.companion.Log;
import bastion.companion.platform.Json;
import bastion.companion.platform.Link;
import bastion.companion.platform.Pixels;
import bastion.companion.platform.Platform;
import bastion.companion.platform.Texture;
import bastion.companion.ui.Text;
import bastion.companion.ui.Clock;

/**
 * What the launcher has told the phone: whether the link is up, each app's newest state, the
 * newest picture per app and slot (as textures the game can draw), and the last notices.
 * Filled by the link's listener on the render thread; read by the screens on the same thread.
 *
 * A state that cannot be read (not JSON, not an object) leaves the app's previous state in
 * place and is logged once per app; whatever is read is bounded and cleaned by
 * {@link AppStates}.
 */
public final class Store implements Link.Listener {
    /** Notices kept for the phone's list. */
    public static final int MAX_NOTICES = 20;
    /** Pictures kept per app (clips: one per thumbnail slot). */
    public static final int MAX_SLOTS = 33;

    /** One notice as shown. */
    public record Notice(String app, String text, long nanos) {
    }

    private final Platform platform;
    private boolean connected;
    private AppStates.PhoneInfo info = AppStates.PhoneInfo.NONE;
    private AppStates.Spotify spotify = AppStates.Spotify.NONE;
    private AppStates.Browser browser = AppStates.Browser.NONE;
    private AppStates.Clips clips = AppStates.Clips.NONE;
    private AppStates.Realms realms = AppStates.Realms.NONE;
    private AppStates.Settings settings = AppStates.Settings.NONE;
    private final Map<String, Long> serials = new HashMap<>();
    private final Map<String, Texture> textures = new HashMap<>();
    private final Map<String, Long> pictureSerials = new HashMap<>();
    private final ArrayDeque<Notice> notices = new ArrayDeque<>();
    private int unread;
    private final Set<String> badStates = new HashSet<>();
    private long pictureCount;
    private long serial;

    public Store(Platform platform) {
        this.platform = platform;
    }

    // ------------------------------------------------------------------ Link.Listener

    @Override
    public void link(boolean up) {
        connected = up;
        serial++;
    }

    @Override
    public void state(String app, String json) {
        Map<String, Object> m;
        try {
            m = Json.object(json);
        } catch (Json.Malformed e) {
            if (badStates.add(app)) {
                Log.warn("phone: a " + Text.clean(app, 16) + " state could not be read (" + e.getMessage() + ")");
            }
            return;
        }
        long now = Clock.now();
        switch (app) {
            case "phone":
                info = AppStates.PhoneInfo.read(m);
                break;
            case "spotify":
                spotify = AppStates.Spotify.read(m, now);
                break;
            case "browser":
                browser = AppStates.Browser.read(m);
                break;
            case "clips":
                AppStates.Clips previous = clips;
                clips = AppStates.Clips.read(m);
                if (!sameIds(previous, clips)) {
                    // Thumbnail slots are positions in the last list: a new list, new slots.
                    dropPictures("clips");
                }
                break;
            case "realms":
                realms = AppStates.Realms.read(m, now);
                break;
            case "settings":
                settings = AppStates.Settings.read(m);
                break;
            default:
                return;
        }
        serials.merge(app, 1L, Long::sum);
        serial++;
    }

    private static boolean sameIds(AppStates.Clips a, AppStates.Clips b) {
        if (a.clips.size() != b.clips.size()) {
            return false;
        }
        for (int i = 0; i < a.clips.size(); i++) {
            String x = a.clips.get(i).id();
            String y = b.clips.get(i).id();
            if (x == null ? y != null : !x.equals(y)) {
                return false;
            }
        }
        return true;
    }

    @Override
    public void notice(String app, String text) {
        String clean = Text.clean(text, 200);
        if (clean.isBlank()) {
            return;
        }
        if (notices.size() == MAX_NOTICES) {
            notices.pollLast();
        }
        notices.addFirst(new Notice(Text.clean(app, 16), clean, Clock.now()));
        unread = Math.min(unread + 1, 99);
        serial++;
    }

    @Override
    public void frame(String app, int slot, Pixels pixels) {
        if (pixels == null || slot < 0 || slot >= MAX_SLOTS) {
            return;
        }
        String key = app + "/" + slot;
        Texture texture = textures.get(key);
        if (texture != null && texture.width() == pixels.width() && texture.height() == pixels.height()) {
            texture.replace(pixels);
        } else {
            if (texture != null) {
                texture.close();
            }
            texture = platform.texture(pixels);
            if (texture == null) {
                textures.remove(key);
                return;
            }
            textures.put(key, texture);
        }
        pictureSerials.put(key, ++pictureCount);
        serial++;
    }

    // ------------------------------------------------------------------ reading

    public boolean connected() {
        return connected;
    }

    public AppStates.PhoneInfo info() {
        return info;
    }

    public AppStates.Spotify spotify() {
        return spotify;
    }

    public AppStates.Browser browser() {
        return browser;
    }

    public AppStates.Clips clips() {
        return clips;
    }

    public AppStates.Realms realms() {
        return realms;
    }

    public AppStates.Settings settings() {
        return settings;
    }

    /** How many states of {@code app} have arrived. */
    public long serial(String app) {
        return serials.getOrDefault(app, 0L);
    }

    /** Grows with everything that arrives. */
    public long serial() {
        return serial;
    }

    /** Whether the launcher announced {@code app} for this launch. */
    public boolean announced(String app) {
        return info.apps.contains(app);
    }

    /** The picture of {@code app} in {@code slot}, or null. */
    public Texture picture(String app, int slot) {
        return textures.get(app + "/" + slot);
    }

    /** A number that changes whenever that picture is replaced (0: none). */
    public long pictureSerial(String app, int slot) {
        return pictureSerials.getOrDefault(app + "/" + slot, 0L);
    }

    /** Closes and forgets every picture of {@code app}. */
    public void dropPictures(String app) {
        String prefix = app + "/";
        Iterator<Map.Entry<String, Texture>> it = textures.entrySet().iterator();
        while (it.hasNext()) {
            Map.Entry<String, Texture> entry = it.next();
            if (entry.getKey().startsWith(prefix)) {
                entry.getValue().close();
                it.remove();
                pictureSerials.remove(entry.getKey());
            }
        }
    }

    /** The newest notices first. */
    public List<Notice> notices() {
        return new ArrayList<>(notices);
    }

    /** The newest notice, or null. */
    public Notice latest() {
        return notices.peekFirst();
    }

    public int unread() {
        return unread;
    }

    public void markRead() {
        unread = 0;
    }

    /** Releases every texture (the phone is going away for good). */
    public void close() {
        for (Texture texture : textures.values()) {
            texture.close();
        }
        textures.clear();
        pictureSerials.clear();
    }
}
