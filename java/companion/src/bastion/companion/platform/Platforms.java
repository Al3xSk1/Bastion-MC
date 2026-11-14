package bastion.companion.platform;

/**
 * Which {@link Platform} this game gets: the adapter of the family its version belongs to,
 * chosen once when the mod starts. Where no adapter fits (a version outside every family, or
 * an adapter that failed to start), {@link #current()} is an inert platform that opens
 * nothing and draws nothing, so that code built on the platform need not check.
 */
public final class Platforms {
    private static volatile Platform current = new Inert("none", "no adapter");
    private static volatile Link link = new Offline();

    private Platforms() {
    }

    /** The platform of this game. Never null. */
    public static Platform current() {
        return current;
    }

    /** The line to the launcher. Never null: without a launcher it says it is not connected. */
    public static Link link() {
        return link;
    }

    /** Called once by the mod's start-up with the launcher link. */
    public static void installLink(Link line) {
        if (line != null) {
            link = line;
        }
    }

    /** Whether a real adapter is in place (false: {@link #current()} does nothing). */
    public static boolean live() {
        return !(current instanceof Inert);
    }

    /** Called once by the mod's start-up with the adapter it chose. */
    public static void install(Platform platform) {
        if (platform != null) {
            current = platform;
        }
    }

    /** A platform that does nothing: for versions without an adapter. */
    public static final class Inert implements Platform {
        private final String version;
        private final String why;

        public Inert(String version, String why) {
            this.version = version;
            this.why = why;
        }

        /** Why there is no adapter. */
        public String why() {
            return why;
        }

        @Override
        public String minecraftVersion() {
            return version;
        }

        @Override
        public String family() {
            return "none";
        }

        @Override
        public void open(Overlay overlay) {
        }

        @Override
        public void close(Overlay overlay) {
        }

        @Override
        public boolean isOpen(Overlay overlay) {
            return false;
        }

        @Override
        public boolean playing() {
            return false;
        }

        @Override
        public boolean inWorld() {
            return false;
        }

        @Override
        public void setHome(Overlay home) {
        }

        @Override
        public boolean available(HomeAction action) {
            return false;
        }

        @Override
        public void perform(HomeAction action) {
        }

        @Override
        public String label(HomeAction action) {
            return HomeLabels.english(action);
        }

        @Override
        public void setMenuBackground(MenuBackground painter) {
        }

        @Override
        public double guiScale() {
            return 1.0;
        }

        @Override
        public boolean demo() {
            return false;
        }

        @Override
        public void setHud(HudLayer layer) {
        }

        @Override
        public void setPhoneKey(Runnable onPress) {
        }

        @Override
        public String phoneKeyName() {
            return "";
        }

        @Override
        public Texture texture(Pixels pixels) {
            return null;
        }

        @Override
        public void clickSound() {
        }

        @Override
        public void runOnRenderThread(Runnable task) {
        }
    }

    /** A link that is never connected: for a game without a launcher channel. */
    public static final class Offline implements Link {
        @Override
        public boolean connected() {
            return false;
        }

        @Override
        public boolean send(String command) {
            return false;
        }

        @Override
        public State state(String app) {
            return null;
        }

        @Override
        public Picture picture(String app, int slot) {
            return null;
        }

        @Override
        public Notice nextNotice() {
            return null;
        }

        @Override
        public long changes() {
            return 0;
        }

        @Override
        public void setListener(Listener listener) {
            if (listener != null) {
                listener.link(false);
            }
        }
    }
}
