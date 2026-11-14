package net.minecraft.client;

/** Stand-in (see CompanionFakeGame.java). */
public class OptionInstance<T> {
    public T value;

    OptionInstance(T value) {
        this.value = value;
    }

    public T get() {
        return value;
    }
}
