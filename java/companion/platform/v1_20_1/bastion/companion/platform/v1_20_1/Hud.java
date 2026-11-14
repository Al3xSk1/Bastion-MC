package bastion.companion.platform.v1_20_1;

import bastion.companion.platform.Events;
import net.minecraft.class_310;
import net.minecraft.class_332;
import net.minecraft.class_437;

/** What the family's mixins call: canvases for the HUD and for a screen's background. */
public final class Hud {
    private Hud() {
    }

    /** The HUD was drawn: the layer on top, unless the HUD is hidden (F1). */
    public static void draw(class_332 graphics) {
        class_310 game = class_310.method_1551();
        if (game.field_1690.field_1842) {
            return;
        }
        int w = game.method_22683().method_4486();
        int h = game.method_22683().method_4502();
        Events.hud(new FamilyCanvas(graphics, w, h), game.field_1755 != null);
    }

    /** A screen's background is due: true when it was drawn by the mod instead. */
    public static boolean background(class_332 graphics, Object screen) {
        class_437 s = (class_437) screen;
        return Events.background(new FamilyCanvas(graphics, s.field_22789, s.field_22790), screen);
    }
}
