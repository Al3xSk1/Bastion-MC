package bastion.companion.platform.v26_3;

import bastion.companion.platform.mojang.Base;
import com.mojang.blaze3d.platform.InputConstants;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.options.OptionsScreen;

/** Minecraft 26.3: as 26.2, but input is SDL's: key numbers are scan codes, modifiers and mouse buttons SDL's. */
public final class FamilyGlue extends Base {
    @Override
    public String family() {
        return "v26_3";
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
        return InputConstants.isKeyDown(code);
    }

    @Override
    protected Screen optionsScreen(Screen back) {
        return new OptionsScreen(back, game().options);
    }

    /**
     * SDL sends text events only while text input is started for the window: the game starts
     * it for its own text fields; a host screen asks for it while it is shown.
     */
    @Override
    protected void textInput(Object owner, boolean on) {
        if (on) {
            game().textInputManager().startTextInput(owner);
        } else {
            game().textInputManager().stopTextInput(owner);
        }
    }

    @Override
    protected int keys(int raw) {
        return Sdl.key(raw);
    }

    @Override
    protected int modifiers(int raw) {
        return Sdl.modifiers(raw);
    }

    @Override
    protected int button(int raw) {
        return Sdl.button(raw);
    }
}
