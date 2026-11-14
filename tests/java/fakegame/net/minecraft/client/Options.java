package net.minecraft.client;

/** Stand-in (see CompanionFakeGame.java). */
public class Options {
    public final OptionInstance<Double> brightness = new OptionInstance<>(0.5);

    public OptionInstance<Double> gamma() {
        return brightness;
    }
}
