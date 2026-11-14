package bastion.companion;

import java.util.ArrayList;
import java.util.List;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Core;
import bastion.companion.platform.Glue;
import bastion.companion.platform.HomeAction;
import bastion.companion.platform.Host;
import bastion.companion.platform.Hosted;
import bastion.companion.platform.HudLayer;
import bastion.companion.platform.Keys;
import bastion.companion.platform.MenuBackground;
import bastion.companion.platform.Overlay;
import bastion.companion.platform.Pixels;
import bastion.companion.platform.Platforms;
import bastion.companion.platform.Texture;

/**
 * The platform's version-independent core (`Core`, `Host`) on a stand-in family (`Glue`)
 * whose "screens" are plain objects: what comes back when an overlay closes, the home screen
 * in place of the title screen (and the game's own title once asked for), the phone key's
 * edges, a failing overlay closed and reported once, the menu background and the HUD layer.
 * The real families are checked in the game (`tests/companion_game.rs`); this is the same
 * logic on every `cargo test`. Prints `ok` or throws.
 */
public final class PlatformCoreCheck {
    /** A game screen of the stand-in. */
    static class Screen {
        final String name;

        Screen(String name) {
            this.name = name;
        }

        @Override
        public String toString() {
            return name;
        }
    }

    static final class Title extends Screen {
        Title() {
            super("title");
        }
    }

    /** The stand-in's host screen. */
    static final class HostScreen extends Screen implements Hosted {
        final Host host;

        HostScreen(Host host) {
            super("host");
            this.host = host;
        }

        @Override
        public Host host() {
            return host;
        }
    }

    /** A family with one screen slot, a key table, and a log of what was asked. */
    static final class FakeGlue extends Glue {
        Object screen;
        boolean player;
        boolean keyR;
        final List<String> asked = new ArrayList<>();

        @Override
        public String family() {
            return "fake";
        }

        @Override
        public Object screen() {
            return screen;
        }

        /**
         * As the game does: the hook decides; the old screen is told it is removed before the
         * new one is recorded; the new one is set up.
         */
        @Override
        public void setScreen(Object next) {
            Object instead = Core.get().onSetScreen(next);
            if (instead != next) {
                setScreen(instead);
                return;
            }
            Object old = screen;
            if (old instanceof Hosted && old != next) {
                ((Hosted) old).host().removed();
            }
            screen = next;
            if (next instanceof Hosted) {
                ((Hosted) next).host().init(320, 240);
            }
        }

        @Override
        public Object newHostScreen(Host host) {
            return new HostScreen(host);
        }

        @Override
        public boolean isTitleScreen(Object s) {
            return s instanceof Title;
        }

        @Override
        public Object newTitleScreen() {
            return new Title();
        }

        @Override
        public boolean hasPlayer() {
            return player;
        }

        @Override
        public boolean loading() {
            return false;
        }

        @Override
        public boolean demo() {
            return true;
        }

        @Override
        public double guiScale() {
            return 2.0;
        }

        @Override
        public int keyCode(String name) {
            return name.equals("key.keyboard.r") ? 82 : -1;
        }

        @Override
        public String keyLabel(String name) {
            return "R";
        }

        @Override
        public boolean keyDown(int code) {
            return code == 82 && keyR;
        }

        @Override
        public boolean available(HomeAction action) {
            return action != HomeAction.MULTIPLAYER;
        }

        @Override
        public void perform(HomeAction action, Object parent) {
            asked.add(action + " from " + parent);
            screen = new Screen(action.name().toLowerCase());
        }

        @Override
        public String translate(String key) {
            return key.equals("menu.singleplayer") ? "Einzelspieler" : key;
        }

        @Override
        public void clickSound() {
            asked.add("click");
        }

        @Override
        public void execute(Runnable task) {
            task.run();
        }

        @Override
        public Texture texture(Pixels pixels) {
            return null;
        }
    }

    static final class Recorder implements Overlay {
        final String name;
        int opened;
        int closed;
        int frames;
        int keys;
        boolean fail;

        Recorder(String name) {
            this.name = name;
        }

        @Override
        public void opened(int width, int height) {
            opened++;
        }

        @Override
        public void render(Canvas canvas, int mouseX, int mouseY, float partialTick) {
            if (fail) {
                throw new IllegalStateException("broken overlay");
            }
            frames++;
        }

        @Override
        public boolean keyPressed(int key, int modifiers) {
            keys++;
            return key == 'A';
        }

        @Override
        public void closed() {
            closed++;
        }
    }

    private static void must(boolean condition, String what) {
        if (!condition) {
            throw new AssertionError(what);
        }
    }

    public static void main(String[] args) {
        FakeGlue glue = new FakeGlue();
        Core core = Core.start(glue, "test", Config.parse("phone.key=key.keyboard.r\n"));
        must(Platforms.current() == core && Platforms.live(), "installed");

        // An overlay over a game screen; closing gives the screen back.
        Screen options = new Screen("options");
        glue.screen = options;
        Recorder phone = new Recorder("phone");
        core.open(phone);
        must(core.isOpen(phone) && phone.opened == 1, "opened");
        must(glue.screen instanceof HostScreen, "a host screen is open");
        core.open(phone);
        must(phone.opened == 1, "opening again does nothing");
        core.close(phone);
        must(glue.screen == options && phone.closed == 1 && !core.isOpen(phone), "closed, the screen underneath back");

        // Over the world: playing, the phone key's edge opens it; held down it does not repeat.
        glue.player = true;
        glue.screen = null;
        int[] presses = {0};
        core.setPhoneKey(() -> {
            presses[0]++;
            if (core.isOpen(phone)) {
                core.close(phone);
            } else {
                core.open(phone);
            }
        });
        must(core.playing(), "playing");
        core.onFrame();
        glue.keyR = true;
        core.onFrame();
        core.onFrame();
        must(presses[0] == 1 && core.isOpen(phone), "one press opened the phone: " + presses[0]);
        // The key, released and pressed while the phone is open: the overlay sees it first
        // and does not use it, so it puts the phone away.
        glue.keyR = false;
        core.onFrame();
        HostScreen host = (HostScreen) glue.screen;
        host.host().keyPressed(82, 'R', 0);
        must(presses[0] == 2 && !core.isOpen(phone) && glue.screen == null, "the same key put it away");
        // A key the overlay uses does not toggle; Escape closes.
        core.open(phone);
        host = (HostScreen) glue.screen;
        host.host().keyPressed('A', 'A', 0);
        must(core.isOpen(phone), "a used key does not close");
        host.host().keyPressed(Keys.ESCAPE, Keys.ESCAPE, 0);
        must(!core.isOpen(phone), "Escape closes");
        // Put away with the key while it is held: the next frames, playing again with the key
        // still down, do not take it out again.
        core.open(phone);
        glue.keyR = true;
        core.onFrame();
        ((HostScreen) glue.screen).host().keyPressed(82, 'R', 0);
        must(!core.isOpen(phone), "put away");
        core.onFrame();
        core.onFrame();
        must(!core.isOpen(phone), "a held key does not take it out again");
        glue.keyR = false;
        core.onFrame();

        // The home screen in place of the title screen; once the classic title is asked for,
        // the game's own title until a world has been entered.
        glue.player = false;
        glue.screen = null;
        Recorder home = new Recorder("home");
        core.setHome(home);
        glue.setScreen(new Title());
        must(glue.screen instanceof HostScreen && ((HostScreen) glue.screen).host().home(), "home in place of the title");
        must(core.titles() == 1, "counted");
        core.perform(HomeAction.SINGLEPLAYER);
        must(glue.asked.get(0).equals("SINGLEPLAYER from host"), glue.asked.toString());
        must(core.label(HomeAction.SINGLEPLAYER).equals("Einzelspieler"), "the game's caption");
        must(core.label(HomeAction.MODS).equals("Mods"), "an English caption where the game has none");
        must(core.available(HomeAction.PLAY_DEMO) && !core.available(HomeAction.MULTIPLAYER), "available");
        core.perform(HomeAction.CLASSIC_TITLE);
        must(glue.screen instanceof Title, "the classic title");
        glue.setScreen(new Title());
        must(glue.screen instanceof Title, "stays classic");
        glue.player = true;
        core.onFrame();
        glue.player = false;
        glue.setScreen(new Title());
        must(glue.screen instanceof HostScreen, "home again after a world");
        // Escape does not close the home screen.
        ((HostScreen) glue.screen).host().keyPressed(Keys.ESCAPE, Keys.ESCAPE, 0);
        must(glue.screen instanceof HostScreen, "home stays");

        // The game replaces an open overlay with a screen of its own (death, a disconnect):
        // from the next frame on the overlay is closed, and told so once.
        glue.player = true;
        glue.screen = null;
        core.open(phone);
        int closedBefore = phone.closed;
        glue.setScreen(new Screen("death"));
        core.onFrame();
        must(!core.isOpen(phone) && phone.closed == closedBefore + 1, "replaced by the game: closed");
        core.onFrame();
        must(phone.closed == closedBefore + 1, "told once");
        glue.player = false;

        // An overlay that throws is closed and reported once; the game is not affected.
        Recorder broken = new Recorder("broken");
        glue.screen = options;
        core.open(broken);
        broken.fail = true;
        HostScreen brokenHost = (HostScreen) glue.screen;
        brokenHost.host().render(null, 0, 0, 0f);
        brokenHost.host().render(null, 0, 0, 0f);
        must(!core.isOpen(broken) && glue.screen == options && broken.closed == 1, "closed after failing");

        // The menu background: the painter for the game's own screens, never for ours.
        int[] painted = {0};
        core.setMenuBackground((canvas, inWorld) -> {
            painted[0]++;
            return true;
        });
        must(core.onBackground(null, options) && painted[0] == 1, "a game screen's background painted");
        core.open(phone);
        must(core.onBackground(null, glue.screen) && painted[0] == 1, "our own: no game background, no painter");
        core.close(phone);
        MenuBackground failing = (canvas, inWorld) -> {
            throw new IllegalStateException("broken painter");
        };
        core.setMenuBackground(failing);
        must(!core.onBackground(null, options), "a failing painter: the game's background");
        must(!core.onBackground(null, options), "and it is off from then on");

        // The HUD layer: called, and off after it fails.
        int[] hud = {0};
        core.setHud((canvas, partial, screenOpen) -> hud[0]++);
        core.onHud(null, false);
        HudLayer bad = (canvas, partial, screenOpen) -> {
            throw new IllegalStateException("broken layer");
        };
        core.setHud(bad);
        core.onHud(null, false);
        core.onHud(null, false);
        must(hud[0] == 1 && core.hudFrames() == 3, "HUD frames counted, the failing layer dropped");
        System.out.println("ok");
    }
}
