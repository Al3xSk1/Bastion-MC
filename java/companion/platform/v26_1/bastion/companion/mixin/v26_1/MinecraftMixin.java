package bastion.companion.mixin.v26_1;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Coerce;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;

/** Minecraft: every screen change (for the home screen), and the start of every frame. */
@Mixin(targets = "net.minecraft.client.Minecraft")
abstract class MinecraftMixin {
    @Inject(method = "setScreen(Lnet/minecraft/client/gui/screens/Screen;)V", at = @At("HEAD"), cancellable = true,
            require = 0)
    private void bastion$setScreen(@Coerce Object screen, CallbackInfo callback) {
        Object instead = Events.setScreen(screen);
        if (instead != screen) {
            callback.cancel();
            Events.reopen(instead);
        }
    }

    @Inject(method = "runTick(Z)V", at = @At("HEAD"), require = 0)
    private void bastion$frame(boolean render, CallbackInfo callback) {
        Events.frame();
    }
}
