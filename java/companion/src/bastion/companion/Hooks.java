package bastion.companion;

import bastion.companion.feature.Fullbright;
import bastion.companion.feature.HudReadout;
import bastion.companion.feature.Zoom;

/**
 * Where the game calls into the mod. Each method is the body of one injected callback (see
 * the `mixin` package) and keeps the same contract: cheap when its feature is off, and no
 * exception ever leaves it — a failing feature is switched off and the game's own value or
 * behaviour is used instead.
 */
public final class Hooks {
    public static final Zoom ZOOM = new Zoom();
    public static final HudReadout HUD = new HudReadout();
    public static final Fullbright FULLBRIGHT = new Fullbright();

    /** The feature registry: everything the launcher can switch on. */
    static final Feature[] ALL = {ZOOM, HUD, FULLBRIGHT};

    private Hooks() {
    }

    private static void off(Feature feature, String where, Throwable failure) {
        feature.failed(where, failure);
    }

    /** {@code Camera.calculateFov}: the world's field of view, in degrees. */
    public static float fov(float computed) {
        if (ZOOM.live()) {
            try {
                return ZOOM.fov(computed);
            } catch (Throwable failure) {
                off(ZOOM, "the field-of-view hook", failure);
            }
        }
        return computed;
    }

    /** Start of {@code MouseHandler.turnPlayer}, with the game's MouseHandler. */
    public static void turn(Object mouse) {
        if (ZOOM.live()) {
            try {
                ZOOM.turn(mouse);
            } catch (Throwable failure) {
                off(ZOOM, "the mouse-movement hook", failure);
            }
        }
    }

    /** {@code MouseHandler.onScroll}: true when the wheel movement was used up here. */
    public static boolean wheel(double vertical) {
        if (ZOOM.live()) {
            try {
                return ZOOM.wheel(vertical);
            } catch (Throwable failure) {
                off(ZOOM, "the mouse-wheel hook", failure);
            }
        }
        return false;
    }

    /** End of {@code Hud.extractRenderState}. */
    public static void hud(Object hud, Object graphics) {
        if (HUD.live()) {
            try {
                HUD.draw(hud, graphics);
            } catch (Throwable failure) {
                off(HUD, "the HUD hook", failure);
            }
        }
    }

    /** End of {@code LightmapRenderStateExtractor.extract}, when the light map was recomputed. */
    public static void lightmap(Object state) {
        if (FULLBRIGHT.live()) {
            try {
                FULLBRIGHT.apply(state);
            } catch (Throwable failure) {
                off(FULLBRIGHT, "the light-map hook", failure);
            }
        }
    }
}
