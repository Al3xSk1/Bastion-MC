package net.minecraft.client;

import bastion.companion.Hooks;

/** Stand-in (see CompanionFakeGame.java and Camera.java). */
public class MouseHandler {
    private double accumulatedDX;
    private double accumulatedDY;
    /** What the player was turned by, in total. */
    public double turnedX;
    public double turnedY;
    /** Wheel movement that reached the game (the hotbar). */
    public double scrolled;

    /** Mouse movement arrives, and the game acts on it in the same frame. */
    public void move(double dx, double dy) {
        accumulatedDX += dx;
        accumulatedDY += dy;
        turnPlayer(0.016);
        accumulatedDX = 0.0;
        accumulatedDY = 0.0;
    }

    private void turnPlayer(double seconds) {
        Hooks.turn(this);
        turnedX += accumulatedDX;
        turnedY += accumulatedDY;
    }

    public void onScroll(long window, double horizontal, double vertical) {
        if (Hooks.wheel(vertical)) {
            return;
        }
        scrolled += vertical;
    }

    private void handler$fake$bastion$zoomTurn() {
    }

    private void handler$fake$bastion$zoomWheel() {
    }
}
