//! A column as wide as its widest child, with every child stretched to that width.
//!
//! iced's `column` cannot do this by itself: when it shrinks to fit, its fill-width
//! children only get the width of the other, non-fill children, so a column made only of
//! fill-width buttons collapses to nothing.
use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, overlay, renderer,
        widget::{Operation, Tree},
    },
    mouse::{Cursor, Interaction},
};

pub struct FitColumn<'a, Message> {
    children: Vec<Element<'a, Message>>,
}

/// A [`FitColumn`] of `children`, top to bottom.
pub fn fit_column<'a, Message>(
    children: impl IntoIterator<Item = Element<'a, Message>>,
) -> FitColumn<'a, Message> {
    FitColumn {
        children: children.into_iter().collect(),
    }
}

impl<Message> Widget<Message, Theme, Renderer> for FitColumn<'_, Message> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let max = limits.max();
        // Shrink limits make fill-width children report the width their content needs.
        let natural = layout::Limits::new(Size::ZERO, max).width(Length::Shrink);
        let width = self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .map(|(child, tree)| {
                child
                    .as_widget_mut()
                    .layout(tree, renderer, &natural)
                    .size()
                    .width
            })
            .fold(0.0, f32::max);

        let stretched = layout::Limits::new(Size::ZERO, Size::new(width, max.height));
        let (nodes, height) = self.children.iter_mut().zip(&mut tree.children).fold(
            (Vec::new(), 0.0),
            |(mut nodes, y), (child, tree)| {
                let node = child
                    .as_widget_mut()
                    .layout(tree, renderer, &stretched)
                    .move_to((0.0, y));
                let y = y + node.size().height;
                nodes.push(node);
                (nodes, y)
            },
        );

        layout::Node::with_children(
            limits.resolve(width, height, Size::new(width, height)),
            nodes,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
    ) {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .for_each(|((child, tree), layout)| {
                child
                    .as_widget()
                    .draw(tree, renderer, theme, style, layout, cursor, viewport);
            });
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .for_each(|((child, tree), layout)| {
                    child
                        .as_widget_mut()
                        .operate(tree, layout, renderer, operation);
                });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .for_each(|((child, tree), layout)| {
                child.as_widget_mut().update(
                    tree, event, layout, cursor, renderer, clipboard, shell, viewport,
                );
            });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, tree), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: 'a> From<FitColumn<'a, Message>> for Element<'a, Message> {
    fn from(column: FitColumn<'a, Message>) -> Self {
        Self::new(column)
    }
}
