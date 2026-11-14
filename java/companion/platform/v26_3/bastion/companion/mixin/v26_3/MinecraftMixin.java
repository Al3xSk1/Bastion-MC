package bastion.companion.mixin.v26_3;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;

/** Minecraft: the start of every frame. */
@Mixin(targets = "net.minecraft.client.Minecraft")
abstract class MinecraftMixin {
    @Inject(method = "runTick(Z)V", at = @At("HEAD"), require = 0)
    private void bastion$frame(boolean render, CallbackInfo callback) {
        Events.frame();
    }
}
