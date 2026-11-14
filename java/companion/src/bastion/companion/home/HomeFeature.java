package bastion.companion.home;

import bastion.companion.Feature;
import bastion.companion.Game;
import bastion.companion.platform.Platform;
import bastion.companion.platform.Platforms;

/**
 * The redone title screen, as a feature of the mod (switch {@code home}, on by default). Off
 * where no platform adapter fits the game; the game's own title screen is then shown, as it is
 * whenever this feature is switched off.
 */
public final class HomeFeature extends Feature {
    private volatile HomeScreen screen;

    public HomeFeature() {
        super("home");
    }

    @Override
    protected void resolve(Game game) throws Throwable {
        if (!Platforms.live()) {
            throw new IllegalStateException(
                    "no platform adapter for Minecraft " + Platforms.current().minecraftVersion());
        }
        Platform platform = Platforms.current();
        HomeScreen made = new HomeScreen(platform);
        platform.setHome(made);
        screen = made;
    }

    /** The home screen, once the feature has started. */
    public HomeScreen screen() {
        return screen;
    }
}
