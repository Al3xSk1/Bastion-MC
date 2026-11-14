package bastion.companion.mixin.v1_21_6;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Coerce;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;
import bastion.companion.platform.v1_21_6.Hud;
import net.minecraft.class_332;

/** Gui (class_329): the HUD layer, after the game's own HUD. */
@Mixin(targets = "net.minecraft.class_329")
abstract class GuiMixin {
    /** render(GuiGraphics, DeltaTracker), from 1.21 on */
    @Inject(method = "method_1753(Lnet/minecraft/class_332;Lnet/minecraft/class_9779;)V", at = @At("TAIL"), require = 0)
    private void bastion$hud(class_332 graphics, @Coerce Object deltaTracker, CallbackInfo callback) {
        if (Events.live()) {
            Hud.draw(graphics);
        }
    }
}
