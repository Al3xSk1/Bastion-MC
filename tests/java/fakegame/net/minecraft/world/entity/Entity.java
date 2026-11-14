package net.minecraft.world.entity;

/** Stand-in (see CompanionFakeGame.java). */
public class Entity {
    public double x;
    public double y;
    public double z;
    public float yaw;
    public float pitch;

    public final double getX() {
        return x;
    }

    public final double getY() {
        return y;
    }

    public final double getZ() {
        return z;
    }

    public float getYRot() {
        return yaw;
    }

    public float getXRot() {
        return pitch;
    }
}
