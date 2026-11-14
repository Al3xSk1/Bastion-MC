package bastion.companion.platform.v1_21_6;

import bastion.companion.platform.Host;
import bastion.companion.platform.Pixels;
import bastion.companion.platform.Texture;
import bastion.companion.platform.intermediary.Base;
import net.minecraft.class_1109;
import net.minecraft.class_310;
import net.minecraft.class_3417;
import net.minecraft.class_3675;
import net.minecraft.class_4185;

/** Minecraft 1.21.6 to 1.21.8: what differs from the other intermediary families. */
public final class FamilyGlue extends Base {
    @Override
    public String family() {
        return "v1_21_6";
    }

    @Override
    public Object newHostScreen(Host host) {
        return new HostScreen(host);
    }

    /** Window.getGuiScale(), an int from 1.21.6 on. */
    @Override
    public double guiScale() {
        return game().method_22683().method_4495();
    }

    /** InputConstants.isKeyDown(window handle, GLFW key code). */
    @Override
    public boolean keyDown(int code) {
        return class_3675.method_15987(game().method_22683().method_4490(), code);
    }

    /** SoundManager.play(SimpleSoundInstance.forUI(SoundEvents.UI_BUTTON_CLICK, 1)). */
    @Override
    public void clickSound() {
        class_310 game = game();
        game.method_1483().method_4873(class_1109.method_47978(class_3417.field_15015, 1.0f));
    }

    @Override
    public Texture texture(Pixels pixels) {
        return new FamilyTexture(pixels);
    }

    /** Button.onPress(), without arguments up to 1.21.8. */
    @Override
    protected void press(class_4185 button) {
        button.method_25306();
    }
}
