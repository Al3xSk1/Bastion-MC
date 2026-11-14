package net.minecraft.client.renderer;

import bastion.companion.Hooks;
import net.minecraft.client.Minecraft;
import net.minecraft.client.renderer.state.LightmapRenderState;

/** Stand-in (see CompanionFakeGame.java and Camera.java). */
public class LightmapRenderStateExtractor {
    /** The pulse of the Darkness effect, which the game subtracts. */
    public float darkness;

    public void extract(LightmapRenderState state, float partialTicks) {
        double option = Minecraft.getInstance().options.gamma().get();
        state.brightness = Math.max(0.0f, (float) option - darkness);
        Hooks.lightmap(state);
    }

    private void handler$fake$bastion$fullbright() {
    }
}
