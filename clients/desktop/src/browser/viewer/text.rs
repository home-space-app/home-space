//! The text viewer: read-only until Edit, then a plain editor with Cancel and Save.

use iced::{
    Element, Length,
    widget::{text_editor, text_editor::Action},
};

use client_core::text::TextDoc;

use crate::theme;

const PADDING: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Read,
    Edit,
}

#[derive(Debug, Clone)]
pub enum Message {
    Action(Action),
}

pub struct State {
    doc: TextDoc,
    content: text_editor::Content,
    mode: Mode,
}

impl State {
    pub fn new(doc: TextDoc) -> Self {
        Self {
            content: text_editor::Content::with_text(&doc.text),
            doc,
            mode: Mode::Read,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn is_editable(&self) -> bool {
        self.doc.is_editable()
    }

    /// Whether the editor holds text that differs from the file.
    pub fn is_changed(&self) -> bool {
        self.mode == Mode::Edit && self.content.text() != self.doc.text
    }

    /// The text shown, with any edits.
    pub fn text(&self) -> String {
        self.content.text()
    }

    /// Starts editing, when the content can be saved back.
    pub fn edit(&mut self) {
        if self.doc.is_editable() {
            self.mode = Mode::Edit;
        }
    }

    /// Drops the edits and shows the file again.
    pub fn cancel(&mut self) {
        self.content = text_editor::Content::with_text(&self.doc.text);
        self.mode = Mode::Read;
    }

    /// The bytes to store for the edited text.
    pub fn encoded(&self) -> Vec<u8> {
        self.doc.encode(&self.content.text())
    }

    /// The edits were saved as `text`.
    pub fn saved(&mut self, text: &str) {
        self.doc = self.doc.with_text(text);
        self.mode = Mode::Read;
    }

    /// While reading, only moving around, selecting and copying take effect.
    pub fn update(&mut self, message: Message) {
        let Message::Action(action) = message;
        if self.mode == Mode::Edit || !action.is_edit() {
            self.content.perform(action);
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        text_editor(&self.content)
            .on_action(Message::Action)
            .font(theme::MONO)
            .size(theme::TEXT_SIZE)
            .padding(PADDING)
            .height(Length::Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use iced::widget::text_editor::Edit;

    use super::*;

    fn state(text: &str) -> State {
        State::new(TextDoc::decode(text.as_bytes()))
    }

    fn insert(state: &mut State, character: char) {
        state.update(Message::Action(Action::Edit(Edit::Insert(character))));
    }

    #[test]
    fn it_opens_read_only() {
        assert_eq!(state("hi").mode(), Mode::Read);
    }

    #[test]
    fn typing_is_ignored_while_reading() {
        let mut state = state("hi");
        insert(&mut state, 'x');
        assert_eq!(state.text(), "hi");
    }

    #[test]
    fn typing_changes_the_text_while_editing() {
        let mut state = state("hi");
        state.edit();
        insert(&mut state, 'x');
        assert_eq!(state.text(), "xhi");
    }

    #[test]
    fn cancel_restores_the_file() {
        let mut state = state("hi");
        state.edit();
        insert(&mut state, 'x');
        state.cancel();
        assert_eq!((state.text().as_str(), state.mode()), ("hi", Mode::Read));
    }

    #[test]
    fn content_that_is_not_utf8_cannot_be_edited() {
        let mut state = State::new(TextDoc::decode(b"\xFF"));
        state.edit();
        assert_eq!(state.mode(), Mode::Read);
    }

    #[test]
    fn saving_makes_the_edits_the_file() {
        let mut state = state("a\r\n");
        state.edit();
        insert(&mut state, 'b');
        let text = state.text();
        state.saved(&text);
        state.cancel();
        assert_eq!(state.text(), "ba\n");
        assert_eq!(state.encoded(), b"ba\r\n");
    }
}
