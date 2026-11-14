package bastion.companion.platform.intermediary;

import bastion.companion.platform.Canvas;
import bastion.companion.platform.Host;
import bastion.companion.platform.Hosted;
import net.minecraft.class_2561;
import net.minecraft.class_332;
import net.minecraft.class_437;

/**
 * The game screen that shows one of this mod's overlays (intermediary families): a subclass
 * of the game's {@code Screen} (class_437) whose every part hands over to the {@link Host}.
 * It does not pause the game, has no widgets of the game's, and leaves Escape to the host.
 * The families add the input methods (their signatures changed in 1.21.9), the background
 * call and the canvas.
 */
public abstract class BaseScreen extends class_437 implements Hosted {
    protected final Host host;

    protected BaseScreen(Host host) {
        super(class_2561.method_43470(""));
        this.host = host;
    }

    @Override
    public Host host() {
        return host;
    }

    /** init(): set up, and again after every resize. */
    @Override
    protected void method_25426() {
        host.init(field_22789, field_22790);
    }

    /** tick() */
    @Override
    public void method_25393() {
        host.tick();
    }

    /** removed(): another screen took this one's place. */
    @Override
    public void method_25432() {
        host.removed();
    }

    /** isPauseScreen() */
    @Override
    public boolean method_25421() {
        return false;
    }

    /** shouldCloseOnEsc() */
    @Override
    public boolean method_25422() {
        return false;
    }

    /** onClose(): nothing; the host decides when its overlay closes. */
    @Override
    public void method_25419() {
    }

    /** render(GuiGraphics, mouseX, mouseY, partialTick) */
    @Override
    public void method_25394(class_332 graphics, int mouseX, int mouseY, float partialTick) {
        if (host.gameBackground()) {
            background(graphics, mouseX, mouseY, partialTick);
        }
        host.render(canvas(graphics), mouseX, mouseY, partialTick);
    }

    /** The game's own background for this screen (its signature changed in 1.20.2). */
    protected abstract void background(class_332 graphics, int mouseX, int mouseY, float partialTick);

    /** A canvas on {@code graphics}, the size of this screen. */
    protected abstract Canvas canvas(class_332 graphics);
}
