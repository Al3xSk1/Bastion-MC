package bastion.companion.phone;

import bastion.companion.platform.Canvas;
import bastion.companion.theme.Theme;
import bastion.companion.ui.Column;
import bastion.companion.ui.Gfx;
import bastion.companion.ui.Icons;
import bastion.companion.ui.Kit;
import bastion.companion.ui.Rect;
import bastion.companion.ui.Text;
import bastion.companion.ui.TextField;

/**
 * Realms: the launcher asks for a code before it lets the game join a Realm — the code it
 * shows in its own window or terminal, or one from an authenticator app (the user's choice in
 * the launcher). The phone shows the question and sends what is typed; the launcher checks it.
 * Verbs: code -- &lt;digits&gt;, cancel.
 */
final class RealmsApp extends App {
    private static final String APP = "realms";
    private final TextField code = TextField.digits(10);
    private boolean promptSeen;

    RealmsApp(Phone phone) {
        super(phone);
        code.onSubmit(this::submit);
    }

    @Override
    String id() {
        return APP;
    }

    @Override
    String title() {
        return "Realms";
    }

    @Override
    Icons.Glyph icon() {
        return Icons.TOWER;
    }

    @Override
    boolean available() {
        return phone.store.announced(APP);
    }

    @Override
    String badge() {
        AppStates.Realms s = phone.store.realms();
        return s.prompt != null ? "waiting" : s.mode;
    }

    @Override
    int badgeTone() {
        AppStates.Realms s = phone.store.realms();
        return s.prompt != null ? Kit.CAUTION : "off".equals(s.mode) ? Kit.NEUTRAL : Kit.ACCENT;
    }

    @Override
    void shown() {
        promptSeen = false;
    }

    private void submit() {
        String digits = code.text();
        if (digits.length() >= 4 && phone.store.realms().prompt != null) {
            phone.send.withText(APP, "code", digits);
            code.clear();
        }
    }

    @Override
    void render(Kit k, Rect area) {
        Canvas c = k.c;
        int line = c.lineHeight();
        AppStates.Realms s = phone.store.realms();
        Column col = new Column(area);
        k.caption(col.take(line + 1), "approval");
        String how = "code".equals(s.mode) ? "Type the code the launcher shows."
                : "totp".equals(s.mode) ? "Type the code from your authenticator app."
                        : "Off: the launcher joins Realms without asking.";
        col.gap(k.paragraph(col.rest().top(2 * line), how, Theme.TEXT_DIM));
        col.gap(5);
        AppStates.Prompt prompt = s.prompt;
        if (prompt == null) {
            promptSeen = false;
            col.gap(k.paragraph(col.rest().top(2 * line), "Nothing is waiting for a code.", Theme.TEXT_FAINT));
            if (s.last != null) {
                col.gap(3);
                int tone = "approved".equals(s.last) ? Theme.ACCENT : "refused".equals(s.last) ? Theme.RED : Theme.AMBER;
                Gfx.caps(c, "last: " + s.last, col.take(line + 1), tone, Gfx.LEFT);
            }
            return;
        }
        if (!promptSeen) {
            promptSeen = true;
            phone.hits.focus("rl-code");
        }
        Gfx.hline(c, area.x(), area.right(), col.take(1).y(), Theme.LINE);
        col.gap(3);
        Gfx.label(c, prompt.world().isEmpty() ? "A Realm" : prompt.world(), col.take(line), Theme.TEXT);
        if (!prompt.address().isEmpty()) {
            Gfx.label(c, prompt.address(), col.take(line), Theme.TEXT_FAINT);
        }
        long left = s.secondsLeft(k.now);
        col.gap(2);
        String expires = left > 0 ? "expires in " + Text.clock(left * 1000) : "expired";
        String tries = prompt.attemptsLeft() + (prompt.attemptsLeft() == 1 ? " try left" : " tries left");
        int expiresColor = left > 15 ? Theme.TEXT_DIM : Theme.AMBER;
        int triesColor = prompt.attemptsLeft() > 1 ? Theme.TEXT_DIM : Theme.AMBER;
        if (c.textWidth(expires) + c.textWidth(tries) + 6 <= area.w()) {
            Rect facts = col.take(line);
            Gfx.label(c, expires, facts, expiresColor, Gfx.LEFT, false);
            Gfx.label(c, tries, facts.rightCols(c.textWidth(tries)), triesColor, Gfx.RIGHT, false);
        } else {
            Gfx.label(c, expires, col.take(line), expiresColor);
            Gfx.label(c, tries, col.take(line), triesColor);
        }
        col.gap(4);
        k.field("rl-code", col.take(16), code, "code");
        col.gap(4);
        Rect buttons = col.take(14);
        k.button("rl-send", buttons.column(0, 2, 3), "approve", Kit.ACCENT,
                code.length() >= 4 && left > 0 ? this::submit : null);
        k.button("rl-cancel", buttons.column(1, 2, 3), "cancel", Kit.NEUTRAL, () -> {
            code.clear();
            phone.send.command(APP, "cancel");
        });
    }
}
