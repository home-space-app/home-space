use iced::{
    Event, event,
    keyboard::{self, Key, Modifiers, key::Named},
    mouse, window,
};

use super::Message;

const KEY_SELECT_ALL: &str = "a";
const KEY_NEW_FOLDER: &str = "n";
const KEY_UPLOAD: &str = "u";
const KEY_BACK: &str = "[";
const KEY_SEARCH: &str = "k";

/// A keyboard command of the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    SelectAll,
    Escape,
    Delete,
    Rename,
    Activate,
    Parent,
    Back,
    NewFolder,
    Upload,
    Refresh,
    Search,
    SelectPrevious,
    SelectNext,
    /// Shift+Up: grows or shrinks the selection one row upwards.
    ExtendPrevious,
    /// Shift+Down: grows or shrinks the selection one row downwards.
    ExtendNext,
    /// Space: opens the single selected folder or file; in the image viewer, shows the next one.
    Open,
    /// Right arrow: in the image viewer, shows the next image.
    Next,
    /// Left arrow: in the image viewer, shows the previous image.
    Previous,
}

pub fn input(event: Event, status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
            if status == event::Status::Ignored =>
        {
            shortcut(&key, modifiers).map(Message::Shortcut)
        }
        Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
            Some(Message::Modifiers(modifiers))
        }
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Back)) => {
            Some(Message::Shortcut(Shortcut::Back))
        }
        Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
        Event::Window(window::Event::FileHovered(_)) => Some(Message::FileHovered),
        Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FilesHoveredLeft),
        _ => None,
    }
}

fn shortcut(key: &Key, modifiers: Modifiers) -> Option<Shortcut> {
    match key.as_ref() {
        Key::Named(Named::Escape) => Some(Shortcut::Escape),
        Key::Named(Named::Delete) => Some(Shortcut::Delete),
        Key::Named(Named::F2) => Some(Shortcut::Rename),
        Key::Named(Named::F5) => Some(Shortcut::Refresh),
        Key::Named(Named::Enter) => Some(Shortcut::Activate),
        Key::Named(Named::Backspace) => Some(Shortcut::Back),
        Key::Named(Named::ArrowUp) if modifiers.alt() => Some(Shortcut::Parent),
        Key::Named(Named::ArrowLeft) if modifiers.alt() => Some(Shortcut::Back),
        Key::Named(Named::ArrowUp) if modifiers.is_empty() => Some(Shortcut::SelectPrevious),
        Key::Named(Named::ArrowDown) if modifiers.is_empty() => Some(Shortcut::SelectNext),
        Key::Named(Named::ArrowUp) if modifiers == Modifiers::SHIFT => {
            Some(Shortcut::ExtendPrevious)
        }
        Key::Named(Named::ArrowDown) if modifiers == Modifiers::SHIFT => Some(Shortcut::ExtendNext),
        Key::Named(Named::Space) if modifiers.is_empty() => Some(Shortcut::Open),
        Key::Named(Named::ArrowRight) if modifiers.is_empty() => Some(Shortcut::Next),
        Key::Named(Named::ArrowLeft) if modifiers.is_empty() => Some(Shortcut::Previous),
        Key::Character(KEY_BACK) if modifiers.command() => Some(Shortcut::Back),
        Key::Character(KEY_SELECT_ALL) if modifiers.command() => Some(Shortcut::SelectAll),
        Key::Character(KEY_NEW_FOLDER) if modifiers.command() => Some(Shortcut::NewFolder),
        Key::Character(KEY_UPLOAD) if modifiers.command() => Some(Shortcut::Upload),
        Key::Character(KEY_SEARCH) if modifiers.command() => Some(Shortcut::Search),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_need_the_command_modifier_for_letters() {
        let key = Key::Character(KEY_SELECT_ALL.into());
        assert_eq!(shortcut(&key, Modifiers::empty()), None);
        assert_eq!(
            shortcut(&key, Modifiers::COMMAND),
            Some(Shortcut::SelectAll)
        );
    }

    #[test]
    fn back_has_keyboard_shortcuts() {
        let backspace = Key::Named(Named::Backspace);
        assert_eq!(
            shortcut(&backspace, Modifiers::empty()),
            Some(Shortcut::Back)
        );
        let bracket = Key::Character(KEY_BACK.into());
        assert_eq!(shortcut(&bracket, Modifiers::COMMAND), Some(Shortcut::Back));
        assert_eq!(shortcut(&bracket, Modifiers::empty()), None);
        let left = Key::Named(Named::ArrowLeft);
        assert_eq!(shortcut(&left, Modifiers::ALT), Some(Shortcut::Back));
    }

    #[test]
    fn plain_arrows_move_the_selection() {
        let up = Key::Named(Named::ArrowUp);
        let down = Key::Named(Named::ArrowDown);
        assert_eq!(
            shortcut(&up, Modifiers::empty()),
            Some(Shortcut::SelectPrevious)
        );
        assert_eq!(
            shortcut(&down, Modifiers::empty()),
            Some(Shortcut::SelectNext)
        );
        assert_eq!(shortcut(&up, Modifiers::ALT), Some(Shortcut::Parent));
    }

    #[test]
    fn shift_arrows_extend_the_selection() {
        let up = Key::Named(Named::ArrowUp);
        let down = Key::Named(Named::ArrowDown);
        assert_eq!(
            shortcut(&up, Modifiers::SHIFT),
            Some(Shortcut::ExtendPrevious)
        );
        assert_eq!(
            shortcut(&down, Modifiers::SHIFT),
            Some(Shortcut::ExtendNext)
        );
        assert_eq!(shortcut(&down, Modifiers::SHIFT | Modifiers::COMMAND), None);
    }

    #[test]
    fn space_opens() {
        let space = Key::Named(Named::Space);
        assert_eq!(shortcut(&space, Modifiers::empty()), Some(Shortcut::Open));
        assert_eq!(shortcut(&space, Modifiers::SHIFT), None);
    }

    #[test]
    fn plain_left_and_right_arrows_step_between_items() {
        let left = Key::Named(Named::ArrowLeft);
        let right = Key::Named(Named::ArrowRight);
        assert_eq!(
            shortcut(&left, Modifiers::empty()),
            Some(Shortcut::Previous)
        );
        assert_eq!(shortcut(&right, Modifiers::empty()), Some(Shortcut::Next));
    }
}
