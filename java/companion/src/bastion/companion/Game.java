package bastion.companion;

import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.Arrays;

/**
 * How this mod reaches Minecraft: by name, once, at start-up.
 *
 * The mod is compiled without Minecraft on the class path, so no game class appears in its
 * source. Features ask for classes and members here and keep method handles whose types are
 * erased to {@code Object} and primitives. A name that does not resolve is an exception to
 * the feature that asked — which then switches itself off — never to the game.
 *
 * Names are the unobfuscated ones Minecraft ships with since 26.1.
 */
public final class Game {
    private final ClassLoader loader;
    private final MethodHandles.Lookup lookup = MethodHandles.publicLookup();

    Game(ClassLoader loader) {
        this.loader = loader;
    }

    /** Loads the class without initialising it. */
    public Class<?> type(String name) throws ClassNotFoundException {
        return Class.forName(name, false, loader);
    }

    /** A public instance method, as {@code (Object receiver, erased parameters…) -> erased result}. */
    public MethodHandle virtual(Class<?> owner, String name, Class<?> result, Class<?>... parameters)
            throws ReflectiveOperationException {
        MethodHandle handle = lookup.findVirtual(owner, name, MethodType.methodType(result, parameters));
        return handle.asType(handle.type().erase());
    }

    /** A public static method, with reference types erased to {@code Object}. */
    public MethodHandle statik(Class<?> owner, String name, Class<?> result, Class<?>... parameters)
            throws ReflectiveOperationException {
        MethodHandle handle = lookup.findStatic(owner, name, MethodType.methodType(result, parameters));
        return handle.asType(handle.type().erase());
    }

    /** Reads a public instance field: {@code (Object receiver) -> erased value}. */
    public MethodHandle getter(Class<?> owner, String name, Class<?> type) throws ReflectiveOperationException {
        MethodHandle handle = lookup.findGetter(owner, name, type);
        return handle.asType(handle.type().erase());
    }

    /** Writes a public instance field: {@code (Object receiver, erased value) -> void}. */
    public MethodHandle setter(Class<?> owner, String name, Class<?> type) throws ReflectiveOperationException {
        MethodHandle handle = lookup.findSetter(owner, name, type);
        return handle.asType(handle.type().erase());
    }

    /** Reads a private instance field: {@code (Object receiver) -> erased value}. */
    public MethodHandle privateGetter(Class<?> owner, String name) throws ReflectiveOperationException {
        MethodHandle handle = MethodHandles.lookup().unreflectGetter(accessible(owner, name));
        return handle.asType(handle.type().erase());
    }

    /** Writes a private instance field: {@code (Object receiver, erased value) -> void}. */
    public MethodHandle privateSetter(Class<?> owner, String name) throws ReflectiveOperationException {
        MethodHandle handle = MethodHandles.lookup().unreflectSetter(accessible(owner, name));
        return handle.asType(handle.type().erase());
    }

    private static Field accessible(Class<?> owner, String name) throws ReflectiveOperationException {
        Field field = owner.getDeclaredField(name);
        // Game and mods share one class loader and so one (unnamed) module: no module
        // boundary is crossed, and nothing outside the game's own classes is opened.
        field.setAccessible(true);
        return field;
    }

    /**
     * Checks that a hook of this mod is in place: that `target` still declares the method the
     * hook was written for, and that Mixin merged the hook's handler into `target` (it does
     * so under a name that ends with the handler's own).
     *
     * Every hook sits at the head or the tail of the method its mixin names, so when that
     * method is the one checked here, the two together mean the callback runs. (That each
     * mixin names the method its feature checks is not checked here — Mixin merges a
     * handler even when its injection matched nothing — but by the launcher's test against
     * the real game, which measures each hook's effect.)
     */
    public void requireHook(Class<?> target, String handler, String method, Class<?> result, Class<?>... parameters)
            throws ReflectiveOperationException {
        boolean merged = false;
        boolean present = false;
        for (Method declared : target.getDeclaredMethods()) {
            String name = declared.getName();
            merged |= name.endsWith(handler);
            present |= name.equals(method) && declared.getReturnType() == result
                    && Arrays.equals(declared.getParameterTypes(), parameters);
        }
        if (!present) {
            throw new NoSuchMethodException(target.getName() + "." + method + " is not as this mod expects it");
        }
        if (!merged) {
            throw new NoSuchMethodException("hook " + handler + " was not applied to " + target.getName());
        }
    }
}
