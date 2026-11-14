package bastion.companion.mixin.v26_1;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import bastion.companion.platform.Events;
import bastion.companion.platform.v26_1.Hooks;
import net.minecraft.client.gui.GuiGraphicsExtractor;

/** Screen: the background behind the game's own screens. */
@Mixin(targets = "net.minecraft.client.gui.screens.Screen")
abstract class ScreenMixin {
    @Inject(method = "extractBackground(Lnet/minecraft/client/gui/GuiGraphicsExtractor;IIF)V", at = @At("HEAD"),
            cancellable = true, require = 0)
    private void bastion$background(GuiGraphicsExtractor graphics, int mouseX, int mouseY, float partialTick,
            CallbackInfo callback) {
        if (Events.live() && Hooks.background(graphics, this)) {
            callback.cancel();
        }
    }
}
