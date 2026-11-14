package bastion.companion.platform.intermediary;

import java.util.concurrent.Executor;

import bastion.companion.platform.Glue;
import bastion.companion.platform.HomeAction;
import net.minecraft.class_1074;
import net.minecraft.class_310;
import net.minecraft.class_3675;
import net.minecraft.class_4185;
import net.minecraft.class_4189;
import net.minecraft.class_426;
import net.minecraft.class_429;
import net.minecraft.class_4325;
import net.minecraft.class_437;
import net.minecraft.class_442;
import net.minecraft.class_500;
import net.minecraft.class_526;
import net.minecraft.class_8219;

/**
 * What every family that runs under Fabric's intermediary names (Minecraft 1.20.1 to 1.21.11)
 * does alike. Compiled against the stand-ins of each of them by the build, which checks that
 * the class files come out the same: everything named here exists, with the same
 * signature, in all of them. Mojang's names, for reading: class_310 Minecraft, class_437
 * Screen, class_442 TitleScreen, class_3675 InputConstants (class_306 its Key), class_1074
 * I18n, class_4185 Button.
 */
public abstract class Base extends Glue {
    /** The game's client object. */
    protected static class_310 game() {
        return class_310.method_1551();
    }

    @Override
    public Object screen() {
        return game().field_1755;
    }

    @Override
    public void setScreen(Object screen) {
        game().method_1507((class_437) screen);
    }

    @Override
    public boolean isTitleScreen(Object screen) {
        return screen != null && screen.getClass() == class_442.class;
    }

    @Override
    public Object newTitleScreen() {
        return new class_442();
    }

    @Override
    public boolean hasPlayer() {
        return game().field_1724 != null;
    }

    @Override
    public boolean loading() {
        return game().method_18506() != null;
    }

    @Override
    public boolean demo() {
        return game().method_1530();
    }

    @Override
    public int keyCode(String name) {
        try {
            int value = class_3675.method_15981(name).method_1444();
            return name.startsWith("key.keyboard.") && value > 0 ? value : -1;
        } catch (RuntimeException unknown) {
            return -1;
        }
    }

    @Override
    public String keyLabel(String name) {
        return class_3675.method_15981(name).method_27445().getString();
    }

    @Override
    public String translate(String key) {
        return class_1074.method_4662(key, new Object[0]);
    }

    @Override
    public void execute(Runnable task) {
        ((Executor) (Object) game()).execute(task);
    }

    @Override
    public boolean available(HomeAction action) {
        boolean demo = game().method_1530();
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
        class_310 game = game();
        class_437 back = parent instanceof class_437 ? (class_437) parent : new class_442();
        switch (action) {
            case SINGLEPLAYER:
                game.method_1507(new class_526(back));
                break;
            case MULTIPLAYER:
                game.method_1507(new class_500(back));
                break;
            case REALMS:
                game.method_1507(new class_4325(back));
                break;
            case OPTIONS:
                game.method_1507(new class_429(back, game.field_1690));
                break;
            case LANGUAGE:
                game.method_1507(new class_426(back, game.field_1690, game.method_1526()));
                break;
            case ACCESSIBILITY:
                game.method_1507(new class_4189(back, game.field_1690));
                break;
            case CREDITS:
                game.method_1507(new class_8219(back));
                break;
            case PLAY_DEMO:
                pressTitleButton("menu.playdemo");
                break;
            case MODS:
                Class<?> screen = modMenu();
                if (screen != null) {
                    game.method_1507((class_437) screen.getConstructor(class_437.class).newInstance(back));
                }
                break;
            case QUIT:
                game.method_1592();
                break;
            default:
                break;
        }
    }

    /** Mod Menu's list of mods, if that mod is installed. */
    private static Class<?> modMenu() {
        try {
            return Class.forName("com.terraformersmc.modmenu.gui.ModsScreen", false, Base.class.getClassLoader());
        } catch (ClassNotFoundException | LinkageError absent) {
            return null;
        }
    }

    /**
     * Does what the game's own title screen does for one of its buttons: opens that screen
     * (so that it sets itself up as it would for the player) and presses the button whose
     * caption is the translation of {@code key}. The title screen is never drawn: pressing
     * opens the next screen within the same call. Where there is no such button, the game's
     * title screen simply stays.
     */
    protected void pressTitleButton(String key) {
        class_442 title = new class_442();
        bastion.companion.platform.Core core = bastion.companion.platform.Core.get();
        if (core != null) {
            core.showClassicTitle(title);
        } else {
            game().method_1507(title);
        }
        String caption = class_1074.method_4662(key, new Object[0]);
        for (Object child : title.method_25396().toArray()) {
            if (child instanceof class_4185 && caption.equals(((class_4185) child).method_25369().getString())) {
                press((class_4185) child);
                return;
            }
        }
    }

    /** Presses {@code button} as a click would. */
    protected abstract void press(class_4185 button);
}
