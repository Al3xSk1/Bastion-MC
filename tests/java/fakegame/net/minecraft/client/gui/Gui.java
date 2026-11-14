package net.minecraft.client.gui;

import net.minecraft.client.gui.screens.Overlay;
import net.minecraft.client.gui.screens.Screen;

/** Stand-in (see CompanionFakeGame.java). */
public class Gui {
    public final Hud hud = new Hud();
    public Screen open;
    public Overlay loading;

    public Screen screen() {
        return open;
    }

    public Overlay overlay() {
        return loading;
    }
}
