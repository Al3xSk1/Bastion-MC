package bastion.companion.phone;

import bastion.companion.Feature;
import bastion.companion.Game;
import bastion.companion.platform.Link;
import bastion.companion.platform.Platform;
import bastion.companion.platform.Platforms;

/**
 * The phone, as a feature of the mod (switch {@code phone}, key {@code phone.key} — both
 * the launcher's settings; the platform binds the key). Off where no platform adapter fits the
 * game: the phone needs one to draw anything.
 */
public final class PhoneFeature extends Feature {
    private volatile Phone phone;

    public PhoneFeature() {
        super("phone");
    }

    @Override
    protected void resolve(Game game) throws Throwable {
        if (!Platforms.live()) {
            throw new IllegalStateException(
                    "no platform adapter for Minecraft " + Platforms.current().minecraftVersion());
        }
        Platform platform = Platforms.current();
        Link link = Platforms.link();
        Phone made = new Phone(platform, link);
        link.setListener(made.store());
        platform.setPhoneKey(made::toggle);
        platform.setHud(made);
        phone = made;
    }

    /** The phone, once the feature has started (null before, or when it is off). */
    public Phone phone() {
        return phone;
    }
}
