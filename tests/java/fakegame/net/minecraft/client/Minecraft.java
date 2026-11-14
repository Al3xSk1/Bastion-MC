package net.minecraft.client;

import net.minecraft.client.gui.Font;
import net.minecraft.client.gui.Gui;
import net.minecraft.client.gui.components.DebugScreenOverlay;
import net.minecraft.client.player.LocalPlayer;

/** Stand-in (see CompanionFakeGame.java). */
public class Minecraft {
    private static final Minecraft INSTANCE = new Minecraft();

    public final Gui gui = new Gui();
    public final Options options = new Options();
    public final Font font = new Font();
    public final MouseHandler mouseHandler = new MouseHandler();
    public final DebugScreenOverlay debug = new DebugScreenOverlay();
    public LocalPlayer player;
    public int fps = 60;
    public boolean reducedInfo;

    public static Minecraft getInstance() {
        return INSTANCE;
    }

    public int getFps() {
        return fps;
    }

    public boolean showOnlyReducedInfo() {
        return reducedInfo;
    }

    public DebugScreenOverlay getDebugOverlay() {
        return debug;
    }
}
