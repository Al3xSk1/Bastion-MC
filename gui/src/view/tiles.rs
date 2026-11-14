//! One line of a grid of tiles: side by side in equal columns, all as high as the highest.
//!
//! The toolkit's rows cannot do this. In a row whose height comes from its content — and
//! every row on a page that scrolls is one — a child that is meant to fill the height is
//! laid out before anything has been measured, and comes out as low as nothing. So a line
//! of tiles is laid out here, twice: once to find how high each tile wants to be, once
//! more with every tile at the height of the highest.
//!
//! A tile that is to stretch says `height(Fill)` (and so do the parts of it that are to
//! stretch with it): while it is measured that counts as "as high as the content", in the
//! second pass it is exactly the height of the line. A tile of any other height is given
//! the line's height as its least height. What is inside keeps its own size.
//!
//! Only the layout is this widget's own. Events, drawing, the operations through which
//! the tests find controls by their ids, and overlays go to the tiles in order, exactly as
//! a row passes them on.
//!
//! The line must stand where its width is known (every page is handed its width); the
//! width is shared out among `columns` places whether or not there is a tile for each, so
//! a short last line of a grid has tiles as wide as the lines above it.

use iced::{
    Element, Event, Length, Point, Rectangle, Size, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree},
    },
};

pub struct Tiles<'a, Message, Theme, Renderer> {
    tiles: Vec<Element<'a, Message, Theme, Renderer>>,
    columns: usize,
    spacing: f32,
}

impl<'a, Message, Theme, Renderer> Tiles<'a, Message, Theme, Renderer> {
    /// `tiles` in a line of `columns` places (at least one), `spacing` apart. Tiles beyond
    /// `columns` would have no place: the caller makes one line per `columns` tiles.
    pub fn new(tiles: Vec<Element<'a, Message, Theme, Renderer>>, columns: usize, spacing: f32) -> Self {
        let columns = columns.max(1);
        debug_assert!(tiles.len() <= columns, "{} tiles in a line of {columns}", tiles.len());
        Self { tiles, columns, spacing }
    }

    /// How wide each place is in a line `width` wide.
    fn cell(&self, width: f32) -> f32 {
        ((width - self.spacing * (self.columns - 1) as f32) / self.columns as f32).max(0.0)
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for Tiles<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn children(&self) -> Vec<Tree> {
        self.tiles.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.tiles);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let width = limits.max().width;
        let cell = self.cell(width);
        // First pass: as wide as a place, as high as the content (a tile that fills its
        // height is taken at its content's height here, which is what the compression of
        // the height means to the toolkit).
        let measuring = layout::Limits::with_compression(
            Size::new(cell, 0.0),
            Size::new(cell, limits.max().height),
            Size::new(false, true),
        );
        let mut height = 0.0_f32;
        for (tile, state) in self.tiles.iter_mut().zip(&mut tree.children) {
            height = height.max(tile.as_widget_mut().layout(state, renderer, &measuring).size().height);
        }
        // Second pass: every tile exactly as high as the highest.
        let exact = layout::Limits::new(Size::new(cell, height), Size::new(cell, height));
        let nodes = self
            .tiles
            .iter_mut()
            .zip(&mut tree.children)
            .enumerate()
            .map(|(place, (tile, state))| {
                tile.as_widget_mut()
                    .layout(state, renderer, &exact)
                    .move_to(Point::new(place as f32 * (cell + self.spacing), 0.0))
            })
            .collect();
        layout::Node::with_children(Size::new(width, height), nodes)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((tile, state), layout) in self.tiles.iter_mut().zip(&mut tree.children).zip(layout.children()) {
                tile.as_widget_mut().operate(state, layout, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for ((tile, state), layout) in self.tiles.iter_mut().zip(&mut tree.children).zip(layout.children()) {
            tile.as_widget_mut().update(state, event, layout, cursor, renderer, clipboard, shell, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.tiles
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((tile, state), layout)| {
                tile.as_widget().mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((tile, state), layout) in self.tiles.iter().zip(&tree.children).zip(layout.children()) {
            if layout.bounds().intersects(viewport) {
                tile.as_widget().draw(state, renderer, theme, style, layout, cursor, viewport);
            }
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(&mut self.tiles, tree, layout, renderer, viewport, translation)
    }
}

impl<'a, Message, Theme, Renderer> From<Tiles<'a, Message, Theme, Renderer>> for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(tiles: Tiles<'a, Message, Theme, Renderer>) -> Self {
        Element::new(tiles)
    }
}
