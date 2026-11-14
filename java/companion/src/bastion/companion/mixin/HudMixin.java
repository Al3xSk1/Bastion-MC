package bastion.companion.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Coerce;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.Hooks;

/**
 * HUD readout: drawn after everything else of the in-game HUD.
 *
 * TAIL skips the early return taken while a world is still loading. The two parameters are
 * game types this mod is compiled without; {@code @Coerce} lets the handler take them as
 * {@code Object}.
 */
@Mixin(targets = "net.minecraft.client.gui.Hud")
abstract class HudMixin {
    @Inject(
            method = "extractRenderState(Lnet/minecraft/client/gui/GuiGraphicsExtractor;Lnet/minecraft/client/DeltaTracker;)V",
            at = @At("TAIL"),
            require = 0)
    private void bastion$hudReadout(@Coerce Object graphics, @Coerce Object deltaTracker, CallbackInfo callback) {
        Hooks.hud(this, graphics);
    }
}
