package bastion.companion.mixin.v26_1;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Coerce;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;
import bastion.companion.platform.v26_1.Hooks;
import net.minecraft.client.gui.GuiGraphicsExtractor;

/** Gui: the HUD layer, after the game's own HUD. */
@Mixin(targets = "net.minecraft.client.gui.Gui")
abstract class GuiMixin {
    @Inject(
            method = "extractRenderState(Lnet/minecraft/client/gui/GuiGraphicsExtractor;Lnet/minecraft/client/DeltaTracker;)V",
            at = @At("TAIL"),
            require = 0)
    private void bastion$hud(GuiGraphicsExtractor graphics, @Coerce Object deltaTracker, CallbackInfo callback) {
        if (Events.live()) {
            Hooks.hud(graphics);
        }
    }
}
