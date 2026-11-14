package bastion.companion.mixin.v1_21_5;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;
import bastion.companion.platform.v1_21_5.Hud;
import net.minecraft.class_332;

/** Screen (class_437): the background behind the game's own screens. */
@Mixin(targets = "net.minecraft.class_437")
abstract class ScreenMixin {
    /** renderBackground(GuiGraphics, mouseX, mouseY, partialTick), from 1.20.2 on */
    @Inject(method = "method_25420(Lnet/minecraft/class_332;IIF)V", at = @At("HEAD"), cancellable = true, require = 0)
    private void bastion$background(class_332 graphics, int mouseX, int mouseY, float partialTick, CallbackInfo callback) {
        if (Events.live() && Hud.background(graphics, this)) {
            callback.cancel();
        }
    }
}
