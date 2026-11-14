package bastion.companion.theme;

import bastion.companion.Feature;
import bastion.companion.Game;
import bastion.companion.platform.Canvas;
import bastion.companion.platform.MenuBackground;
import bastion.companion.platform.Platforms;
import bastion.companion.ui.Gfx;
import bastion.companion.ui.Rect;

/**
 * Darker backgrounds behind the game's own menus (switch {@code darkmenus}, on by default):
 * outside a world a flat gunmetal with a faint light from the top instead of the game's
 * textured background; in a world a dark veil through which the world stays faintly visible,
 * instead of the game's blur or dimming. Only the background is replaced — the menus
 * themselves, their lists and buttons, are the game's.
 */
public final class DarkMenus extends Feature implements MenuBackground {
    public DarkMenus() {
        super("darkmenus");
    }

    @Override
    protected void resolve(Game game) throws Throwable {
        if (!Platforms.live()) {
            throw new IllegalStateException(
                    "no platform adapter for Minecraft " + Platforms.current().minecraftVersion());
        }
        Platforms.current().setMenuBackground(this);
    }

    @Override
    public boolean paint(Canvas c, boolean inWorld) {
        paintBackground(c, inWorld);
        return true;
    }

    /** The background itself (also what the tests look at). */
    public static void paintBackground(Canvas c, boolean inWorld) {
        int w = c.width();
        int h = c.height();
        if (w <= 0 || h <= 0) {
            return;
        }
        if (inWorld) {
            c.fill(0, 0, w, h, Theme.SCRIM_WORLD);
            return;
        }
        c.fill(0, 0, w, h, Theme.VOID);
        Gfx.gradient(c, new Rect(0, 0, w, Math.max(1, h / 3)), Theme.PLATE, Theme.VOID, 3);
    }
}
