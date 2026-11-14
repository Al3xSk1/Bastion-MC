package bastion.companion.ui;

import java.util.function.IntPredicate;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Keys;
import bastion.companion.theme.Theme;

/**
 * One line of text the player types: a search, a code, a new name. Holds at most
 * {@code maxCodePoints} characters, and only those {@code allowed} lets in (control and
 * direction-changing characters never). The cursor moves with the arrow keys, Home and End;
 * Backspace and Delete remove; Enter calls {@code submit}.
 */
public final class TextField {
    private final StringBuilder text = new StringBuilder();
    private final int maxCodePoints;
    private final IntPredicate allowed;
    /** Cursor position, in code points. */
    private int cursor;
    private Runnable submit = () -> {
    };

    public TextField(int maxCodePoints, IntPredicate allowed) {
        this.maxCodePoints = maxCodePoints;
        this.allowed = allowed;
    }

    /** Any printable text. */
    public static TextField plain(int maxCodePoints) {
        return new TextField(maxCodePoints, c -> true);
    }

    /** Digits only (a one-time code). */
    public static TextField digits(int maxDigits) {
        return new TextField(maxDigits, c -> c >= '0' && c <= '9');
    }

    public TextField onSubmit(Runnable action) {
        submit = action == null ? () -> {
        } : action;
        return this;
    }

    public String text() {
        return text.toString();
    }

    public boolean isEmpty() {
        return text.length() == 0;
    }

    public int length() {
        return text.codePointCount(0, text.length());
    }

    public void set(String value) {
        text.setLength(0);
        cursor = 0;
        if (value != null) {
            value.codePoints().forEach(this::insert);
        }
    }

    public void clear() {
        set("");
    }

    private boolean insert(int c) {
        if (Character.isISOControl(c) || Text.isBidiControl(c) || c == 0xA7
                || (c >= Character.MIN_SURROGATE && c <= Character.MAX_SURROGATE) || !Character.isValidCodePoint(c)
                || !allowed.test(c) || length() >= maxCodePoints) {
            return false;
        }
        int at = text.offsetByCodePoints(0, cursor);
        text.insert(at, Character.toChars(c));
        cursor++;
        return true;
    }

    /** A typed character. True when it was taken. */
    public boolean typed(int codePoint) {
        return insert(codePoint);
    }

    /**
     * A key for the field. True when the field used it; with the field focused the phone
     * also swallows letter keys, so that typing never triggers a binding.
     */
    public boolean key(int key, int modifiers) {
        switch (key) {
            case Keys.BACKSPACE:
                if (cursor > 0) {
                    int end = text.offsetByCodePoints(0, cursor);
                    int start = text.offsetByCodePoints(0, cursor - 1);
                    text.delete(start, end);
                    cursor--;
                }
                return true;
            case Keys.DELETE:
                if (cursor < length()) {
                    int start = text.offsetByCodePoints(0, cursor);
                    int end = text.offsetByCodePoints(0, cursor + 1);
                    text.delete(start, end);
                }
                return true;
            case Keys.LEFT:
                cursor = Math.max(0, cursor - 1);
                return true;
            case Keys.RIGHT:
                cursor = Math.min(length(), cursor + 1);
                return true;
            case Keys.HOME:
                cursor = 0;
                return true;
            case Keys.END:
                cursor = length();
                return true;
            case Keys.ENTER:
            case Keys.KEYPAD_ENTER:
                submit.run();
                return true;
            default:
                return false;
        }
    }

    /**
     * Draws the field in r: a recessed well, the text (scrolled so the cursor stays visible),
     * a blinking cursor while focused, {@code placeholder} in faint text while empty.
     */
    public void render(Canvas c, Rect r, boolean focused, long nowNanos, String placeholder) {
        if (r.empty()) {
            return;
        }
        c.fill(r.x(), r.y(), r.right(), r.bottom(), Theme.WELL);
        Gfx.frame(c, r, focused ? Theme.ACCENT : Theme.LINE_HI);
        Rect inner = r.inset(3, 1);
        if (!Gfx.fitsLine(c, inner)) {
            return;
        }
        int y = Gfx.textY(c, inner);
        int caretTop = Math.max(inner.y(), y - 1);
        int caretBottom = Math.min(inner.bottom(), y + c.lineHeight() - 1);
        String value = text.toString();
        if (value.isEmpty()) {
            Gfx.label(c, placeholder, inner.rightOf(2), Theme.TEXT_FAINT);
        } else {
            // The part of the text that fits, ending at the cursor if the cursor is beyond.
            int caretAt = text.offsetByCodePoints(0, cursor);
            String before = value.substring(0, caretAt);
            int room = inner.w() - 2;
            int start = 0;
            while (start < before.length() && c.textWidth(before.substring(start)) > room) {
                start += Character.charCount(before.codePointAt(start));
            }
            String visible = value.substring(start);
            int end = visible.length();
            while (end > 0 && c.textWidth(visible.substring(0, end)) > room) {
                end -= Character.charCount(visible.codePointBefore(end));
            }
            visible = visible.substring(0, end);
            c.text(visible, inner.x(), y, Theme.TEXT, false);
            if (focused && blink(nowNanos)) {
                int caretX = inner.x() + c.textWidth(value.substring(start, Math.max(start, caretAt)));
                caretX = Math.min(caretX, inner.right() - 1);
                c.fill(caretX, caretTop, caretX + 1, caretBottom, Theme.ACCENT);
            }
            return;
        }
        if (focused && blink(nowNanos)) {
            c.fill(inner.x(), caretTop, inner.x() + 1, caretBottom, Theme.ACCENT);
        }
    }

    private static boolean blink(long nowNanos) {
        return (nowNanos / 500_000_000L) % 2 == 0;
    }
}
