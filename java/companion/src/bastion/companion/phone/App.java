package bastion.companion.phone;

import bastion.companion.ui.Icons;
import bastion.companion.ui.Kit;
import bastion.companion.ui.Rect;

/**
 * One app of the phone: a screen below the app's header. The phone owns navigation, the
 * status bar and the input rules; an app draws its content and answers what reaches it.
 */
abstract class App {
    protected final Phone phone;

    protected App(Phone phone) {
        this.phone = phone;
    }

    /** The id the phone navigates by ("spotify", "discord", …). */
    abstract String id();

    /** The name under the icon and in the header. */
    abstract String title();

    abstract Icons.Glyph icon();

    /** Whether the home screen shows this app (the launcher announced what it needs). */
    abstract boolean available();

    /** Became the screen shown (the phone was pulled out with it, or it was opened). */
    void shown() {
    }

    /** No longer shown (another screen, or the phone was put away). */
    void hidden() {
    }

    /** Draws the content into {@code area} (below the header). */
    abstract void render(Kit k, Rect area);

    /** Escape or Backspace: true when the app went back within itself. */
    boolean back() {
        return false;
    }

    /** A key not used by the phone's navigation; true when used. */
    boolean key(int key, int modifiers) {
        return false;
    }

    /** A typed character when {@link #capturesKeys()}; true when used. */
    boolean typed(int codePoint) {
        return false;
    }

    /** Every key goes to the app (the browser while its page has the keyboard). */
    boolean capturesKeys() {
        return false;
    }

    /** The wheel over the app's area; true when used. */
    boolean scroll(double x, double y, double amount) {
        return false;
    }

    /** A mouse button went down where no control is; true when used. */
    boolean press(double x, double y, int button) {
        return false;
    }

    boolean release(double x, double y, int button) {
        return false;
    }

    boolean drag(double x, double y, int button) {
        return false;
    }

    /** Twenty times a second while shown. */
    void tick() {
    }

    /** The phone is turned sideways for this app (the browser's reading mode). */
    boolean landscape() {
        return false;
    }

    /** Something to show at the right of the header (a status word), or null. */
    String badge() {
        return null;
    }

    /** The badge's tone ({@link Kit#ACCENT} …). */
    int badgeTone() {
        return Kit.NEUTRAL;
    }
}
