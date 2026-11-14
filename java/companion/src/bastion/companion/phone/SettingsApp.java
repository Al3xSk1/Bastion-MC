package bastion.companion.phone;

import bastion.companion.platform.Canvas;
import bastion.companion.theme.Theme;
import bastion.companion.ui.Column;
import bastion.companion.ui.Gfx;
import bastion.companion.ui.Icons;
import bastion.companion.ui.Kit;
import bastion.companion.ui.Rect;

/**
 * Settings: whether the game's settings are kept in step across instances (the launcher does
 * the syncing, outside the game), when that last happened, and a button to do it now; and a
 * few facts about the phone itself. Verbs: status, sync-now.
 */
final class SettingsApp extends App {
    private static final String APP = "settings";

    SettingsApp(Phone phone) {
        super(phone);
    }

    @Override
    String id() {
        return APP;
    }

    @Override
    String title() {
        return "Settings";
    }

    @Override
    Icons.Glyph icon() {
        return Icons.GEAR;
    }

    @Override
    boolean available() {
        return phone.store.announced(APP);
    }

    @Override
    void shown() {
        phone.send.command(APP, "status");
    }

    /** A name on the left (faint capitals) and its value on the right; the value keeps its room first. */
    static void fact(Canvas c, Rect row, String name, String value, int color) {
        int valueWidth = Math.min(c.textWidth(value), row.w() * 3 / 5);
        Gfx.label(c, value, row.rightCols(valueWidth), color, Gfx.RIGHT, false);
        Gfx.caps(c, name, row.leftOf(valueWidth + 4), Theme.TEXT_FAINT, Gfx.LEFT);
    }

    @Override
    void render(Kit k, Rect area) {
        Canvas c = k.c;
        int line = c.lineHeight();
        AppStates.Settings s = phone.store.settings();
        Column col = new Column(area);
        k.caption(col.take(line + 1), "settings sync");
        col.gap(2);
        fact(c, col.take(line), "sync", s.sync ? "on" : "off", s.sync ? Theme.ACCENT : Theme.TEXT_DIM);
        fact(c, col.take(line), "last", s.lastSync.isEmpty() ? "never" : s.lastSync, Theme.TEXT);
        if (!s.keys.isEmpty()) {
            fact(c, col.take(line), "settings", s.keys, Theme.TEXT);
        }
        if (!s.instances.isEmpty()) {
            fact(c, col.take(line), "instances", s.instances, Theme.TEXT);
        }
        if (!s.instanceNames.isEmpty()) {
            Gfx.label(c, String.join(", ", s.instanceNames), col.take(line), Theme.TEXT_FAINT);
        }
        if (!s.error.isEmpty()) {
            col.gap(2);
            col.gap(k.paragraph(col.rest().top(2 * line), s.error, Theme.RED));
        }
        col.gap(4);
        k.button("st-sync", col.take(14), "sync now", Kit.ACCENT,
                s.sync ? () -> phone.send.command(APP, "sync-now") : null);
        if (!s.sync) {
            col.gap(2);
            col.gap(k.paragraph(col.rest().top(2 * line), "Sync is off in the launcher's config ([sync]).",
                    Theme.TEXT_FAINT));
        }
        col.gap(8);
        if (col.left() >= 4 * line) {
            Gfx.hline(c, area.x(), area.right(), col.take(1).y(), Theme.LINE);
            col.gap(3);
            k.caption(col.take(line + 1), "phone");
            fact(c, col.take(line), "launcher", phone.store.connected() ? "connected" : "offline",
                    phone.store.connected() ? Theme.ACCENT : Theme.RED);
            fact(c, col.take(line), "minecraft", phone.platform.minecraftVersion(), Theme.TEXT);
            String key = phone.platform.phoneKeyName();
            if (key != null && !key.isEmpty()) {
                fact(c, col.take(line), "phone key", key, Theme.TEXT);
            }
        }
    }
}
