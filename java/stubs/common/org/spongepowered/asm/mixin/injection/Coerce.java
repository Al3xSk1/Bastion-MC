// Compile-only stand-in for the real type (names and signatures only; see build.rs).
// Never packaged into the mod: at run time the instance's loader libraries provide it.
package org.spongepowered.asm.mixin.injection;

import java.lang.annotation.ElementType;
import java.lang.annotation.Target;

@Target({ElementType.PARAMETER, ElementType.METHOD})
public @interface Coerce {
}
