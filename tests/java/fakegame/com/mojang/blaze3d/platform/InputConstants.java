package com.mojang.blaze3d.platform;

/**
 * Part of the stand-in game for `tests/companion.rs` (see CompanionFakeGame.java): the
 * members of the real class that the companion mod uses, with the same names and shapes.
 */
public class InputConstants {
    /** The keyboard state the test scripts, by key number. */
    public static final boolean[] DOWN = new boolean[512];

    public static boolean isKeyDown(int key) {
        return DOWN[key];
    }

    /**
     * As the game does it (checked against the 26.3 class): a known name gives its key; a
     * number after a type's prefix gives the key with that number — under the name it
     * already has, or a new one named after the number; anything else is an exception.
     */
    public static Key getKey(String name) {
        switch (name) {
            case "key.keyboard.c":
                return new Key(name, Type.KEYBOARD, 6);
            case "key.keyboard.v":
                return new Key(name, Type.KEYBOARD, 25);
            case "key.mouse.4":
                return new Key(name, Type.MOUSE, 4);
            default:
                break;
        }
        if (name.startsWith("key.keyboard")) {
            int number = Integer.parseInt(name.substring("key.keyboard".length() + 1));
            if (number == 6) {
                return getKey("key.keyboard.c");
            }
            if (number == 25) {
                return getKey("key.keyboard.v");
            }
            return new Key("key.keyboard." + number, Type.KEYBOARD, number);
        }
        throw new IllegalArgumentException("Unknown key name: " + name);
    }

    public static final class Key {
        private final String name;
        private final Type type;
        private final int value;

        Key(String name, Type type, int value) {
            this.name = name;
            this.type = type;
            this.value = value;
        }

        public Type getType() {
            return type;
        }

        public String getName() {
            return name;
        }

        public int getValue() {
            return value;
        }
    }

    public enum Type {
        KEYBOARD,
        MOUSE
    }
}
