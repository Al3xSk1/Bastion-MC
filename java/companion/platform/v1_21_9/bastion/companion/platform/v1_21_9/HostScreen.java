package bastion.companion.platform.v1_21_9;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Host;
import bastion.companion.platform.Keys;
import bastion.companion.platform.intermediary.BaseScreen;
import net.minecraft.class_11905;
import net.minecraft.class_11908;
import net.minecraft.class_11909;
import net.minecraft.class_332;

/**
 * The host screen from 1.21.9 on: input arrives as event records (KeyEvent class_11908,
 * CharacterEvent class_11905, MouseButtonEvent class_11909), still in GLFW numbering, which
 * is {@link Keys} numbering.
 */
final class HostScreen extends BaseScreen {
    HostScreen(Host host) {
        super(host);
    }

    /** renderBackground(GuiGraphics, mouseX, mouseY, partialTick) */
    @Override
    protected void background(class_332 graphics, int mouseX, int mouseY, float partialTick) {
        method_25420(graphics, mouseX, mouseY, partialTick);
    }

    @Override
    protected Canvas canvas(class_332 graphics) {
        return new FamilyCanvas(graphics, field_22789, field_22790);
    }

    /** keyPressed(KeyEvent) */
    @Override
    public boolean method_25404(class_11908 event) {
        return host.keyPressed(event.comp_4795(), event.comp_4795(), event.comp_4797());
    }

    /** keyReleased(KeyEvent) */
    @Override
    public boolean method_16803(class_11908 event) {
        return host.keyReleased(event.comp_4795(), event.comp_4797());
    }

    /** charTyped(CharacterEvent) */
    @Override
    public boolean method_25400(class_11905 event) {
        return host.charTyped(event.comp_4793(), event.comp_4794());
    }

    /** mouseClicked(MouseButtonEvent, doubleClick) */
    @Override
    public boolean method_25402(class_11909 event, boolean doubleClick) {
        return host.mousePressed(event.comp_4798(), event.comp_4799(), event.method_74245(), event.comp_4797());
    }

    /** mouseReleased(MouseButtonEvent) */
    @Override
    public boolean method_25406(class_11909 event) {
        return host.mouseReleased(event.comp_4798(), event.comp_4799(), event.method_74245(), event.comp_4797());
    }

    /** mouseDragged(MouseButtonEvent, dx, dy) */
    @Override
    public boolean method_25403(class_11909 event, double dx, double dy) {
        return host.mouseDragged(event.comp_4798(), event.comp_4799(), event.method_74245(), dx, dy);
    }

    /** mouseScrolled(x, y, horizontal, vertical) */
    @Override
    public boolean method_25401(double x, double y, double horizontal, double vertical) {
        return host.mouseScrolled(x, y, horizontal, vertical);
    }
}
