package bastion.companion.mixin.v1_20_1;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;
import bastion.companion.platform.v1_20_1.Hud;
import net.minecraft.class_332;

/** Screen (class_437): the background behind the game's own screens. */
@Mixin(targets = "net.minecraft.class_437")
abstract class ScreenMixin {
    /** renderBackground(GuiGraphics), in 1.20.1 */
    @Inject(method = "method_25420(Lnet/minecraft/class_332;)V", at = @At("HEAD"), cancellable = true, require = 0)
    private void bastion$background(class_332 graphics, CallbackInfo callback) {
        if (Events.live() && Hud.background(graphics, this)) {
            callback.cancel();
        }
    }
}
