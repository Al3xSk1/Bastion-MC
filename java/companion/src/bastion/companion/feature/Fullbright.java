package bastion.companion.feature;

import java.lang.invoke.MethodHandle;
import java.util.List;
import java.util.Locale;

import bastion.companion.Client;
import bastion.companion.Config;
import bastion.companion.Feature;
import bastion.companion.Game;
import bastion.companion.Hooks;

/**
 * Renders dark places as if the brightness slider went far beyond its maximum.
 *
 * Minecraft computes one "brightness" number for its light map from the Brightness option
 * (0 to 1) minus the pulse of the Darkness effect. This feature raises that number by the
 * difference between the configured value and the option, at the end of
 * {@code LightmapRenderStateExtractor.extract} — so the option itself, the options file
 * and the Darkness effect's own dimming are left as they are.
 *
 * It changes what this client draws and nothing it sends. Some servers forbid it all the
 * same; it is off unless the launcher's configuration turns it on.
 */
public final class Fullbright extends Feature {
    private double gamma = 15.0;

    private Client client;
    private MethodHandle options;
    private MethodHandle gammaOption;
    private MethodHandle optionValue;
    private MethodHandle readBrightness;
    private MethodHandle writeBrightness;
    private Class<?> stateType;

    private long applied;

    public Fullbright() {
        super("fullbright");
    }

    @Override
    protected void configure(Config config) {
        gamma = config.number("fullbright.gamma", gamma, 1.0, 100.0);
    }

    @Override
    protected void resolve(Game game) throws Throwable {
        client = new Client(game);
        Class<?> minecraft = game.type("net.minecraft.client.Minecraft");
        Class<?> optionsType = game.type("net.minecraft.client.Options");
        Class<?> option = game.type("net.minecraft.client.OptionInstance");
        Class<?> state = game.type("net.minecraft.client.renderer.state.LightmapRenderState");
        stateType = state;
        options = game.getter(minecraft, "options", optionsType);
        gammaOption = game.virtual(optionsType, "gamma", option);
        optionValue = game.virtual(option, "get", Object.class);
        readBrightness = game.getter(state, "brightness", float.class);
        writeBrightness = game.setter(state, "brightness", float.class);
        game.requireHook(game.type("net.minecraft.client.renderer.LightmapRenderStateExtractor"),
                "bastion$fullbright", "extract", void.class, state, float.class);
    }

    /** Called with the light map's render state each time the game has recomputed it. */
    public void apply(Object state) throws Throwable {
        Object minecraft = client.minecraft();
        if (minecraft == null) {
            return;
        }
        float current = (float) readBrightness.invokeExact(state);
        float raised = raise(current, optionGamma(minecraft));
        if (raised != current) {
            writeBrightness.invokeExact(state, raised);
        }
        applied++;
    }

    /** The Brightness option as the game holds it (0 to 1 in an unmodified game). */
    public double optionGamma(Object minecraft) throws Throwable {
        Object held = (Object) options.invokeExact(minecraft);
        Object value = (Object) optionValue.invokeExact((Object) gammaOption.invokeExact(held));
        return value instanceof Number ? ((Number) value).doubleValue() : 1.0;
    }

    /** `current` is what the game computed from an option value of `option`. */
    float raise(float current, double option) {
        double boost = gamma - option;
        if (Float.isNaN(current) || Double.isNaN(boost) || boost <= 0.0) {
            return current;
        }
        return (float) (current + boost);
    }

    public double gamma() {
        return gamma;
    }

    /** How often the hook has run (the self-test reads this). */
    public long applied() {
        return applied;
    }

    /** Runs the hook on a light-map state of the game's own type, as the game would hand it over. */
    @Override
    protected void probe(Object minecraft, List<String> out) throws Throwable {
        double option = optionGamma(minecraft);
        Object state = stateType.getConstructor().newInstance();
        writeBrightness.invokeExact(state, (float) option);
        Hooks.lightmap(state);
        float plain = (float) readBrightness.invokeExact(state);
        // The Darkness effect at 0.3: the game hands over max(0, option - 0.3).
        writeBrightness.invokeExact(state, (float) Math.max(0.0, option - 0.3));
        Hooks.lightmap(state);
        float darkened = (float) readBrightness.invokeExact(state);
        out.add(String.format(Locale.ROOT,
                "fullbright gamma=%.3f option=%.3f brightness=%.3f brightness_under_darkness_0.3=%.3f", gamma, option,
                plain, darkened));
    }
}
