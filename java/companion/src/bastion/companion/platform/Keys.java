package bastion.companion.platform;

/**
 * Key, mouse-button and modifier numbers as {@link Overlay} receives them, the same on every
 * version.
 *
 * The numbering is GLFW's, which Minecraft used up to 1.21.x; adapters for versions with
 * another input layer (26.x reads SDL scan codes) translate into it. Letters and digits are
 * their upper-case ASCII codes ({@code 'A'} is 65, {@code '0'} is 48).
 */
public final class Keys {
    public static final int UNKNOWN = -1;

    public static final int SPACE = 32;
    public static final int APOSTROPHE = 39;
    public static final int COMMA = 44;
    public static final int MINUS = 45;
    public static final int PERIOD = 46;
    public static final int SLASH = 47;
    public static final int DIGIT_0 = 48;
    public static final int DIGIT_9 = 57;
    public static final int SEMICOLON = 59;
    public static final int EQUAL = 61;
    public static final int A = 65;
    public static final int C = 67;
    public static final int V = 86;
    public static final int X = 88;
    public static final int Z = 90;
    public static final int LEFT_BRACKET = 91;
    public static final int BACKSLASH = 92;
    public static final int RIGHT_BRACKET = 93;
    public static final int GRAVE = 96;

    public static final int ESCAPE = 256;
    public static final int ENTER = 257;
    public static final int TAB = 258;
    public static final int BACKSPACE = 259;
    public static final int INSERT = 260;
    public static final int DELETE = 261;
    public static final int RIGHT = 262;
    public static final int LEFT = 263;
    public static final int DOWN = 264;
    public static final int UP = 265;
    public static final int PAGE_UP = 266;
    public static final int PAGE_DOWN = 267;
    public static final int HOME = 268;
    public static final int END = 269;
    public static final int F1 = 290;
    public static final int F12 = 301;
    public static final int KEYPAD_ENTER = 335;
    public static final int LEFT_SHIFT = 340;
    public static final int LEFT_CONTROL = 341;
    public static final int LEFT_ALT = 342;
    public static final int LEFT_SUPER = 343;
    public static final int RIGHT_SHIFT = 344;
    public static final int RIGHT_CONTROL = 345;
    public static final int RIGHT_ALT = 346;
    public static final int RIGHT_SUPER = 347;

    public static final int MOUSE_LEFT = 0;
    public static final int MOUSE_RIGHT = 1;
    public static final int MOUSE_MIDDLE = 2;

    public static final int MOD_SHIFT = 1;
    public static final int MOD_CONTROL = 2;
    public static final int MOD_ALT = 4;
    public static final int MOD_SUPER = 8;

    private Keys() {
    }

    /** True for a letter key, {@code A} to {@code Z}. */
    public static boolean isLetter(int key) {
        return key >= A && key <= Z;
    }

    /** Whether {@code modifiers} has Control (and neither Alt nor Super): a shortcut like Ctrl+C. */
    public static boolean control(int modifiers) {
        return (modifiers & MOD_CONTROL) != 0 && (modifiers & (MOD_ALT | MOD_SUPER)) == 0;
    }

    /** A name for the key, as the browser app's vocabulary spells it, or null when it has none. */
    public static String browserName(int key, int modifiers) {
        if (control(modifiers)) {
            switch (key) {
                case A: return "ctrl-a";
                case C: return "ctrl-c";
                case V: return "ctrl-v";
                case X: return "ctrl-x";
                case Z: return "ctrl-z";
                default: return null;
            }
        }
        switch (key) {
            case ENTER:
            case KEYPAD_ENTER: return "Enter";
            case BACKSPACE: return "Backspace";
            case TAB: return "Tab";
            case ESCAPE: return "Escape";
            case DELETE: return "Delete";
            case HOME: return "Home";
            case END: return "End";
            case PAGE_UP: return "PageUp";
            case PAGE_DOWN: return "PageDown";
            case UP: return "ArrowUp";
            case DOWN: return "ArrowDown";
            case LEFT: return "ArrowLeft";
            case RIGHT: return "ArrowRight";
            default: return null;
        }
    }
}
