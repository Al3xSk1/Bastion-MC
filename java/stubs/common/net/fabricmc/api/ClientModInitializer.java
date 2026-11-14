// Compile-only stand-in for the real type (names and signatures only; see build.rs).
// Never packaged into the mod: at run time the instance's loader libraries provide it.
package net.fabricmc.api;

@FunctionalInterface
public interface ClientModInitializer {
    void onInitializeClient();
}
