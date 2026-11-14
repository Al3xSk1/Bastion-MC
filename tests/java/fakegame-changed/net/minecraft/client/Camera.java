package net.minecraft.client;

/**
 * The stand-in camera of a game version that has moved on: the method the zoom hook was
 * written for takes another argument now, and no callback was injected.
 */
public class Camera {
    public float base = 70.0f;

    private float calculateFov(float partialTicks, boolean firstPerson) {
        return base;
    }

    public float frame() {
        return calculateFov(1.0f, true);
    }
}
