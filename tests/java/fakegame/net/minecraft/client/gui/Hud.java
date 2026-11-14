package net.minecraft.client.gui;

import bastion.companion.Hooks;
import net.minecraft.client.DeltaTracker;

/** Stand-in (see CompanionFakeGame.java and Camera.java). */
public class Hud {
    public boolean hidden;

    public boolean isHidden() {
        return hidden;
    }

    public void extractRenderState(GuiGraphicsExtractor graphics, DeltaTracker deltaTracker) {
        Hooks.hud(this, graphics);
    }

    private void handler$fake$bastion$hudReadout() {
    }
}
