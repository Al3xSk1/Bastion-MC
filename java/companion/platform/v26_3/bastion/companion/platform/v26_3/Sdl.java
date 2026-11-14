package bastion.companion.platform.v26_3;

import bastion.companion.platform.Keys;

/**
 * 26.3 reads input through SDL 3: keys by scan code (USB HID usage numbers), modifiers and
 * mouse buttons in SDL's numbering. Translated here into {@link Keys} numbering (GLFW's).
 */
final class Sdl {
    private Sdl() {
    }

    static int key(int scancode) {
        if (scancode >= 4 && scancode <= 29) {
            return Keys.A + (scancode - 4);
        }
        if (scancode >= 30 && scancode <= 38) {
            return '1' + (scancode - 30);
        }
        if (scancode >= 58 && scancode <= 69) {
            return Keys.F1 + (scancode - 58);
        }
        switch (scancode) {
            case 39: return '0';
            case 40: return Keys.ENTER;
            case 41: return Keys.ESCAPE;
            case 42: return Keys.BACKSPACE;
            case 43: return Keys.TAB;
            case 44: return Keys.SPACE;
            case 45: return Keys.MINUS;
            case 46: return Keys.EQUAL;
            case 47: return Keys.LEFT_BRACKET;
            case 48: return Keys.RIGHT_BRACKET;
            case 49: return Keys.BACKSLASH;
            case 51: return Keys.SEMICOLON;
            case 52: return Keys.APOSTROPHE;
            case 53: return Keys.GRAVE;
            case 54: return Keys.COMMA;
            case 55: return Keys.PERIOD;
            case 56: return Keys.SLASH;
            case 73: return Keys.INSERT;
            case 74: return Keys.HOME;
            case 75: return Keys.PAGE_UP;
            case 76: return Keys.DELETE;
            case 77: return Keys.END;
            case 78: return Keys.PAGE_DOWN;
            case 79: return Keys.RIGHT;
            case 80: return Keys.LEFT;
            case 81: return Keys.DOWN;
            case 82: return Keys.UP;
            case 88: return Keys.KEYPAD_ENTER;
            case 224: return Keys.LEFT_CONTROL;
            case 225: return Keys.LEFT_SHIFT;
            case 226: return Keys.LEFT_ALT;
            case 227: return Keys.LEFT_SUPER;
            case 228: return Keys.RIGHT_CONTROL;
            case 229: return Keys.RIGHT_SHIFT;
            case 230: return Keys.RIGHT_ALT;
            case 231: return Keys.RIGHT_SUPER;
            default: return Keys.UNKNOWN;
        }
    }

    /** SDL_Keymod: shift 0x0003, control 0x00C0, alt 0x0300, GUI 0x0C00. */
    static int modifiers(int sdl) {
        int out = 0;
        if ((sdl & 0x0003) != 0) {
            out |= Keys.MOD_SHIFT;
        }
        if ((sdl & 0x00C0) != 0) {
            out |= Keys.MOD_CONTROL;
        }
        if ((sdl & 0x0300) != 0) {
            out |= Keys.MOD_ALT;
        }
        if ((sdl & 0x0C00) != 0) {
            out |= Keys.MOD_SUPER;
        }
        return out;
    }

    /** SDL: left 1, middle 2, right 3; GLFW: left 0, right 1, middle 2. */
    static int button(int sdl) {
        switch (sdl) {
            case 1: return Keys.MOUSE_LEFT;
            case 2: return Keys.MOUSE_MIDDLE;
            case 3: return Keys.MOUSE_RIGHT;
            default: return sdl - 1;
        }
    }
}
