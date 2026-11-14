package bastion.companion.platform.mojang;

import bastion.companion.platform.Host;
import bastion.companion.platform.Hosted;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.input.CharacterEvent;
import net.minecraft.client.input.KeyEvent;
import net.minecraft.client.input.MouseButtonEvent;
import net.minecraft.network.chat.Component;

/**
 * The game screen that shows one of this mod's overlays, for 26.x: drawing goes through the
 * game's render-state extraction ({@code extractRenderState}); input arrives as event records
 * whose numbers the family translates.
 */
final class HostScreen extends Screen implements Hosted {
    private final Host host;
    private final Base glue;

    HostScreen(Host host, Base glue) {
        super(Component.literal(""));
        this.host = host;
        this.glue = glue;
    }

    @Override
    public Host host() {
        return host;
    }

    @Override
    protected void init() {
        glue.textInput(this, true);
        host.init(width, height);
    }

    @Override
    public void tick() {
        host.tick();
    }

    @Override
    public void removed() {
        glue.textInput(this, false);
        host.removed();
    }

    @Override
    public boolean isPauseScreen() {
        return false;
    }

    @Override
    public boolean shouldCloseOnEsc() {
        return false;
    }

    @Override
    public void onClose() {
    }

    @Override
    public void extractRenderState(GuiGraphicsExtractor graphics, int mouseX, int mouseY, float partialTick) {
        if (host.gameBackground()) {
            extractBackground(graphics, mouseX, mouseY, partialTick);
        }
        host.render(new FamilyCanvas(graphics, width, height), mouseX, mouseY, partialTick);
    }

    @Override
    public boolean keyPressed(KeyEvent event) {
        return host.keyPressed(event.key(), glue.keys(event.key()), glue.modifiers(event.modifiers()));
    }

    @Override
    public boolean keyReleased(KeyEvent event) {
        return host.keyReleased(glue.keys(event.key()), glue.modifiers(event.modifiers()));
    }

    @Override
    public boolean charTyped(CharacterEvent event) {
        return host.charTyped(event.codepoint(), 0);
    }

    @Override
    public boolean mouseClicked(MouseButtonEvent event, boolean doubleClick) {
        return host.mousePressed(event.x(), event.y(), glue.button(event.button()), glue.modifiers(event.modifiers()));
    }

    @Override
    public boolean mouseReleased(MouseButtonEvent event) {
        return host.mouseReleased(event.x(), event.y(), glue.button(event.button()), glue.modifiers(event.modifiers()));
    }

    @Override
    public boolean mouseDragged(MouseButtonEvent event, double dx, double dy) {
        return host.mouseDragged(event.x(), event.y(), glue.button(event.button()), dx, dy);
    }

    @Override
    public boolean mouseScrolled(double x, double y, double horizontal, double vertical) {
        return host.mouseScrolled(x, y, horizontal, vertical);
    }
}
