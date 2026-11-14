package bastion.companion.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

import bastion.companion.Hooks;

/**
 * Zoom: the world's field of view, as the camera computes it once per frame.
 *
 * TAIL is the method's last return, the one with the computed angle; the early return of
 * the fixed 90 degrees used for panorama screenshots is left alone.
 */
@Mixin(targets = "net.minecraft.client.Camera")
abstract class CameraMixin {
    @Inject(method = "calculateFov(F)F", at = @At("TAIL"), cancellable = true, require = 0)
    private void bastion$zoomFov(float partialTicks, CallbackInfoReturnable<Float> callback) {
        float computed = callback.getReturnValueF();
        float wanted = Hooks.fov(computed);
        if (wanted != computed) {
            callback.setReturnValue(wanted);
        }
    }
}
