package net.minecraft.client.gui;

import java.util.ArrayList;
import java.util.List;

/** Stand-in (see CompanionFakeGame.java): remembers what was drawn. */
public class GuiGraphicsExtractor {
    public final List<String> drawn = new ArrayList<>();
    /** Set by the test: the next call fails, as a changed renderer might. */
    public boolean broken;

    public void text(Font font, String text, int x, int y, int color, boolean shadow) {
        if (broken) {
            throw new IllegalStateException("the renderer has changed");
        }
        drawn.add(x + "," + y + "," + Integer.toHexString(color) + "," + shadow + ":" + text);
    }
}
