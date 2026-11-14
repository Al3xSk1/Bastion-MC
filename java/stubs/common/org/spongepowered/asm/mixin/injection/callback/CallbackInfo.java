// Compile-only stand-in for the real type (names and signatures only; see build.rs).
// Never packaged into the mod: at run time the instance's loader libraries provide it.
package org.spongepowered.asm.mixin.injection.callback;

public class CallbackInfo {
    public CallbackInfo(String name, boolean cancellable) {
    }

    public void cancel() {
    }
}
