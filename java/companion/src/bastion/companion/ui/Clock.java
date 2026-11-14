package bastion.companion.ui;

import java.util.function.LongSupplier;

/**
 * The time animations, rate limits and countdowns are measured in: {@link System#nanoTime()}
 * in the game. The tests set {@link #source} to a clock they move themselves, so that a slide,
 * a batch of typing or a countdown can be checked without waiting for it.
 */
public final class Clock {
    public static volatile LongSupplier source = System::nanoTime;

    private Clock() {
    }

    /** Now, in nanoseconds of an arbitrary origin. */
    public static long now() {
        return source.getAsLong();
    }
}
