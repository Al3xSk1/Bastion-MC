package bastion.companion.platform.v26_2;

import bastion.companion.platform.Events;
import bastion.companion.platform.mojang.FamilyCanvas;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.Hud;
import net.minecraft.client.gui.screens.Screen;

/** What the family's mixins call. */
public final class Hooks {
    private Hooks() {
    }

    /** The HUD was extracted: the layer on top, unless the HUD is hidden (F1). */
    public static void hud(Object hud, GuiGraphicsExtractor graphics) {
        if (((Hud) hud).isHidden()) {
            return;
        }
        Minecraft game = Minecraft.getInstance();
        Events.hud(new FamilyCanvas(graphics, game.getWindow().getGuiScaledWidth(), game.getWindow().getGuiScaledHeight()),
                game.gui.screen() != null);
    }

    public static boolean background(GuiGraphicsExtractor graphics, Object screen) {
        Screen s = (Screen) screen;
        return Events.background(new FamilyCanvas(graphics, s.width, s.height), screen);
    }
}
