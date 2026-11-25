use iced::{Element, Length, Subscription, Task, Theme, theme::Mode, widget::container};

use crate::{browser, login, theme};

pub struct App {
    screen: Screen,
    /// The system's colour scheme, once it is known.
    mode: Mode,
}

enum Screen {
    Login(login::State),
    Browser(Box<browser::State>),
}

#[derive(Debug, Clone)]
pub enum Message {
    Login(login::Message),
    Browser(browser::Message),
    Mode(Mode),
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        let (state, task) = login::State::restoring();
        let app = Self {
            screen: Screen::Login(state),
            mode: Mode::None,
        };
        let mode = iced::system::theme().map(Message::Mode);
        (app, Task::batch([task.map(Message::Login), mode]))
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match (&mut self.screen, message) {
            (_, Message::Mode(mode)) => {
                self.mode = mode;
                Task::none()
            }
            (Screen::Login(state), Message::Login(message)) => match state.update(message) {
                login::Update::Task(task) => task.map(Message::Login),
                login::Update::SignedIn { client, me } => {
                    let (state, task) = browser::State::open(*client, me);
                    self.screen = Screen::Browser(Box::new(state));
                    task.map(Message::Browser)
                }
            },
            (Screen::Browser(state), Message::Browser(message)) => match state.update(message) {
                browser::Update::Task(task) => task.map(Message::Browser),
                browser::Update::SignedOut { server } => {
                    let (state, task) = login::State::signed_out(server);
                    self.screen = Screen::Login(state);
                    task.map(Message::Login)
                }
            },
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let screen = match &self.screen {
            Screen::Login(_) => login::subscription().map(Message::Login),
            Screen::Browser(state) => state.subscription().map(Message::Browser),
        };
        Subscription::batch([iced::system::theme_changes().map(Message::Mode), screen])
    }

    pub fn view(&self) -> Element<'_, Message> {
        let screen = match &self.screen {
            Screen::Login(state) => state.view().map(Message::Login),
            Screen::Browser(state) => state.view().map(Message::Browser),
        };
        container(screen)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::window)
            .into()
    }

    /// The theme for the system's colour scheme; until that is known iced picks one for it.
    pub fn theme(&self) -> Option<Theme> {
        match self.mode {
            Mode::None => None,
            Mode::Light => Some(theme::app(false)),
            Mode::Dark => Some(theme::app(true)),
        }
    }
}
