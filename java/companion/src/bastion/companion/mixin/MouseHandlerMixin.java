package bastion.companion.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.Hooks;

/** Zoom: mouse movement is scaled while zoomed, and the wheel changes the factor. */
@Mixin(targets = "net.minecraft.client.MouseHandler")
abstract class MouseHandlerMixin {
    @Inject(method = "turnPlayer(D)V", at = @At("HEAD"), require = 0)
    private void bastion$zoomTurn(double seconds, CallbackInfo callback) {
        Hooks.turn(this);
    }

    @Inject(method = "onScroll(JDD)V", at = @At("HEAD"), cancellable = true, require = 0)
    private void bastion$zoomWheel(long window, double horizontal, double vertical, CallbackInfo callback) {
        if (Hooks.wheel(vertical)) {
            callback.cancel();
        }
    }
}
