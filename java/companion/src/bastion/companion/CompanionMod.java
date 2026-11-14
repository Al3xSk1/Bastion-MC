package bastion.companion;

import net.fabricmc.api.ClientModInitializer;

/** The entry point named in fabric.mod.json; the only class that refers to Fabric Loader. */
public final class CompanionMod implements ClientModInitializer {
    @Override
    public void onInitializeClient() {
        Companion.start();
    }
}
