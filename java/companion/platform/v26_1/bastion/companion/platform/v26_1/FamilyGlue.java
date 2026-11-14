package bastion.companion.platform.v26_1;

import bastion.companion.platform.mojang.Base;
import com.mojang.blaze3d.platform.InputConstants;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.options.OptionsScreen;

/** Minecraft 26.1 to 26.1.2: the open screen is the client's; keys and buttons are GLFW's. */
public final class FamilyGlue extends Base {
    @Override
    public String family() {
        return "v26_1";
    }

    @Override
    public Object screen() {
        return game().screen;
    }

    @Override
    public void setScreen(Object screen) {
        game().setScreen((Screen) screen);
    }

    @Override
    public boolean loading() {
        return game().getOverlay() != null;
    }

    @Override
    public boolean keyDown(int code) {
        return InputConstants.isKeyDown(game().getWindow(), code);
    }

    @Override
    protected Screen optionsScreen(Screen back) {
        return new OptionsScreen(back, game().options, false);
    }

    @Override
    protected int keys(int raw) {
        return raw;
    }

    @Override
    protected int modifiers(int raw) {
        return raw;
    }

    @Override
    protected int button(int raw) {
        return raw;
    }
}
