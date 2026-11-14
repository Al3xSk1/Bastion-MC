import bastion.companion.Companion;
import bastion.companion.Hooks;
import com.mojang.blaze3d.platform.InputConstants;
import net.minecraft.client.Camera;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.screens.Overlay;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.client.renderer.LightmapRenderStateExtractor;
import net.minecraft.client.renderer.state.LightmapRenderState;

/**
 * A few scripted seconds of a stand-in game (`tests/java/fakegame`): classes with the names
 * and members the companion mod looks up, which call the mod's hooks where Mixin would have
 * injected them. It lets `tests/companion.rs` exercise the mod's own logic — the lookups
 * succeeding, the zoom's arithmetic, what the HUD writes, a hook that starts failing — on
 * every test run, without Minecraft. It says nothing about the real game; that is
 * `tests/companion_game.rs`.
 *
 * Argument: the number of the zoom key in the stand-in's keyboard state. Prints `name=value`
 * lines; `tests/companion.rs` knows what they must be.
 */
public final class CompanionFakeGame {
    private static long clock = 5_000_000_000L;

    private static void show(String name, Object value) {
        System.out.println(name + "=" + value);
    }

    /** That many 16 ms frames on the scripted clock; the last frame's field of view. */
    private static float frames(Camera camera, int count) {
        float last = 0.0f;
        for (int i = 0; i < count; i++) {
            clock += 16_000_000L;
            Hooks.ZOOM.testNanos = clock;
            last = camera.frame();
        }
        return last;
    }

    public static void main(String[] args) throws Exception {
        int key = Integer.parseInt(args[0]);
        Companion.start();
        show("live", Hooks.ZOOM.live() + "," + Hooks.HUD.live() + "," + Hooks.FULLBRIGHT.live());
        Minecraft game = Minecraft.getInstance();
        Camera camera = new Camera();

        // Zoom. The key is down from the start; what changes is whether it means "zoom".
        InputConstants.DOWN[key] = true;
        show("fov_no_world", frames(camera, 60));
        game.player = new LocalPlayer();
        game.gui.open = new Screen();
        show("fov_screen_open", frames(camera, 60));
        game.gui.open = null;
        game.gui.loading = new Overlay();
        show("fov_loading", frames(camera, 60));
        game.gui.loading = null;
        show("fov_first_frame", frames(camera, 1));
        show("fov_zoomed", frames(camera, 60));
        game.mouseHandler.move(100.0, -40.0);
        show("turned_zoomed", game.mouseHandler.turnedX + "/" + game.mouseHandler.turnedY);
        game.mouseHandler.onScroll(1L, 0.0, 1.0);
        game.mouseHandler.onScroll(1L, 0.0, 1.0);
        show("fov_two_notches", frames(camera, 60));
        for (int i = 0; i < 200; i++) {
            game.mouseHandler.onScroll(1L, 0.0, 1.0);
        }
        show("fov_wheel_limit_in", frames(camera, 60));
        for (int i = 0; i < 400; i++) {
            game.mouseHandler.onScroll(1L, 0.0, -1.0);
        }
        show("fov_wheel_limit_out", frames(camera, 60));
        show("scrolled_while_zoomed", game.mouseHandler.scrolled);
        InputConstants.DOWN[key] = false;
        frames(camera, 60);
        InputConstants.DOWN[key] = true;
        show("fov_pressed_again", frames(camera, 60));
        InputConstants.DOWN[key] = false;
        show("fov_first_frame_out", frames(camera, 1));
        show("fov_released", frames(camera, 60));
        game.mouseHandler.turnedX = 0.0;
        game.mouseHandler.turnedY = 0.0;
        game.mouseHandler.move(100.0, -40.0);
        show("turned_released", game.mouseHandler.turnedX + "/" + game.mouseHandler.turnedY);
        game.mouseHandler.onScroll(1L, 0.0, 1.0);
        show("scrolled_released", game.mouseHandler.scrolled);
        // A typed letter: with a screen open nothing zooms, and the wheel is the screen's.
        InputConstants.DOWN[key] = true;
        game.gui.open = new Screen();
        show("fov_typing", frames(camera, 60));
        game.gui.open = null;
        InputConstants.DOWN[key] = false;
        frames(camera, 60);
        Hooks.ZOOM.testNanos = null;

        // HUD readout. (Its text is rebuilt at most every 100 ms of real time.)
        game.fps = 144;
        game.player.x = 12.34;
        game.player.y = 64.0;
        game.player.z = -7.89;
        game.player.yaw = -90.5f;
        game.player.pitch = 12.25f;
        GuiGraphicsExtractor graphics = new GuiGraphicsExtractor();
        game.gui.hud.extractRenderState(graphics, null);
        show("hud", String.join(" | ", graphics.drawn));
        Thread.sleep(150);
        game.reducedInfo = true;
        graphics.drawn.clear();
        game.gui.hud.extractRenderState(graphics, null);
        show("hud_reduced_info", String.join(" | ", graphics.drawn));
        game.reducedInfo = false;
        game.gui.hud.hidden = true;
        graphics.drawn.clear();
        game.gui.hud.extractRenderState(graphics, null);
        show("hud_hidden", graphics.drawn.size());
        game.gui.hud.hidden = false;
        game.debug.shown = true;
        game.gui.hud.extractRenderState(graphics, null);
        show("hud_debug_screen", graphics.drawn.size());
        game.debug.shown = false;

        // Fullbright.
        LightmapRenderStateExtractor extractor = new LightmapRenderStateExtractor();
        LightmapRenderState state = new LightmapRenderState();
        extractor.extract(state, 1.0f);
        show("brightness", state.brightness);
        extractor.darkness = 0.3f;
        extractor.extract(state, 1.0f);
        show("brightness_darkness", state.brightness);
        game.options.brightness.value = 20.0;
        extractor.extract(state, 1.0f);
        show("brightness_option_above", state.brightness);

        // A hook that starts failing: its feature goes, the others and the game stay.
        graphics.broken = true;
        for (int frame = 0; frame < 500; frame++) {
            game.gui.hud.extractRenderState(graphics, null);
        }
        show("live_after_failure", Hooks.ZOOM.live() + "," + Hooks.HUD.live() + "," + Hooks.FULLBRIGHT.live());
        System.out.flush();
        // Until the test has read the channel (or given up).
        while (System.in.read() != -1) {
            continue;
        }
        System.out.println("done");
    }
}
