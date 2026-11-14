package bastion.companion.ui;

import java.util.ArrayList;
import java.util.List;

/**
 * Where on the screen something can be clicked, and the keyboard focus among those places.
 *
 * Screens are drawn anew every frame; while drawing, each control registers its rectangle
 * here ({@link #button}, {@link #slider}, {@link #field}). Mouse and keyboard events between
 * two frames are matched against the rectangles of the last complete frame — the ones the
 * player is looking at. Keyboard focus moves between focusable places by direction (the
 * nearest one that way), so that every screen can be used with the arrow keys and Enter as
 * well as with the mouse.
 */
public final class Hits {
    /** Reacts to a slider being pressed, dragged and released at a position. */
    public interface Drag {
        void at(double x, double y, boolean released);
    }

    /** One place. */
    public static final class Hit {
        public final String id;
        public final Rect rect;
        final Runnable click;
        final Drag drag;
        final TextField field;
        final boolean focusable;

        Hit(String id, Rect rect, Runnable click, Drag drag, TextField field, boolean focusable) {
            this.id = id;
            this.rect = rect;
            this.click = click;
            this.drag = drag;
            this.field = field;
            this.focusable = focusable;
        }

        public boolean isField() {
            return field != null;
        }
    }

    private List<Hit> building = new ArrayList<>();
    private List<Hit> shown = new ArrayList<>();
    private Rect bounds = Rect.EMPTY;
    private final java.util.ArrayDeque<Rect> clips = new java.util.ArrayDeque<>();
    private String focus;
    private Hit dragging;

    /** Starts a frame: places registered from now on replace the last frame's at {@link #end}. */
    public void begin(Rect visible) {
        building = new ArrayList<>();
        bounds = visible;
        clips.clear();
    }

    /** Places registered until {@link #popClip} are clickable only inside {@code r} as well (a scrolled list). */
    public void pushClip(Rect r) {
        clips.push(r);
    }

    public void popClip() {
        clips.poll();
    }

    /** Ends the frame: its places become the ones input is matched against. */
    public void end() {
        shown = building;
        if (focus != null && find(focus) == null) {
            focus = null;
        }
    }

    /** Forget everything: no places, no focus, no drag. */
    public void clear() {
        building = new ArrayList<>();
        shown = new ArrayList<>();
        focus = null;
        dragging = null;
    }

    private void add(Hit hit) {
        // Only the part that is visible can be clicked.
        Rect visible = hit.rect.intersect(bounds);
        for (Rect clip : clips) {
            visible = visible.intersect(clip);
        }
        if (!visible.empty()) {
            building.add(new Hit(hit.id, visible, hit.click, hit.drag, hit.field, hit.focusable));
        }
    }

    /** A control that does {@code click} when clicked or activated with Enter. */
    public void button(String id, Rect rect, Runnable click) {
        add(new Hit(id, rect, click, null, null, true));
    }

    /** A region that reacts to clicks but is not reached with the keyboard (a page, a backdrop). */
    public void area(String id, Rect rect, Runnable click) {
        add(new Hit(id, rect, click, null, null, false));
    }

    /** A control dragged with the mouse (a slider); keyboard: see the screen. */
    public void slider(String id, Rect rect, Drag drag) {
        add(new Hit(id, rect, null, drag, null, true));
    }

    /** A text field: focus sends typing to it. */
    public void field(String id, Rect rect, TextField field) {
        add(new Hit(id, rect, null, null, field, true));
    }

    // ------------------------------------------------------------------ queries

    private Hit find(String id) {
        for (Hit hit : shown) {
            if (hit.id.equals(id)) {
                return hit;
            }
        }
        return null;
    }

    /** The place at (x, y) — the one registered last, which is drawn on top — or null. */
    public Hit at(double x, double y) {
        for (int i = shown.size() - 1; i >= 0; i--) {
            Hit hit = shown.get(i);
            if (hit.rect.contains(x, y)) {
                return hit;
            }
        }
        return null;
    }

    public String focus() {
        return focus;
    }

    public boolean focused(String id) {
        return id != null && id.equals(focus);
    }

    public void focus(String id) {
        focus = id;
    }

    /** The text field that has the focus, or null. */
    public TextField focusedField() {
        Hit hit = focus == null ? null : find(focus);
        return hit == null ? null : hit.field;
    }

    // ------------------------------------------------------------------ input

    /** A mouse button went down at (x, y): true when it landed on a place. */
    public boolean press(double x, double y) {
        Hit hit = at(x, y);
        if (hit == null) {
            return false;
        }
        if (hit.focusable) {
            focus = hit.id;
        }
        if (hit.drag != null) {
            dragging = hit;
            hit.drag.at(x, y, false);
        } else if (hit.click != null) {
            hit.click.run();
        }
        return true;
    }

    /** The mouse moved with the button held. */
    public boolean drag(double x, double y) {
        if (dragging == null) {
            return false;
        }
        dragging.drag.at(x, y, false);
        return true;
    }

    /** The button came up. */
    public boolean release(double x, double y) {
        if (dragging == null) {
            return false;
        }
        Hit hit = dragging;
        dragging = null;
        hit.drag.at(x, y, true);
        return true;
    }

    public boolean dragging() {
        return dragging != null;
    }

    /** Enter or Space on the focused place: true when something was activated. */
    public boolean activate() {
        Hit hit = focus == null ? null : find(focus);
        if (hit == null || hit.click == null) {
            return false;
        }
        hit.click.run();
        return true;
    }

    /**
     * Moves the focus to the nearest focusable place in direction (dx, dy) — one of them is 0,
     * the other ±1 — or to the first one when nothing has the focus. True when it moved.
     */
    public boolean move(int dx, int dy) {
        Hit from = focus == null ? null : find(focus);
        if (from == null) {
            for (Hit hit : shown) {
                if (hit.focusable) {
                    focus = hit.id;
                    return true;
                }
            }
            return false;
        }
        double fx = from.rect.centerX();
        double fy = from.rect.centerY();
        Hit best = null;
        double bestScore = Double.MAX_VALUE;
        for (Hit hit : shown) {
            if (!hit.focusable || hit == from) {
                continue;
            }
            double ddx = hit.rect.centerX() - fx;
            double ddy = hit.rect.centerY() - fy;
            double along = ddx * dx + ddy * dy;
            if (along <= 0.5) {
                continue;
            }
            double across = Math.abs(dx != 0 ? ddy : ddx);
            // Prefer what is straight ahead: sideways distance counts double.
            double score = along + 2.0 * across;
            if (score < bestScore) {
                bestScore = score;
                best = hit;
            }
        }
        if (best == null) {
            return false;
        }
        focus = best.id;
        return true;
    }

    /** Tab: the next (or previous) focusable place in drawing order, wrapping around. */
    public boolean cycle(int step) {
        List<Hit> focusable = new ArrayList<>();
        for (Hit hit : shown) {
            if (hit.focusable) {
                focusable.add(hit);
            }
        }
        if (focusable.isEmpty()) {
            return false;
        }
        int at = -1;
        for (int i = 0; i < focusable.size(); i++) {
            if (focusable.get(i).id.equals(focus)) {
                at = i;
            }
        }
        int next = at < 0 ? (step > 0 ? 0 : focusable.size() - 1)
                : Math.floorMod(at + step, focusable.size());
        focus = focusable.get(next).id;
        return true;
    }

    /** The places of the last frame (for tests and the self-test). */
    public List<Hit> shown() {
        return shown;
    }
}
