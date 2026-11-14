package bastion.companion.theme;

/**
 * The look of everything this mod draws: one dark theme, the same family of colours as the
 * launcher's own front end ({@code gui/src/theme.rs}) — near-black gunmetal surfaces, one cool
 * accent, amber for caution, red for danger.
 *
 * <p>The rules, as in the front end:
 * <ul>
 * <li><b>One accent, two alarms.</b> {@link #ACCENT} means "on / selected / go"; {@link #AMBER}
 * caution; {@link #RED} danger or failure. No other hue is used, so a colour always says one of
 * those three things — and never alone: every state also has a word.
 * <li><b>Lines, not cards.</b> Flat, nearly black surfaces; structure from hairlines
 * ({@link #LINE}, {@link #LINE_HI}).
 * <li><b>Text first.</b> {@link #TEXT} on {@link #PLATE}; {@link #TEXT_FAINT} is the faintest
 * colour used for anything that has to be read.
 * </ul>
 *
 * Colours are {@code 0xAARRGGBB}, opaque unless a name says otherwise.
 */
public final class Theme {
    /** The deepest background: the home screen, the phone's screen. */
    public static final int VOID = 0xFF0A0D10;
    /** A shade below: the phone's body, the home screen's side rail. */
    public static final int DECK = 0xFF070A0C;
    /** Panels. */
    public static final int PLATE = 0xFF10161B;
    /** A panel under the pointer or selected. */
    public static final int PLATE_HI = 0xFF16202A;
    /** Recessed areas: text fields, the page area. */
    public static final int WELL = 0xFF05080A;
    /** Hairlines. */
    public static final int LINE = 0xFF1F2B35;
    /** Hairlines that have to be seen: outlines of controls, scales. */
    public static final int LINE_HI = 0xFF33485A;

    /** On, selected, go. */
    public static final int ACCENT = 0xFF3FD9E6;
    /** Caution. */
    public static final int AMBER = 0xFFF5B041;
    /** Danger, failure, destructive. */
    public static final int RED = 0xFFFF5F56;

    public static final int TEXT = 0xFFDBE6EC;
    /** Secondary text. */
    public static final int TEXT_DIM = 0xFF9BB0BD;
    /** Captions: the faintest text that still has to be read. */
    public static final int TEXT_FAINT = 0xFF738A9A;
    /** Controls that are shown but cannot be used now. */
    public static final int TEXT_OFF = 0xFF4A5C69;

    /** Behind vanilla menus in a world: the world stays faintly visible. */
    public static final int SCRIM_WORLD = 0xD00A0D10;
    /** Behind vanilla menus outside a world. */
    public static final int SCRIM_MENU = 0xFF0A0D10;

    private Theme() {
    }

    /** {@code argb} with its alpha replaced by {@code alpha} (0 to 1). */
    public static int alpha(int argb, float alpha) {
        int a = Math.round(Math.max(0.0f, Math.min(1.0f, alpha)) * 255.0f);
        return (a << 24) | (argb & 0x00FFFFFF);
    }

    /** The colour {@code t} of the way from {@code from} to {@code to} (t from 0 to 1), alpha included. */
    public static int mix(int from, int to, float t) {
        float u = Math.max(0.0f, Math.min(1.0f, t));
        int a = channel(from >>> 24, to >>> 24, u);
        int r = channel((from >> 16) & 0xFF, (to >> 16) & 0xFF, u);
        int g = channel((from >> 8) & 0xFF, (to >> 8) & 0xFF, u);
        int b = channel(from & 0xFF, to & 0xFF, u);
        return (a << 24) | (r << 16) | (g << 8) | b;
    }

    private static int channel(int from, int to, float t) {
        return Math.round(from + (to - from) * t);
    }

    /**
     * The contrast ratio of two opaque colours (WCAG's definition, 1 to 21). The tests use it
     * to keep the palette readable, as the front end's tests do.
     */
    public static double contrast(int a, int b) {
        double la = luminance(a);
        double lb = luminance(b);
        return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
    }

    private static double luminance(int argb) {
        return 0.2126 * linear((argb >> 16) & 0xFF) + 0.7152 * linear((argb >> 8) & 0xFF)
                + 0.0722 * linear(argb & 0xFF);
    }

    private static double linear(int channel) {
        double c = channel / 255.0;
        return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
    }
}
