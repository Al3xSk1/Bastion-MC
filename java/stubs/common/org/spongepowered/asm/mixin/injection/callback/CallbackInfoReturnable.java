// Compile-only stand-in for the real type (names and signatures only; see build.rs).
// Never packaged into the mod: at run time the instance's loader libraries provide it.
package org.spongepowered.asm.mixin.injection.callback;

public class CallbackInfoReturnable<R> extends CallbackInfo {
    public CallbackInfoReturnable(String name, boolean cancellable) {
        super(name, cancellable);
    }

    public void setReturnValue(R returnValue) {
    }

    public float getReturnValueF() {
        return 0.0f;
    }
}
