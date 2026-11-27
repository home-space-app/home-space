use iced::{Element, Length, Subscription, Task, Theme, theme::Mode, widget::container, window};

use crate::{app_link, app_link::AppLink, browser, login, protected, theme};

pub struct App {
    screen: Screen,
    /// The system's colour scheme, once it is known.
    mode: Mode,
    /// A `home-space://` link to open once the user is signed in.
    link: Option<AppLink>,
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
    /// The window is asked to close, which ends the app.
    CloseRequested,
    /// A `home-space://` link, handed over by a later start.
    Link(AppLink),
}

impl App {
    /// The app, opening `link` once signed in.
    pub fn new(link: Option<AppLink>) -> (Self, Task<Message>) {
        let (state, task) = login::State::restoring();
        let app = Self {
            screen: Screen::Login(state),
            mode: Mode::None,
            link,
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
            (screen, Message::CloseRequested) => {
                // The keys of unlocked folders go with the screen; the copies with the sweep.
                *screen = Screen::Login(login::State::default());
                protected::files::sweep();
                iced::exit()
            }
            (Screen::Browser(state), Message::Link(link)) => {
                let task = match state.update(browser::Message::OpenLink(link)) {
                    browser::Update::Task(task) => task.map(Message::Browser),
                    browser::Update::SignedOut { .. } => Task::none(),
                };
                Task::batch([task, focus()])
            }
            (Screen::Login(_), Message::Link(link)) => {
                self.link = Some(link);
                focus()
            }
            (Screen::Login(state), Message::Login(message)) => match state.update(message) {
                login::Update::Task(task) => task.map(Message::Login),
                login::Update::SignedIn { client, me } => {
                    let (mut state, task) = browser::State::open(*client, me);
                    let link = match self.link.take() {
                        Some(link) => match state.update(browser::Message::OpenLink(link)) {
                            browser::Update::Task(task) => task,
                            browser::Update::SignedOut { .. } => Task::none(),
                        },
                        None => Task::none(),
                    };
                    self.screen = Screen::Browser(Box::new(state));
                    Task::batch([task, link]).map(Message::Browser)
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
        Subscription::batch([
            iced::system::theme_changes().map(Message::Mode),
            window::close_requests().map(|_| Message::CloseRequested),
            app_link::listen().map(Message::Link),
            screen,
        ])
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

/// Brings the window to the front, for a link opened from the browser.
fn focus() -> Task<Message> {
    window::latest().and_then(window::gain_focus)
}
