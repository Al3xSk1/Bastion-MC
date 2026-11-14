package bastion.companion.platform.v1_21_9;

import bastion.companion.platform.Host;
import bastion.companion.platform.Pixels;
import bastion.companion.platform.Texture;
import bastion.companion.platform.intermediary.Base;
import net.minecraft.class_1109;
import net.minecraft.class_11908;
import net.minecraft.class_310;
import net.minecraft.class_3417;
import net.minecraft.class_3675;
import net.minecraft.class_4185;

/** Minecraft 1.21.9 to 1.21.11: what differs from the other intermediary families. */
public final class FamilyGlue extends Base {
    @Override
    public String family() {
        return "v1_21_9";
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

    /** InputConstants.isKeyDown(Window, GLFW key code), from 1.21.9 on. */
    @Override
    public boolean keyDown(int code) {
        return class_3675.method_15987(game().method_22683(), code);
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

    /** Button.onPress(InputWithModifiers), from 1.21.9 on: as if Enter had pressed it. */
    @Override
    protected void press(class_4185 button) {
        button.method_25306(new class_11908(257, 0, 0));
    }
}
