package bastion.companion.mixin.v1_21_6;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Coerce;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;

/** Minecraft (class_310): every screen change (for the home screen), and the start of every frame. */
@Mixin(targets = "net.minecraft.class_310")
abstract class MinecraftMixin {
    /** setScreen(Screen) */
    @Inject(method = "method_1507(Lnet/minecraft/class_437;)V", at = @At("HEAD"), cancellable = true, require = 0)
    private void bastion$setScreen(@Coerce Object screen, CallbackInfo callback) {
        Object instead = Events.setScreen(screen);
        if (instead != screen) {
            callback.cancel();
            Events.reopen(instead);
        }
    }

    /** runTick(boolean): once per frame. */
    @Inject(method = "method_1523(Z)V", at = @At("HEAD"), require = 0)
    private void bastion$frame(boolean render, CallbackInfo callback) {
        Events.frame();
    }
}
