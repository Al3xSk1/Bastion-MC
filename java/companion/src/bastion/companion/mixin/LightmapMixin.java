package bastion.companion.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Coerce;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.Hooks;

/**
 * Fullbright: the light map's inputs, right after the game recomputed them.
 *
 * TAIL is reached only then; the early returns (nothing changed since the last frame, or
 * no world) leave the state untouched.
 */
@Mixin(targets = "net.minecraft.client.renderer.LightmapRenderStateExtractor")
abstract class LightmapMixin {
    @Inject(
            method = "extract(Lnet/minecraft/client/renderer/state/LightmapRenderState;F)V",
            at = @At("TAIL"),
            require = 0)
    private void bastion$fullbright(@Coerce Object state, float partialTicks, CallbackInfo callback) {
        Hooks.lightmap(state);
    }
}
