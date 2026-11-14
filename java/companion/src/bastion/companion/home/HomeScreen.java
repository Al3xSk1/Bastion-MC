package bastion.companion.home;

import java.time.LocalTime;
import java.time.format.DateTimeFormatter;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.HomeAction;
import bastion.companion.platform.Keys;
import bastion.companion.platform.Overlay;
import bastion.companion.platform.Platform;
import bastion.companion.theme.Theme;
import bastion.companion.ui.Gfx;
import bastion.companion.ui.Hits;
import bastion.companion.ui.Icons;
import bastion.companion.ui.Kit;
import bastion.companion.ui.Rect;
import bastion.companion.ui.Ui;
import bastion.companion.ui.Clock;

/**
 * The home screen that takes the place of the game's title screen: an original dark design in
 * the launcher's colours — a rail on the left with the game's name and what the title screen
 * offers (single player, multiplayer, Realms, options, the mod list where there is one, quit;
 * in demo mode the demo world instead of the first three), and on the right an outline of the
 * phone with the key that pulls it out in a world.
 *
 * Nothing of the game's artwork is used; the captions are the game's own, in the player's
 * language ({@link Platform#label}). Every button does exactly what the title screen's button
 * does ({@link Platform#perform}). Keyboard: arrow keys or Tab, then Enter.
 */
public final class HomeScreen implements Overlay {
    private static final long FADE_NANOS = 350_000_000L;
    private static final String COPYRIGHT = "Copyright Mojang AB. Do not distribute!";

    private final Platform platform;
    private final java.util.function.BooleanSupplier phoneHint;
    private final Hits hits = new Hits();
    private long fadeStart;
    private boolean faded;

    public HomeScreen(Platform platform) {
        this(platform, Ui.PHONE::live);
    }

    /** {@code phoneHint}: whether to show the phone (its feature is on). */
    public HomeScreen(Platform platform, java.util.function.BooleanSupplier phoneHint) {
        this.platform = platform;
        this.phoneHint = phoneHint;
    }

    /** The buttons of the rail, in order: what the game offers now. */
    List<HomeAction> menu() {
        List<HomeAction> out = new ArrayList<>();
        if (platform.demo() || platform.available(HomeAction.PLAY_DEMO)) {
            out.add(HomeAction.PLAY_DEMO);
        } else {
            out.add(HomeAction.SINGLEPLAYER);
            out.add(HomeAction.MULTIPLAYER);
            out.add(HomeAction.REALMS);
        }
        out.add(HomeAction.OPTIONS);
        if (platform.available(HomeAction.MODS)) {
            out.add(HomeAction.MODS);
        }
        out.add(HomeAction.QUIT);
        return out;
    }

    @Override
    public void opened(int width, int height) {
        if (!faded && fadeStart == 0) {
            fadeStart = Clock.now();
        }
    }

    @Override
    public boolean closesOnEscape() {
        return false;
    }

    @Override
    public boolean gameBackground() {
        return false;
    }

    @Override
    public void closed() {
        hits.clear();
    }

    /** The width of the rail on a screen {@code width} wide. */
    static int railWidth(int width) {
        return Math.min(width, Math.max(Math.min(150, width), Math.min(230, Math.round(width * 0.36f))));
    }

    @Override
    public void render(Canvas c, int mouseX, int mouseY, float partialTick) {
        long now = Clock.now();
        int w = c.width();
        int h = c.height();
        Rect screen = new Rect(0, 0, w, h);
        c.clip(0, 0, w, h);
        try {
            hits.begin(screen);
            Kit k = new Kit(c, hits, mouseX, mouseY, now, null);
            c.fill(0, 0, w, h, Theme.VOID);
            Gfx.gradient(c, new Rect(0, 0, w, Math.max(1, h / 2)), Theme.PLATE, Theme.VOID, 3);
            int railW = railWidth(w);
            Rect rail = new Rect(0, 0, railW, h);
            Rect stage = new Rect(railW, 0, w - railW, h);
            c.fill(rail.x(), rail.y(), rail.right(), rail.bottom(), Theme.alpha(Theme.DECK, 0.9f));
            Gfx.vline(c, rail.right() - 1, 0, h, Theme.LINE);
            drawStage(k, stage);
            drawRail(k, rail.inset(Math.min(14, railW / 10), Math.min(12, h / 20)).leftOf(1));
            hits.end();
            if (!faded) {
                float t = (now - fadeStart) / (float) FADE_NANOS;
                if (t >= 1.0f) {
                    faded = true;
                } else {
                    c.fill(0, 0, w, h, Theme.alpha(Theme.VOID, 1.0f - Math.max(0.0f, t)));
                }
            }
        } finally {
            c.unclip();
        }
    }

    private void drawRail(Kit k, Rect area) {
        Canvas c = k.c;
        int line = c.lineHeight();
        // The game's name, large where there is room.
        String name = "Minecraft";
        int scale = Gfx.capsWidth(c, name) * 2 <= area.w() && area.h() >= 200 ? 2 : 1;
        int y = area.y() + 4;
        c.push();
        c.translate(area.x(), y);
        c.scale(scale);
        Gfx.caps(c, name, new Rect(0, 0, area.w() / scale, line), Theme.TEXT, Gfx.LEFT);
        c.pop();
        y += line * scale + 3;
        c.fill(area.x(), y, Math.min(area.right(), area.x() + 24), y + 1, Theme.ACCENT);
        y += 4;
        String version = platform.minecraftVersion();
        String demo = platform.demo() ? " · demo" : "";
        String edition = "Java Edition " + version + demo;
        if (Gfx.capsWidth(c, edition) > area.w()) {
            edition = version + demo;
        }
        if (Gfx.capsWidth(c, edition) > area.w()) {
            edition = version;
        }
        Gfx.caps(c, edition, new Rect(area.x(), y, area.w(), line), Theme.TEXT_FAINT, Gfx.LEFT);
        y += line + Math.max(6, Math.min(18, area.h() / 14));

        // The links at the bottom: language, accessibility, the classic title screen.
        int linkRows = 3;
        int bottomH = linkRows * (line + 3) + 4;
        Rect bottom = area.bottomRows(bottomH);
        int rowH = area.h() >= 300 ? 20 : area.h() >= 230 ? 18 : 16;
        List<HomeAction> items = menu();
        int menuTop = y;
        for (int i = 0; i < items.size(); i++) {
            HomeAction action = items.get(i);
            if (action == HomeAction.QUIT) {
                menuTop += 5;
            }
            Rect row = new Rect(area.x() - 4, menuTop, area.w() + 4, rowH);
            if (row.bottom() > bottom.y()) {
                break;
            }
            item(k, row, action);
            menuTop += rowH + 1;
        }
        int linkY = bottom.y();
        for (HomeAction action : new HomeAction[] {HomeAction.LANGUAGE, HomeAction.ACCESSIBILITY}) {
            if (!platform.available(action)) {
                continue;
            }
            String label = platform.label(action).replace("...", "").replace("\u2026", "").strip();
            Rect slot = new Rect(bottom.x(), linkY, bottom.w(), line + 3);
            int width = Math.min(slot.w(), c.textWidth(label) + 4);
            Rect link = slot.left(width);
            String id = "home:" + action.name();
            boolean hot = k.hot(id, link);
            Gfx.label(c, label, slot.inset(2, 1), hot ? Theme.ACCENT : Theme.TEXT_FAINT, Gfx.LEFT, false);
            hits.button(id, link, () -> act(action));
            linkY += line + 3;
        }
        Rect classic = new Rect(bottom.x(), linkY, bottom.w(), line + 3);
        String classicLabel = "Classic title screen";
        if (platform.available(HomeAction.CLASSIC_TITLE)) {
            boolean hot = k.hot("home:classic", classic);
            int width = Gfx.label(c, classicLabel, classic.inset(2, 1), hot ? Theme.ACCENT : Theme.TEXT_OFF, Gfx.LEFT,
                    false);
            hits.button("home:classic", classic.left(width + 4), () -> act(HomeAction.CLASSIC_TITLE));
        }
    }

    private void item(Kit k, Rect row, HomeAction action) {
        Canvas c = k.c;
        boolean enabled = platform.available(action);
        String id = "home:" + action.name();
        boolean hot = enabled && k.hot(id, row);
        if (hot) {
            c.fill(row.x(), row.y(), row.right(), row.bottom(), Theme.PLATE_HI);
            c.fill(row.x(), row.y(), row.x() + 2, row.bottom(), Theme.ACCENT);
        }
        String label = platform.label(action).replace("...", "").replace("\u2026", "").strip();
        int color = !enabled ? Theme.TEXT_OFF : hot ? Theme.ACCENT : Theme.TEXT;
        Gfx.caps(c, label, row.inset(8, 1), color, Gfx.LEFT);
        if (enabled) {
            hits.button(id, row, () -> act(action));
        }
    }

    private void act(HomeAction action) {
        platform.clickSound();
        platform.perform(action);
    }

    /** The right part: an outline of the phone, and the copyright line in the corner. */
    private void drawStage(Kit k, Rect stage) {
        Canvas c = k.c;
        int line = c.lineHeight();
        if (stage.w() < 60 || stage.h() < 80) {
            return;
        }
        // The copyright line (it opens the credits, as on the game's title screen).
        String copyright = COPYRIGHT;
        int room = stage.w() - 12;
        if (c.textWidth(copyright) > room) {
            copyright = "Copyright Mojang AB.";
        }
        if (c.textWidth(copyright) > room) {
            copyright = "© Mojang AB";
        }
        int cw = Math.min(room, c.textWidth(copyright));
        Rect credit = new Rect(stage.right() - 6 - cw, stage.bottom() - line - 3, cw, line);
        boolean hot = platform.available(HomeAction.CREDITS) && k.hot("home:credits", credit);
        Gfx.label(c, copyright, credit, hot ? Theme.TEXT : Theme.TEXT_FAINT, Gfx.RIGHT, false);
        if (platform.available(HomeAction.CREDITS)) {
            hits.area("home:credits", credit, () -> act(HomeAction.CREDITS));
        }
        if (!phoneHint.getAsBoolean()) {
            return;
        }
        // The phone, standing in the middle of the stage above the copyright line.
        Rect room2 = stage.inset(12, 12).aboveBottom(line + 6);
        int ph = Math.min(Math.round(room2.h() * 0.78f), 200);
        int pw = Math.round(ph * 0.56f);
        if (pw > room2.w()) {
            pw = room2.w();
            ph = Math.round(pw / 0.56f);
        }
        if (ph < 70) {
            return;
        }
        Rect phone = new Rect(room2.centerX() - pw / 2, room2.y() + (room2.h() - ph) / 2, pw, ph);
        c.roundRect(phone.x(), phone.y(), phone.w(), phone.h(), 8, Theme.LINE_HI);
        c.roundRect(phone.x() + 1, phone.y() + 1, phone.w() - 2, phone.h() - 2, 7, Theme.DECK);
        Rect screen = phone.inset(4);
        c.roundRect(screen.x(), screen.y(), screen.w(), screen.h(), 3, Theme.VOID);
        int slit = Math.min(14, phone.w() / 6);
        c.fill(phone.centerX() - slit / 2, phone.y() + 2, phone.centerX() + slit / 2, phone.y() + 3, Theme.LINE);
        // On its screen: the time, a few tiles, and the key.
        String time = LocalTime.now().format(DateTimeFormatter.ofPattern("HH:mm", Locale.ROOT));
        int scale = c.textWidth(time) * 2 <= screen.w() - 8 && screen.h() >= 110 ? 2 : 1;
        int tw = c.textWidth(time) * scale;
        c.push();
        c.translate(screen.centerX() - tw / 2, screen.y() + 10);
        c.scale(scale);
        c.text(time, 0, 0, Theme.TEXT, false);
        c.pop();
        int tilesTop = screen.y() + 10 + line * scale + 10;
        int tile = Math.min(18, (screen.w() - 16) / 3 - 4);
        Icons.Glyph[] glyphs = {Icons.MUSIC, Icons.CHAT, Icons.GLOBE, Icons.CLIP, Icons.TOWER, Icons.GEAR};
        if (tile >= 10) {
            int gridW = 3 * tile + 2 * 4;
            int gx = screen.centerX() - gridW / 2;
            for (int i = 0; i < 6; i++) {
                int tx = gx + (i % 3) * (tile + 4);
                int ty = tilesTop + (i / 3) * (tile + 4);
                Rect t = new Rect(tx, ty, tile, tile);
                if (t.bottom() > screen.bottom() - 2 * line - 6) {
                    break;
                }
                Gfx.frame(c, t, Theme.LINE_HI);
                Icons.drawIn(c, glyphs[i], t.inset(2), 1, Theme.alpha(Theme.ACCENT, 0.8f));
            }
        }
        String key = platform.phoneKeyName();
        if (key != null && !key.isEmpty()) {
            Rect hint = new Rect(screen.x() + 2, screen.bottom() - 2 * line - 4, screen.w() - 4, line);
            String press = Gfx.capsThatFits(c, hint.w(), "press " + key, key);
            if (press != null) {
                Gfx.caps(c, press, hint, Theme.ACCENT, Gfx.CENTER);
            }
            String what = Gfx.capsThatFits(c, hint.w(), "for the phone", "phone");
            if (what != null) {
                Gfx.caps(c, what, hint.moved(0, line), Theme.TEXT_FAINT, Gfx.CENTER);
            }
        }
    }

    // ------------------------------------------------------------------ input

    @Override
    public boolean keyPressed(int key, int modifiers) {
        switch (key) {
            case Keys.UP:
            case Keys.LEFT:
                hits.move(0, -1);
                return true;
            case Keys.DOWN:
            case Keys.RIGHT:
                hits.move(0, 1);
                return true;
            case Keys.TAB:
                hits.cycle((modifiers & Keys.MOD_SHIFT) != 0 ? -1 : 1);
                return true;
            case Keys.ENTER:
            case Keys.KEYPAD_ENTER:
            case Keys.SPACE:
                hits.activate();
                return true;
            case Keys.ESCAPE:
                return true;
            default:
                return false;
        }
    }

    @Override
    public boolean mousePressed(double x, double y, int button, int modifiers) {
        return button == Keys.MOUSE_LEFT && hits.press(x, y);
    }

    /** The places of the last frame (the self-test presses them by name). */
    public Hits hits() {
        return hits;
    }
}
