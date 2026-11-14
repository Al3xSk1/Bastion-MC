package bastion.companion.platform.v26_1;

import bastion.companion.platform.Events;
import bastion.companion.platform.mojang.FamilyCanvas;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.screens.Screen;

/** What the family's mixins call. */
public final class Hooks {
    private Hooks() {
    }

    /** The HUD was extracted: the layer on top, unless the HUD is hidden (F1). */
    public static void hud(GuiGraphicsExtractor graphics) {
        Minecraft game = Minecraft.getInstance();
        if (game.options.hideGui) {
            return;
        }
        Events.hud(new FamilyCanvas(graphics, game.getWindow().getGuiScaledWidth(), game.getWindow().getGuiScaledHeight()),
                game.screen != null);
    }

    public static boolean background(GuiGraphicsExtractor graphics, Object screen) {
        Screen s = (Screen) screen;
        return Events.background(new FamilyCanvas(graphics, s.width, s.height), screen);
    }
}
