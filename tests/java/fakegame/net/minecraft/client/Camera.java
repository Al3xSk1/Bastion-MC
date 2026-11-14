package net.minecraft.client;

import bastion.companion.Hooks;

/**
 * Stand-in (see CompanionFakeGame.java). Where Mixin would have injected the mod's callback
 * into the real method, this one calls the hook itself — and carries a method named like
 * the merged handler, which is what the mod looks for.
 */
public class Camera {
    public float base = 70.0f;

    private float calculateFov(float partialTicks) {
        return Hooks.fov(base);
    }

    /** What the game does once per frame. */
    public float frame() {
        return calculateFov(1.0f);
    }

    private void handler$fake$bastion$zoomFov() {
    }
}
