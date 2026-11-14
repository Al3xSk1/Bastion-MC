package bastion.companion.mixin.v26_2;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Coerce;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;

/** Gui: every screen change (for the home screen); the open screen lives here from 26.2 on. */
@Mixin(targets = "net.minecraft.client.gui.Gui")
abstract class GuiMixin {
    @Inject(method = "setScreen(Lnet/minecraft/client/gui/screens/Screen;)V", at = @At("HEAD"), cancellable = true,
            require = 0)
    private void bastion$setScreen(@Coerce Object screen, CallbackInfo callback) {
        Object instead = Events.setScreen(screen);
        if (instead != screen) {
            callback.cancel();
            Events.reopen(instead);
        }
    }
}
