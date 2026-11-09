use std::time::Duration;

use api_client::{ApiError, Client};
use iced::{
    Alignment, ContentFit, Element, Length, Padding, Subscription, Task,
    keyboard::{self, Key, key::Named},
    task,
    widget::{
        button, column, container, operation, row, scrollable, space, stack, svg, text, text_input,
    },
};
use protocol::{Me, Providers};

use crate::{
    browser::parts::{FIELD_PADDING, alert_error, field, outlined, primary},
    icons::{ICON_SIZE, Icon, Tint},
    session::{self, Restored, Saved},
    sso, theme,
};

const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";
/// Shown for an `http://` server outside the home network, where the password would cross
/// the internet unencrypted.
const UNPROTECTED_SERVER: &str = "Use https:// for a server outside your home network";
const CARD_WIDTH: f32 = 400.0;
const CARD_PADDING: f32 = 32.0;
const FORM_GAP: f32 = 20.0;
const SHADOW_ROOM: [f32; 2] = [72.0, 40.0];
const GLOW: &[u8] = include_bytes!("../assets/login_glow.svg");
const LOGO_SIZE: f32 = 52.0;
const LOGO_ICON: f32 = 30.0;
const BRAND_GAP: f32 = 12.0;
const TITLE_SIZE: f32 = 24.0;
const SERVER_PADDING: Padding = Padding {
    top: 9.0,
    right: 38.0,
    bottom: 9.0,
    left: 38.0,
};
const PASSWORD_PADDING: Padding = Padding {
    top: 9.0,
    right: 38.0,
    bottom: 9.0,
    left: 10.0,
};
const FIELD_OVERLAY_PADDING: [f32; 2] = [0.0, 10.0];
const EYE_OVERLAY_PADDING: [f32; 2] = [0.0, 6.0];
const EYE_SIZE: f32 = 16.0;
const EYE_PADDING: f32 = 6.0;
const DIVIDER: f32 = 1.0;
const OR_SIZE: f32 = 12.0;
const OR_GAP: f32 = 12.0;
const WIDE_ICON: f32 = 16.0;
const WIDE_GAP: f32 = 8.0;
const BUTTON_HEIGHT: f32 = 40.0;
const FAILED_WIDTH: f32 = 420.0;
const FAILED_DISC: f32 = 56.0;
const FAILED_ICON: f32 = 30.0;
const FAILED_GAP: f32 = 16.0;
const FAILED_BUTTON_GAP: f32 = 8.0;
const FAILED_BUTTONS_PADDING: Padding = Padding {
    top: 8.0,
    right: 0.0,
    bottom: 0.0,
    left: 0.0,
};
const HEADING_SIZE: f32 = 18.0;
const RESUME_FAILED: &str = "Could not resume the session";
/// How long the server field must rest before the server is asked how users sign in.
const PROBE_DELAY: Duration = Duration::from_millis(600);

#[derive(Debug, Clone)]
pub enum Message {
    ServerChanged(String),
    UsernameChanged(String),
    PasswordChanged(String),
    Submit,
    TogglePassword,
    SignInWithProvider,
    Cancel,
    /// Tries the saved session again after it could not be resumed.
    RetryResume,
    /// Gives up on the saved session and shows the form.
    ForgetSession,
    /// The server field has rested for [`PROBE_DELAY`]; carries the probe generation and server.
    Probe(u64, String),
    ProvidersLoaded(u64, Result<Providers, String>),
    FocusNext,
    FocusPrevious,
    Finished(Result<(Client, Me), String>),
    /// The session saved by an earlier run has been read, before it is resumed.
    SavedLoaded(Option<Saved>),
    Restored(Restored),
}

pub enum Update {
    Task(Task<Message>),
    SignedIn { client: Client, me: Me },
}

pub struct State {
    server: String,
    username: String,
    password: String,
    show_password: bool,
    busy: bool,
    /// The session saved by an earlier run is being resumed.
    restoring: bool,
    error: Option<String>,
    /// Why the saved session could not be resumed, though it may still be good.
    resume_failed: Option<String>,
    /// How the server lets users sign in, once known.
    providers: Option<Providers>,
    /// The server has not answered the first question about how users sign in, so the
    /// password form stays hidden rather than flashing before the provider button replaces it.
    first_probe_pending: bool,
    /// The server in the field did not answer.
    unreachable: bool,
    /// Bumped whenever the server changes, so answers about an earlier one are ignored.
    probe: u64,
    /// The browser sign-in in progress, if any.
    sso: Option<task::Handle>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.to_owned(),
            username: String::new(),
            password: String::new(),
            show_password: false,
            busy: false,
            restoring: false,
            error: None,
            resume_failed: None,
            providers: None,
            first_probe_pending: true,
            unreachable: false,
            probe: 0,
            sso: None,
        }
    }
}

impl State {
    /// The login screen, busy while it resumes the session saved by an earlier run.
    pub fn restoring() -> (Self, Task<Message>) {
        let state = Self {
            busy: true,
            restoring: true,
            ..Self::default()
        };
        (
            state,
            Task::perform(session::load_saved(), Message::SavedLoaded),
        )
    }

    /// The login screen after signing out of `server`, which it offers again.
    pub fn signed_out(server: String) -> (Self, Task<Message>) {
        let mut state = Self {
            server,
            ..Self::default()
        };
        let task = state.schedule_probe();
        (state, task)
    }

    pub fn update(&mut self, message: Message) -> Update {
        match message {
            Message::ServerChanged(_) if self.restoring => Update::Task(Task::none()),
            Message::ServerChanged(value) => {
                self.server = value;
                Update::Task(self.schedule_probe())
            }
            Message::Probe(probe, server) if probe == self.probe && !server.is_empty() => {
                Update::Task(Task::perform(fetch_providers(server), move |result| {
                    Message::ProvidersLoaded(probe, result)
                }))
            }
            Message::Probe(probe, _) => {
                if probe == self.probe {
                    self.first_probe_pending = false;
                }
                Update::Task(Task::none())
            }
            Message::ProvidersLoaded(probe, result) => {
                // An unreachable server leaves the password form, which reports the problem.
                if probe == self.probe {
                    self.unreachable = result.is_err();
                    self.providers = result.ok();
                    self.first_probe_pending = false;
                }
                Update::Task(Task::none())
            }
            Message::SignInWithProvider | Message::Submit if self.is_unprotected() => {
                self.error = Some(UNPROTECTED_SERVER.to_owned());
                Update::Task(Task::none())
            }
            Message::SignInWithProvider if self.busy => Update::Task(Task::none()),
            Message::SignInWithProvider => {
                self.busy = true;
                self.error = None;
                let (task, handle) =
                    Task::perform(sso::sign_in(self.server_url()), Message::Finished).abortable();
                self.sso = Some(handle);
                Update::Task(task)
            }
            Message::Cancel => {
                if let Some(handle) = self.sso.take() {
                    handle.abort();
                }
                self.busy = false;
                Update::Task(Task::none())
            }
            Message::UsernameChanged(value) => {
                self.username = value;
                Update::Task(Task::none())
            }
            Message::PasswordChanged(value) => {
                self.password = value;
                Update::Task(Task::none())
            }
            Message::TogglePassword => {
                self.show_password = !self.show_password;
                Update::Task(Task::none())
            }
            Message::RetryResume => {
                let (state, task) = Self::restoring();
                *self = state;
                Update::Task(task)
            }
            Message::ForgetSession => {
                self.resume_failed = None;
                let probe = self.schedule_probe();
                let forget = Task::perform(session::forget_saved(), |()| ()).discard();
                Update::Task(Task::batch([forget, probe]))
            }
            Message::FocusNext => Update::Task(operation::focus_next()),
            Message::FocusPrevious => Update::Task(operation::focus_previous()),
            Message::Submit if self.busy => Update::Task(Task::none()),
            Message::Submit => {
                self.busy = true;
                self.error = None;
                let server = self.server_url();
                let username = self.username.trim().to_owned();
                let password = self.password.clone();
                Update::Task(Task::perform(
                    sign_in(server, username, password),
                    Message::Finished,
                ))
            }
            Message::Finished(Ok((client, me)))
            | Message::Restored(Restored::SignedIn { client, me }) => {
                Update::SignedIn { client, me }
            }
            Message::Finished(Err(err)) => {
                self.busy = false;
                self.sso = None;
                self.error = Some(err);
                Update::Task(Task::none())
            }
            Message::SavedLoaded(Some(saved)) => {
                self.server.clone_from(&saved.server);
                Update::Task(Task::perform(session::resume(saved), Message::Restored))
            }
            Message::SavedLoaded(None) => {
                self.busy = false;
                self.restoring = false;
                Update::Task(self.schedule_probe())
            }
            Message::Restored(Restored::Failed { server, error }) => {
                self.busy = false;
                self.restoring = false;
                if error == ApiError::Unauthorized {
                    self.server = server;
                    return Update::Task(self.schedule_probe());
                }
                self.resume_failed = Some(resume_reason(&server, &error));
                self.server = server;
                Update::Task(Task::none())
            }
        }
    }

    /// What is known about the server in the field.
    fn server_state(&self) -> ServerState {
        if self.restoring {
            ServerState::Resuming
        } else if self.is_unprotected() {
            ServerState::Unprotected
        } else if let Some(providers) = &self.providers {
            ServerState::Connected(sign_in_methods(providers))
        } else if self.unreachable {
            ServerState::Unreachable
        } else if self.server_url().is_empty() {
            ServerState::Empty
        } else {
            ServerState::Contacting
        }
    }

    fn server_url(&self) -> String {
        self.server.trim().trim_end_matches('/').to_owned()
    }

    /// Whether signing in to the server in the field would send the password over the
    /// internet unencrypted.
    fn is_unprotected(&self) -> bool {
        api_client::is_unprotected(&self.server_url())
    }

    /// Asks the server how users sign in once the server field stops changing.
    fn schedule_probe(&mut self) -> Task<Message> {
        self.probe += 1;
        self.providers = None;
        self.unreachable = false;
        let (probe, server) = (self.probe, self.server_url());
        Task::perform(
            async move {
                tokio::time::sleep(PROBE_DELAY).await;
                (probe, server)
            },
            |(probe, server)| Message::Probe(probe, server),
        )
    }

    pub fn view(&self) -> Element<'_, Message> {
        if let Some(reason) = &self.resume_failed {
            return centered(resume_failed(reason));
        }
        let mut form = column![brand(), self.server_field()]
            .spacing(FORM_GAP)
            .width(Length::Fill);
        let provider = self
            .providers
            .as_ref()
            .and_then(|providers| providers.oidc.as_ref());
        if let Some(provider) = provider {
            let label = if self.busy {
                "Waiting for the browser…".to_owned()
            } else {
                format!("Sign in with {}", provider.name)
            };
            form = form.push(wide_button(
                Some((Icon::Key, Tint::OnAccent)),
                label,
                theme::primary,
                (!self.busy).then_some(Message::SignInWithProvider),
            ));
            if self.sso.is_some() {
                form = form.push(wide_button(
                    None,
                    "Cancel".to_owned(),
                    theme::secondary,
                    Some(Message::Cancel),
                ));
            }
        }
        let password_login = !self.first_probe_pending
            && self
                .providers
                .as_ref()
                .is_none_or(|providers| providers.password);
        if password_login {
            if provider.is_some() {
                form = form.push(or_divider());
            }
            form = form
                .push(field(
                    "Username",
                    text_input("", &self.username)
                        .style(theme::input)
                        .on_input(Message::UsernameChanged)
                        .on_submit(Message::Submit)
                        .padding(FIELD_PADDING),
                    None,
                ))
                .push(self.password_field())
                .push(wide_button(
                    None,
                    if self.busy {
                        "Signing in…"
                    } else {
                        "Sign in"
                    }
                    .to_owned(),
                    if provider.is_some() {
                        theme::secondary
                    } else {
                        theme::primary
                    },
                    (!self.busy).then_some(Message::Submit),
                ));
        }
        if let Some(error) = &self.error {
            form = form.push(alert_error(Icon::Error, error.as_str(), None));
        }
        let card = container(form)
            .padding(CARD_PADDING)
            .width(CARD_WIDTH)
            .style(theme::panel);
        stack![glow(), centered(card.into())].into()
    }

    /// The server's address, with what is known about it in the field and under it.
    fn server_field(&self) -> Element<'_, Message> {
        let state = self.server_state();
        let input = text_input("", &self.server)
            .style(theme::input)
            .padding(SERVER_PADDING)
            .on_input_maybe((!self.restoring).then_some(Message::ServerChanged));
        let overlay = row![Icon::Dns.view(ICON_SIZE, Tint::Muted), space::horizontal()]
            .push(match state {
                ServerState::Connected(_) => Some(Icon::Synced.view(ICON_SIZE, Tint::Success)),
                ServerState::Unreachable | ServerState::Unprotected => {
                    Some(Icon::Error.view(ICON_SIZE, Tint::Danger))
                }
                _ => None,
            })
            .align_y(Alignment::Center)
            .padding(FIELD_OVERLAY_PADDING)
            .height(Length::Fill);
        field("Server", stack![input, overlay], state.hint())
    }

    fn password_field(&self) -> Element<'_, Message> {
        let input = text_input("", &self.password)
            .style(theme::input)
            .secure(!self.show_password)
            .on_input(Message::PasswordChanged)
            .on_submit(Message::Submit)
            .padding(PASSWORD_PADDING);
        let eye = if self.show_password {
            Icon::VisibilityOff
        } else {
            Icon::Visibility
        };
        let overlay = row![
            space::horizontal(),
            button(eye.view(EYE_SIZE, Tint::Muted))
                .padding(EYE_PADDING)
                .style(theme::tool)
                .on_press(Message::TogglePassword),
        ]
        .align_y(Alignment::Center)
        .padding(EYE_OVERLAY_PADDING)
        .height(Length::Fill);
        field("Password", stack![input, overlay], None)
    }
}

/// What is known about the server in the field.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerState {
    Empty,
    Resuming,
    Contacting,
    Unreachable,
    /// An `http://` address outside the home network; signing in there is refused.
    Unprotected,
    /// It answered; how users sign in there, in words.
    Connected(String),
}

impl ServerState {
    fn hint(&self) -> Option<String> {
        match self {
            Self::Empty => None,
            Self::Resuming => Some("Resuming your session…".to_owned()),
            Self::Contacting => Some("Contacting the server…".to_owned()),
            Self::Unreachable => Some("Could not reach the server".to_owned()),
            Self::Unprotected => Some(UNPROTECTED_SERVER.to_owned()),
            Self::Connected(methods) => Some(methods.clone()),
        }
    }
}

fn sign_in_methods(providers: &Providers) -> String {
    match (providers.password, providers.oidc.is_some()) {
        (true, true) => "Connected · password and single sign-on",
        (true, false) => "Connected · password",
        (false, true) => "Connected · single sign-on",
        (false, false) => "Connected",
    }
    .to_owned()
}

/// The end of a chain of causes such as "error sending request: tcp connect error: Connection
/// refused (os error 61)", which is the part that means something to a user.
fn last_cause(reason: &str) -> String {
    let cause = reason.rsplit(": ").next().unwrap_or(reason);
    let cause = cause.split(" (os error").next().unwrap_or(cause);
    let mut letters = cause.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(letters).collect()
    })
}

/// `content` in the middle of the window, scrolling when the window is too short for it. The
/// padding keeps the card's shadow inside what the scrolling clips.
fn centered(content: Element<'_, Message>) -> Element<'_, Message> {
    let room = container(content)
        .padding(SHADOW_ROOM)
        .center_x(Length::Fill);
    container(scrollable(room).width(Length::Fill))
        .center(Length::Fill)
        .into()
}

/// The faint glow behind the card.
fn glow<'a>() -> Element<'a, Message> {
    svg(svg::Handle::from_memory(GLOW))
        .width(Length::Fill)
        .height(Length::Fill)
        .content_fit(ContentFit::Fill)
        .style(|theme: &iced::Theme, _status| svg::Style {
            color: Some(theme::tokens(theme).accent),
        })
        .into()
}

/// The mark and the name of the app.
fn brand<'a>() -> Element<'a, Message> {
    column![
        container(Icon::Home.view(LOGO_ICON, Tint::OnAccent))
            .center(LOGO_SIZE)
            .style(theme::big_logo),
        column![
            text("Home Space").size(TITLE_SIZE).font(theme::BOLD),
            text("Sign in to your files").style(theme::muted_text),
        ]
        .align_x(Alignment::Center),
    ]
    .spacing(BRAND_GAP)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn or_divider<'a>() -> Element<'a, Message> {
    let line = || {
        container(space())
            .width(Length::Fill)
            .height(DIVIDER)
            .style(theme::divider)
    };
    row![
        line(),
        text("or").size(OR_SIZE).style(theme::muted_text),
        line()
    ]
    .spacing(OR_GAP)
    .align_y(Alignment::Center)
    .into()
}

fn wide_button<'a>(
    icon: Option<(Icon, Tint)>,
    label: String,
    style: fn(&iced::Theme, button::Status) -> button::Style,
    message: Option<Message>,
) -> Element<'a, Message> {
    let line = row![]
        .push(icon.map(|(icon, tint)| icon.view(WIDE_ICON, tint)))
        .push(text(label).font(theme::MEDIUM))
        .spacing(WIDE_GAP)
        .align_y(Alignment::Center);
    button(container(line).center(Length::Fill))
        .padding(0)
        .width(Length::Fill)
        .height(BUTTON_HEIGHT)
        .style(style)
        .on_press_maybe(message)
        .into()
}

/// Why the saved session could not be resumed, and what can be done.
fn resume_failed(reason: &str) -> Element<'_, Message> {
    container(
        column![
            container(Icon::CloudOff.view(FAILED_ICON, Tint::Danger))
                .center(FAILED_DISC)
                .style(theme::danger_disc),
            text(RESUME_FAILED).size(HEADING_SIZE).font(theme::SEMIBOLD),
            text(reason)
                .style(theme::muted_text)
                .align_x(Alignment::Center)
                .width(Length::Fill),
            container(
                row![
                    outlined(None, "Sign out", Some(Message::ForgetSession)),
                    primary(Some(Icon::Refresh), "Retry", Some(Message::RetryResume)),
                ]
                .spacing(FAILED_BUTTON_GAP)
            )
            .padding(FAILED_BUTTONS_PADDING),
        ]
        .spacing(FAILED_GAP)
        .align_x(Alignment::Center),
    )
    .padding(CARD_PADDING)
    .width(FAILED_WIDTH)
    .style(theme::panel)
    .into()
}

/// Why `server` could not resume the session, in words.
fn resume_reason(server: &str, error: &ApiError) -> String {
    let host = server
        .split_once("://")
        .map_or(server, |(_, rest)| rest)
        .trim_end_matches('/');
    match error {
        ApiError::Transport(reason) => {
            format!(
                "{host} didn't answer: {}. Check that you're on your home network or VPN.",
                last_cause(reason)
            )
        }
        error => format!("{host}: {error}"),
    }
}

pub fn subscription() -> Subscription<Message> {
    keyboard::listen().filter_map(|event| match event {
        keyboard::Event::KeyPressed {
            key: Key::Named(Named::Tab),
            modifiers,
            ..
        } if modifiers.shift() => Some(Message::FocusPrevious),
        keyboard::Event::KeyPressed {
            key: Key::Named(Named::Tab),
            ..
        } => Some(Message::FocusNext),
        _ => None,
    })
}

async fn sign_in(
    server: String,
    username: String,
    password: String,
) -> Result<(Client, Me), String> {
    let client = session::client(&server);
    let me = client
        .login(&username, &password)
        .await
        .map_err(|err| err.to_string())?;
    Ok((client, me))
}

async fn fetch_providers(server: String) -> Result<Providers, String> {
    Client::new(server)
        .providers()
        .await
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAVED_SERVER: &str = "http://nas.local:8080";

    #[test]
    fn an_expired_saved_session_shows_the_form_for_its_server() {
        let mut state = State::default();

        let _ = state.update(Message::Restored(Restored::Failed {
            server: SAVED_SERVER.to_owned(),
            error: ApiError::Unauthorized,
        }));

        assert_eq!(
            (state.server.as_str(), state.busy, state.error),
            (SAVED_SERVER, false, None)
        );
    }

    fn providers(password: bool) -> Providers {
        Providers {
            password,
            oidc: Some(protocol::OidcProvider {
                name: "Keycloak".to_owned(),
            }),
        }
    }

    #[test]
    fn the_login_screen_after_signing_out_asks_the_server_how_to_sign_in() {
        let (state, _task) = State::signed_out(SAVED_SERVER.to_owned());

        assert_eq!(state.probe, 1);
        assert!(state.first_probe_pending);
        assert_eq!(state.server, SAVED_SERVER);
    }

    #[test]
    fn the_password_form_waits_for_the_first_answer_and_returns_if_there_is_none() {
        let mut state = State::default();
        let _ = state.update(Message::ServerChanged("http://nas.local".to_owned()));
        assert!(state.first_probe_pending);

        let _ = state.update(Message::ProvidersLoaded(
            state.probe,
            Err("down".to_owned()),
        ));

        assert!(!state.first_probe_pending);
        assert_eq!(state.providers, None);
    }

    #[test]
    fn providers_for_the_current_server_are_kept() {
        let mut state = State::default();
        let _ = state.update(Message::ServerChanged("http://nas.local".to_owned()));

        let _ = state.update(Message::ProvidersLoaded(state.probe, Ok(providers(false))));

        assert_eq!(state.providers, Some(providers(false)));
    }

    #[test]
    fn providers_for_an_earlier_server_are_ignored() {
        let mut state = State::default();
        let _ = state.update(Message::ServerChanged("http://old.local".to_owned()));
        let stale = state.probe;
        let _ = state.update(Message::ServerChanged("http://new.local".to_owned()));

        let _ = state.update(Message::ProvidersLoaded(stale, Ok(providers(true))));

        assert_eq!(state.providers, None);
    }

    #[test]
    fn changing_the_server_forgets_its_providers() {
        let mut state = State::default();
        let _ = state.update(Message::ProvidersLoaded(state.probe, Ok(providers(true))));

        let _ = state.update(Message::ServerChanged("http://other.local".to_owned()));

        assert_eq!(state.providers, None);
    }

    #[test]
    fn signing_in_over_http_across_the_internet_is_refused() {
        let mut state = State::default();
        let _ = state.update(Message::ServerChanged(
            "http://files.example.com".to_owned(),
        ));

        let _ = state.update(Message::Submit);

        assert_eq!(state.server_state(), ServerState::Unprotected);
        assert_eq!(
            (state.busy, state.error.as_deref()),
            (false, Some(UNPROTECTED_SERVER))
        );
        let _ = state.update(Message::ServerChanged("http://nas.local:8080".to_owned()));
        assert_ne!(state.server_state(), ServerState::Unprotected);
    }

    #[test]
    fn a_saved_session_shows_its_server_while_it_resumes() {
        let (mut state, _task) = State::restoring();

        let _ = state.update(Message::SavedLoaded(Some(Saved {
            server: SAVED_SERVER.to_owned(),
            refresh_token: "token".to_owned(),
        })));

        assert_eq!(
            (state.server.as_str(), state.restoring),
            (SAVED_SERVER, true)
        );
    }

    #[test]
    fn the_server_field_is_editable_once_resuming_fails() {
        let (mut state, _task) = State::restoring();
        let _ = state.update(Message::ServerChanged("http://typed.local".to_owned()));
        assert_eq!(state.server, DEFAULT_SERVER);

        let _ = state.update(Message::Restored(Restored::Failed {
            server: SAVED_SERVER.to_owned(),
            error: ApiError::Unauthorized,
        }));
        let _ = state.update(Message::ServerChanged("http://typed.local".to_owned()));

        assert_eq!(state.server, "http://typed.local");
    }

    #[test]
    fn the_server_field_says_what_is_known_about_the_server() {
        let hint = |state: &State| state.server_state().hint();
        let (mut state, _task) = State::restoring();
        assert_eq!(hint(&state).as_deref(), Some("Resuming your session…"));

        let _ = state.update(Message::SavedLoaded(None));
        assert_eq!(hint(&state).as_deref(), Some("Contacting the server…"));

        let _ = state.update(Message::ProvidersLoaded(state.probe, Ok(providers(true))));
        assert_eq!(
            hint(&state).as_deref(),
            Some("Connected · password and single sign-on")
        );

        let _ = state.update(Message::ServerChanged("http://down.local".to_owned()));
        let _ = state.update(Message::ProvidersLoaded(
            state.probe,
            Err("down".to_owned()),
        ));
        assert_eq!(hint(&state).as_deref(), Some("Could not reach the server"));

        let _ = state.update(Message::ServerChanged(String::new()));
        assert_eq!(hint(&state), None);
    }

    #[test]
    fn the_methods_say_how_users_sign_in() {
        assert_eq!(
            sign_in_methods(&providers(false)),
            "Connected · single sign-on"
        );
        let password_only = Providers {
            password: true,
            oidc: None,
        };
        assert_eq!(sign_in_methods(&password_only), "Connected · password");
    }

    fn failed_to_resume() -> State {
        let (mut state, _task) = State::restoring();
        let _ = state.update(Message::Restored(Restored::Failed {
            server: SAVED_SERVER.to_owned(),
            error: ApiError::Transport("connection refused".to_owned()),
        }));
        state
    }

    #[test]
    fn a_server_that_does_not_answer_keeps_the_session_for_a_retry() {
        let state = failed_to_resume();
        assert_eq!(
            state.resume_failed.as_deref(),
            Some(
                "nas.local:8080 didn't answer: connection refused. Check that you're on your \
                 home network or VPN."
            )
        );

        let mut ui = iced_test::simulator(state.view());
        assert!(ui.find(RESUME_FAILED).is_ok());
        assert!(ui.find("Sign out").is_ok());
        assert!(ui.find("Retry").is_ok());
        assert!(ui.find("Username").is_err());
    }

    #[test]
    fn a_connection_error_is_cut_down_to_its_cause() {
        assert_eq!(
            last_cause(
                "error sending request for url (http://nas:9/api/v1/auth/refresh): client error \
                 (Connect): tcp connect error: Connection refused (os error 61)"
            ),
            "connection refused"
        );
        assert_eq!(last_cause(""), "");
    }

    #[test]
    fn retrying_resumes_again() {
        let mut state = failed_to_resume();

        let _ = state.update(Message::RetryResume);

        assert!(state.restoring && state.busy);
        assert_eq!(state.resume_failed, None);
    }

    #[test]
    fn signing_out_of_a_failed_session_shows_the_form_for_its_server() {
        let mut state = failed_to_resume();
        let probe = state.probe;

        let _ = state.update(Message::ForgetSession);

        assert_eq!(state.resume_failed, None);
        assert_eq!(state.server, SAVED_SERVER);
        assert_eq!(state.probe, probe + 1);
    }

    #[test]
    fn an_unknown_answer_names_the_server_too() {
        let error = ApiError::Decode("not json".to_owned());
        assert_eq!(
            resume_reason(SAVED_SERVER, &error),
            "nas.local:8080: unexpected response: not json"
        );
    }

    #[test]
    fn the_password_can_be_shown() {
        let mut state = State::default();
        assert!(!state.show_password);

        let _ = state.update(Message::TogglePassword);

        assert!(state.show_password);
    }

    #[test]
    fn the_form_offers_single_sign_on_above_the_password() {
        let mut state = State::default();
        let _ = state.update(Message::ProvidersLoaded(state.probe, Ok(providers(true))));

        let mut ui = iced_test::simulator(state.view());

        let keycloak = ui.find("Sign in with Keycloak").unwrap().bounds().y;
        let username = ui.find("Username").unwrap().bounds().y;
        assert!(keycloak < username);
    }

    #[test]
    fn a_failed_browser_sign_in_shows_the_error_and_frees_the_form() {
        let mut state = State::default();
        let _ = state.update(Message::SignInWithProvider);
        assert!(state.busy);

        let _ = state.update(Message::Finished(Err("denied".to_owned())));

        assert_eq!(
            (state.busy, state.sso.is_none(), state.error.as_deref()),
            (false, true, Some("denied"))
        );
    }

    #[test]
    fn cancelling_a_browser_sign_in_frees_the_form() {
        let mut state = State::default();
        let _ = state.update(Message::SignInWithProvider);

        let _ = state.update(Message::Cancel);

        assert_eq!((state.busy, state.sso.is_none()), (false, true));
    }
}
