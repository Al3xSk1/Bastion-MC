package bastion.companion.platform;

/**
 * One {@link Overlay} on screen: what a family's host screen (a subclass of the game's
 * {@code Screen}) calls. Version-independent: the host screen has already translated the
 * game's events into {@link Keys} numbering. An exception from the overlay closes it and is
 * logged once; it never reaches the game.
 */
public final class Host {
    private final Core core;
    private final Overlay overlay;
    private final boolean home;
    /** The screen to return to when the overlay closes (null: back to the game). */
    Object previous;
    /** This host's game screen, once made. */
    Object screen;
    private boolean failed;

    Host(Core core, Overlay overlay, boolean home) {
        this.core = core;
        this.overlay = overlay;
        this.home = home;
    }

    public Overlay overlay() {
        return overlay;
    }

    /** Whether this is the home screen (the title screen's replacement). */
    public boolean home() {
        return home;
    }

    private void failed(String where, Throwable failure) {
        if (!failed) {
            failed = true;
            core.problem("the " + (home ? "home screen" : "overlay") + " (" + where + ")", failure);
            core.closeFailed(this);
        }
    }

    /** The screen was set up, or resized: it is {@code width} by {@code height}. */
    public void init(int width, int height) {
        try {
            overlay.opened(width, height);
        } catch (Throwable failure) {
            failed("opened", failure);
        }
    }

    public void render(Canvas canvas, int mouseX, int mouseY, float partialTick) {
        if (failed) {
            return;
        }
        try {
            overlay.render(canvas, mouseX, mouseY, partialTick);
        } catch (Throwable failure) {
            failed("render", failure);
        }
    }

    public void tick() {
        if (failed) {
            return;
        }
        try {
            overlay.tick();
        } catch (Throwable failure) {
            failed("tick", failure);
        }
    }

    public boolean mousePressed(double x, double y, int button, int modifiers) {
        try {
            return !failed && overlay.mousePressed(x, y, button, modifiers);
        } catch (Throwable failure) {
            failed("mousePressed", failure);
            return true;
        }
    }

    public boolean mouseReleased(double x, double y, int button, int modifiers) {
        try {
            return !failed && overlay.mouseReleased(x, y, button, modifiers);
        } catch (Throwable failure) {
            failed("mouseReleased", failure);
            return true;
        }
    }

    public boolean mouseDragged(double x, double y, int button, double dx, double dy) {
        try {
            return !failed && overlay.mouseDragged(x, y, button, dx, dy);
        } catch (Throwable failure) {
            failed("mouseDragged", failure);
            return true;
        }
    }

    public boolean mouseScrolled(double x, double y, double horizontal, double vertical) {
        try {
            return !failed && overlay.mouseScrolled(x, y, horizontal, vertical);
        } catch (Throwable failure) {
            failed("mouseScrolled", failure);
            return true;
        }
    }

    /**
     * A key went down. {@code raw} is the family's own number for it (compared with the phone
     * key's), {@code key} the {@link Keys} number. Escape closes the overlay where it allows
     * that; a key the overlay does not use and that is the phone key runs the phone key's
     * action. Always true: nothing typed while one of this mod's screens is open goes to the
     * game.
     */
    public boolean keyPressed(int raw, int key, int modifiers) {
        if (failed) {
            return true;
        }
        try {
            if (key == Keys.ESCAPE && overlay.closesOnEscape() && !home) {
                core.close(overlay);
                return true;
            }
            if (overlay.keyPressed(key, modifiers)) {
                return true;
            }
            if (core.isPhoneKey(raw)) {
                core.phoneKeyPressed();
            }
        } catch (Throwable failure) {
            failed("keyPressed", failure);
        }
        return true;
    }

    public boolean keyReleased(int key, int modifiers) {
        try {
            return !failed && overlay.keyReleased(key, modifiers);
        } catch (Throwable failure) {
            failed("keyReleased", failure);
            return true;
        }
    }

    public boolean charTyped(int codePoint, int modifiers) {
        try {
            return !failed && overlay.charTyped(codePoint, modifiers);
        } catch (Throwable failure) {
            failed("charTyped", failure);
            return true;
        }
    }

    /** Whether Escape closes the screen (the game's own Escape handling is not used). */
    public boolean closesOnEscape() {
        return false;
    }

    /** Whether the game draws its background behind the overlay. */
    public boolean gameBackground() {
        try {
            return !failed && overlay.gameBackground();
        } catch (Throwable failure) {
            failed("gameBackground", failure);
            return false;
        }
    }

    /** The game took the screen away (another screen, or none). */
    public void removed() {
        core.removed(this);
    }

    void closed() {
        try {
            overlay.closed();
        } catch (Throwable failure) {
            core.problem("the overlay (closed)", failure);
        }
    }
}
