package bastion.companion.platform.v1_20_1;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Host;
import bastion.companion.platform.Keys;
import bastion.companion.platform.intermediary.BaseScreen;
import net.minecraft.class_332;

/**
 * Minecraft 1.20.1's host screen: GLFW key codes, modifiers and mouse buttons are already in
 * {@link Keys} numbering; the wheel reports the vertical movement only, and the background
 * method takes the graphics alone.
 */
final class HostScreen extends BaseScreen {
    HostScreen(Host host) {
        super(host);
    }

    /** renderBackground(GuiGraphics) */
    @Override
    protected void background(class_332 graphics, int mouseX, int mouseY, float partialTick) {
        method_25420(graphics);
    }

    @Override
    protected Canvas canvas(class_332 graphics) {
        return new FamilyCanvas(graphics, field_22789, field_22790);
    }

    /** keyPressed(keyCode, scanCode, modifiers) */
    @Override
    public boolean method_25404(int key, int scancode, int modifiers) {
        return host.keyPressed(key, key, modifiers);
    }

    /** keyReleased(keyCode, scanCode, modifiers) */
    @Override
    public boolean method_16803(int key, int scancode, int modifiers) {
        return host.keyReleased(key, modifiers);
    }

    /** charTyped(character, modifiers) */
    @Override
    public boolean method_25400(char character, int modifiers) {
        return host.charTyped(character, modifiers);
    }

    /** mouseClicked(x, y, button) */
    @Override
    public boolean method_25402(double x, double y, int button) {
        return host.mousePressed(x, y, button, 0);
    }

    /** mouseReleased(x, y, button) */
    @Override
    public boolean method_25406(double x, double y, int button) {
        return host.mouseReleased(x, y, button, 0);
    }

    /** mouseDragged(x, y, button, dx, dy) */
    @Override
    public boolean method_25403(double x, double y, int button, double dx, double dy) {
        return host.mouseDragged(x, y, button, dx, dy);
    }

    /** mouseScrolled(x, y, vertical) */
    @Override
    public boolean method_25401(double x, double y, double vertical) {
        return host.mouseScrolled(x, y, 0.0, vertical);
    }
}
