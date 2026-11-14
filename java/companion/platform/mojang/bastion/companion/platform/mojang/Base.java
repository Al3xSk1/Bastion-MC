package bastion.companion.platform.mojang;

import java.util.concurrent.Executor;

import bastion.companion.platform.Glue;
import bastion.companion.platform.HomeAction;
import bastion.companion.platform.Host;
import bastion.companion.platform.Pixels;
import bastion.companion.platform.Texture;
import com.mojang.blaze3d.platform.InputConstants;
import com.mojang.realmsclient.RealmsMainScreen;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.screens.CreditsAndAttributionScreen;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.TitleScreen;
import net.minecraft.client.gui.screens.multiplayer.JoinMultiplayerScreen;
import net.minecraft.client.gui.screens.options.AccessibilityOptionsScreen;
import net.minecraft.client.gui.screens.options.LanguageSelectScreen;
import net.minecraft.client.gui.screens.worldselection.SelectWorldScreen;
import net.minecraft.client.input.KeyEvent;
import net.minecraft.client.resources.language.I18n;
import net.minecraft.client.resources.sounds.SimpleSoundInstance;
import net.minecraft.sounds.SoundEvents;

/**
 * What the unobfuscated families (Minecraft 26.1 on, Mojang's names at run time) do alike.
 * Compiled against the stand-ins of each of them by the build, which checks that the class
 * files come out the same. The families add where the open screen lives (on the client up to
 * 26.1, on its Gui from 26.2), how a key's state is read, and the options screen's
 * constructor.
 */
public abstract class Base extends Glue {
    protected static Minecraft game() {
        return Minecraft.getInstance();
    }

    @Override
    public boolean isTitleScreen(Object screen) {
        return screen != null && screen.getClass() == TitleScreen.class;
    }

    @Override
    public Object newTitleScreen() {
        return new TitleScreen();
    }

    @Override
    public boolean hasPlayer() {
        return game().player != null;
    }

    @Override
    public boolean demo() {
        return game().isDemo();
    }

    @Override
    public double guiScale() {
        return game().getWindow().getGuiScale();
    }

    @Override
    public Object newHostScreen(Host host) {
        return new HostScreen(host, this);
    }

    @Override
    public int keyCode(String name) {
        try {
            int value = InputConstants.getKey(name).getValue();
            return name.startsWith("key.keyboard.") && value > 0 ? value : -1;
        } catch (RuntimeException unknown) {
            return -1;
        }
    }

    @Override
    public String keyLabel(String name) {
        return InputConstants.getKey(name).getDisplayName().getString();
    }

    @Override
    public String translate(String key) {
        return I18n.get(key, new Object[0]);
    }

    @Override
    public void execute(Runnable task) {
        ((Executor) (Object) game()).execute(task);
    }

    @Override
    public void clickSound() {
        game().getSoundManager().play(SimpleSoundInstance.forUI(SoundEvents.UI_BUTTON_CLICK, 1.0f));
    }

    @Override
    public Texture texture(Pixels pixels) {
        return new FamilyTexture(pixels);
    }

    @Override
    public boolean available(HomeAction action) {
        boolean demo = game().isDemo();
        switch (action) {
            case SINGLEPLAYER:
            case MULTIPLAYER:
            case REALMS:
                return !demo;
            case PLAY_DEMO:
                return demo;
            case MODS:
                return modMenu() != null;
            default:
                return true;
        }
    }

    @Override
    public void perform(HomeAction action, Object parent) throws Throwable {
        Minecraft game = game();
        Screen back = parent instanceof Screen ? (Screen) parent : new TitleScreen();
        switch (action) {
            case SINGLEPLAYER:
                setScreen(new SelectWorldScreen(back));
                break;
            case MULTIPLAYER:
                setScreen(new JoinMultiplayerScreen(back));
                break;
            case REALMS:
                setScreen(new RealmsMainScreen(back));
                break;
            case OPTIONS:
                setScreen(optionsScreen(back));
                break;
            case LANGUAGE:
                setScreen(new LanguageSelectScreen(back, game.options, game.getLanguageManager()));
                break;
            case ACCESSIBILITY:
                setScreen(new AccessibilityOptionsScreen(back, game.options));
                break;
            case CREDITS:
                setScreen(new CreditsAndAttributionScreen(back));
                break;
            case PLAY_DEMO:
                pressTitleButton("menu.playdemo");
                break;
            case MODS:
                Class<?> screen = modMenu();
                if (screen != null) {
                    setScreen(screen.getConstructor(Screen.class).newInstance(back));
                }
                break;
            case QUIT:
                game.stop();
                break;
            default:
                break;
        }
    }

    /** The options screen, its parent being {@code back} (the constructor changed in 26.3). */
    protected abstract Screen optionsScreen(Screen back);

    private static Class<?> modMenu() {
        try {
            return Class.forName("com.terraformersmc.modmenu.gui.ModsScreen", false, Base.class.getClassLoader());
        } catch (ClassNotFoundException | LinkageError absent) {
            return null;
        }
    }

    /** As the intermediary families do: open the game's title screen, press its button for {@code key}. */
    protected void pressTitleButton(String key) {
        TitleScreen title = new TitleScreen();
        bastion.companion.platform.Core core = bastion.companion.platform.Core.get();
        if (core == null) {
            return;
        }
        core.showClassicTitle(title);
        String caption = I18n.get(key, new Object[0]);
        for (Object child : title.children().toArray()) {
            if (child instanceof Button && caption.equals(((Button) child).getMessage().getString())) {
                // The input that "pressed" it: Enter, as a keyboard would.
                ((Button) child).onPress(new KeyEvent(257, 0, 0));
                return;
            }
        }
    }

    // ------------------------------------------------------------------ input, for the host screen

    /**
     * {@code owner} (a host screen) is shown ({@code on}) or gone: where the game's input
     * layer delivers typed characters only to a window that has asked for them (SDL, 26.3),
     * ask for them while it is shown. Nothing to do elsewhere.
     */
    protected void textInput(Object owner, boolean on) {
    }

    /** The {@link bastion.companion.platform.Keys} number of a key event's key. */
    protected abstract int keys(int raw);

    /** {@link bastion.companion.platform.Keys} modifier bits from the game's. */
    protected abstract int modifiers(int raw);

    /** {@link bastion.companion.platform.Keys} mouse button from the game's. */
    protected abstract int button(int raw);
}
