package org.spongepowered.asm.mixin.extensibility;

import java.util.List;
import java.util.Set;

import org.objectweb.asm.tree.ClassNode;

/** Compile-only stand-in for Mixin's type of the same name: names and signatures only. */
public interface IMixinConfigPlugin {
    default void onLoad(String mixinPackage) {
    }

    default String getRefMapperConfig() {
        return null;
    }

    default boolean shouldApplyMixin(String targetClassName, String mixinClassName) {
        return true;
    }

    default void acceptTargets(Set<String> myTargets, Set<String> otherTargets) {
    }

    default List<String> getMixins() {
        return null;
    }

    default void preApply(String targetClassName, ClassNode targetClass, String mixinClassName, IMixinInfo mixinInfo) {
    }

    default void postApply(String targetClassName, ClassNode targetClass, String mixinClassName, IMixinInfo mixinInfo) {
    }
}
