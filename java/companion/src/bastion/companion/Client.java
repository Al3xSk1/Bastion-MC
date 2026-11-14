package bastion.companion;

import java.lang.invoke.MethodHandle;

/**
 * The few things several features ask the running client: is there a player, is a screen
 * open. Resolved by whichever feature needs it; a failure is that feature's failure.
 */
public final class Client {
    private final MethodHandle instance;
    private final MethodHandle player;
    private final MethodHandle gui;
    private final MethodHandle screen;
    private final MethodHandle overlay;

    public Client(Game game) throws ReflectiveOperationException {
        Class<?> minecraft = game.type("net.minecraft.client.Minecraft");
        Class<?> guiType = game.type("net.minecraft.client.gui.Gui");
        instance = game.statik(minecraft, "getInstance", minecraft);
        player = game.getter(minecraft, "player", game.type("net.minecraft.client.player.LocalPlayer"));
        gui = game.getter(minecraft, "gui", guiType);
        screen = game.virtual(guiType, "screen", game.type("net.minecraft.client.gui.screens.Screen"));
        overlay = game.virtual(guiType, "overlay", game.type("net.minecraft.client.gui.screens.Overlay"));
    }

    /** The client object; null before the game has created it. */
    public Object minecraft() throws Throwable {
        return (Object) instance.invokeExact();
    }

    /** The local player, or null outside a world. */
    public Object player(Object minecraft) throws Throwable {
        return (Object) player.invokeExact(minecraft);
    }

    /** The open screen (chat, inventory, a menu), or null. */
    public Object screen(Object minecraft) throws Throwable {
        Object g = (Object) gui.invokeExact(minecraft);
        return g == null ? null : (Object) screen.invokeExact(g);
    }

    /**
     * In a world with nothing in front of it: no screen and no loading overlay. The state
     * in which keys mean "play", not "type".
     */
    public boolean playing() throws Throwable {
        Object minecraft = (Object) instance.invokeExact();
        if (minecraft == null || (Object) player.invokeExact(minecraft) == null) {
            return false;
        }
        Object g = (Object) gui.invokeExact(minecraft);
        return g != null && (Object) screen.invokeExact(g) == null && (Object) overlay.invokeExact(g) == null;
    }
}
