package bastion.companion.platform.v26_2;

import bastion.companion.platform.mojang.Base;
import com.mojang.blaze3d.platform.InputConstants;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.options.OptionsScreen;

/** Minecraft 26.2: the open screen is the Gui's, the HUD a Hud of its own; keys and buttons are GLFW's. */
public final class FamilyGlue extends Base {
    @Override
    public String family() {
        return "v26_2";
    }

    @Override
    public Object screen() {
        return game().gui.screen();
    }

    @Override
    public void setScreen(Object screen) {
        game().gui.setScreen((Screen) screen);
    }

    @Override
    public boolean loading() {
        return game().gui.overlay() != null;
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
