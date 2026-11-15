//! A wrapper that tells where on the window a click landed, which `button` and `mouse_area`
//! don't, so a popup menu can open at the pointer or hang from the clicked widget.
use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    mouse::{self, Button, Cursor, Interaction},
};

/// What the wrapper learns about its position from the last pass over the overlays.
#[derive(Default)]
struct State {
    /// What turns a position inside a scrollable into one in the window.
    translation: Vector,
    /// The window bounds last sent through `on_move`.
    reported: Option<Rectangle>,
}

pub struct Anchor<'a, Message> {
    content: Element<'a, Message>,
    on_press: Option<Box<dyn Fn(Rectangle) -> Message + 'a>>,
    on_right_press: Option<Box<dyn Fn(Point) -> Message + 'a>>,
    on_move: Option<Box<dyn Fn(Rectangle) -> Message + 'a>>,
}

impl<'a, Message> Anchor<'a, Message> {
    pub fn new(content: impl Into<Element<'a, Message>>) -> Self {
        Self {
            content: content.into(),
            on_press: None,
            on_right_press: None,
            on_move: None,
        }
    }

    /// Sends the window bounds of the content on a left press, which then never reaches the
    /// content: a button inside only needs a handler to look enabled.
    pub fn on_press(mut self, on_press: impl Fn(Rectangle) -> Message + 'a) -> Self {
        self.on_press = Some(Box::new(on_press));
        self
    }

    /// Sends the window bounds of the content whenever they change, so a popup hanging from it
    /// can follow it: when the dialog around it grows, say.
    pub fn on_move(mut self, on_move: impl Fn(Rectangle) -> Message + 'a) -> Self {
        self.on_move = Some(Box::new(on_move));
        self
    }

    /// Sends the window position of the pointer on a right press the content left alone.
    pub fn on_right_press(mut self, on_right_press: impl Fn(Point) -> Message + 'a) -> Self {
        self.on_right_press = Some(Box::new(on_right_press));
        self
    }
}

impl<Message> Widget<Message, Theme, Renderer> for Anchor<'_, Message> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
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
        let state = tree.state.downcast_mut::<State>();
        let translation = state.translation;
        let bounds = layout.bounds() + translation;
        if let Some(on_move) = &self.on_move
            && state.reported != Some(bounds)
        {
            state.reported = Some(bounds);
            shell.publish(on_move(bounds));
        }
        let over = cursor.is_over(layout.bounds());

        if over
            && let Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) = event
            && let Some(on_press) = &self.on_press
        {
            shell.publish(on_press(bounds));
            shell.capture_event();
            return;
        }

        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        if shell.is_event_captured() {
            return;
        }

        if over
            && let Event::Mouse(mouse::Event::ButtonPressed(Button::Right)) = event
            && let Some(on_right_press) = &self.on_right_press
            && let Some(position) = cursor.position()
        {
            shell.publish(on_right_press(position + translation));
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        // The runtime asks for the overlays before it delivers events, which is the only place a
        // widget learns how far the scrollables around it have moved it.
        tree.state.downcast_mut::<State>().translation = translation;
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: 'a> From<Anchor<'a, Message>> for Element<'a, Message> {
    fn from(anchor: Anchor<'a, Message>) -> Self {
        Element::new(anchor)
    }
}
