package bastion.companion.ui;

import bastion.companion.Feature;
import bastion.companion.home.HomeFeature;
import bastion.companion.phone.PhoneFeature;
import bastion.companion.theme.DarkMenus;

/**
 * The entry point of the phone, the home screen and the dark menus (package mod-ui). The
 * mod's start-up asks {@link #features()} for the UI's features and starts them like its own
 * (zoom, HUD readout, fullbright): each is configured from the launcher's settings, resolved,
 * listed in the hello when live, and switched off with one diagnostic when it fails. The
 * platform and the launcher link are {@code bastion.companion.platform.Platforms.current()}
 * and {@code .link()}.
 *
 * Everything here is built on the platform API alone ({@code bastion.companion.platform}):
 * no game class is named, so the same code runs on every Minecraft family the platform has an
 * adapter for.
 */
public final class Ui {
    public static final PhoneFeature PHONE = new PhoneFeature();
    public static final HomeFeature HOME = new HomeFeature();
    public static final DarkMenus DARK_MENUS = new DarkMenus();

    private Ui() {
    }

    /** The UI's features (ids {@code phone}, {@code home}, {@code darkmenus}); called once. */
    public static Feature[] features() {
        return new Feature[] {PHONE, HOME, DARK_MENUS};
    }
}
