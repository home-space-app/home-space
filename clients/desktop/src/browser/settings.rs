//! The settings page. For now its Account tab: the profile, changing the password, and two-factor
//! sign-in with the authenticator app, the recovery codes, the sign-in keys, and the encryption
//! keys that open the protected folders. Adding a passkey or a sign-in key happens in the web
//! app, which can ask the system for one; everything else happens here.

use std::path::PathBuf;

use iced::{
    Alignment, Color, Element, Length, Task,
    widget::{
        Column, column, container, operation, qr_code, row, scrollable, space, text, text_input,
    },
};
use uuid::Uuid;

use api_client::{ApiError, Client, Confirmation};
use client_core::{
    accounts::password_problem,
    protected::{folder_keys, recovery_unlocker},
};
use protocol::{
    AuthMethod, Me, RecoveryCodes, TotpEnrolment, Vault, WebAuthnKey, WebAuthnKind, features,
};

use super::{parts, protect};
use crate::{
    icons::{ICON_SIZE, Icon, Tint},
    locale::short_date,
    protected::security_key,
    theme,
};

const GAP: f32 = 12.0;
const CARD_GAP: f32 = 16.0;
const CARD_PADDING: f32 = 20.0;
const CARD_TITLE: f32 = 16.0;
const SECTION_GAP: f32 = 6.0;
const AVATAR: f32 = 48.0;
const AVATAR_TEXT: f32 = 20.0;
const BADGE_PADDING: [f32; 2] = [2.0, 8.0];
const TAB_LINE: f32 = 2.0;
const QR_CELL: f32 = 4.0;
const ROW_PADDING: [f32; 2] = [10.0, 12.0];
/// Few enough recovery codes left to suggest making new ones.
const FEW_RECOVERY_CODES: u32 = 3;
const CODES_FILE: &str = "home-space-recovery-codes.txt";
const CONFIRM_INPUT: &str = "settings-confirm";
const APP_CODE_INPUT: &str = "settings-app-code";
const RENAME_INPUT: &str = "settings-rename";

/// A field of the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    CurrentPassword,
    NewPassword,
    RepeatPassword,
    /// The code from the authenticator app being set up.
    AppCode,
    /// The password or code that confirms it is the user.
    Confirmation,
    /// The new name of a sign-in key.
    KeyName,
}

/// What the page asks the server that may need the user to confirm it is them first.
#[derive(Debug, Clone)]
pub enum Action {
    StartApp,
    ConfirmApp(String),
    RemoveApp,
    NewCodes,
    RenameKey(Uuid, String),
    RemoveKey(Uuid),
}

#[cfg(test)]
impl State {
    /// Takes in `message` with a client that reaches nothing, for what needs no server.
    fn update_offline(&mut self, message: Message) -> Outcome {
        self.update(&Client::new("http://localhost"), message)
    }

    fn ran_offline(&mut self, action: Action, result: Result<Reply, ApiError>) -> Outcome {
        self.ran(&Client::new("http://localhost"), action, result)
    }
}

/// What the server answered an [Action].
#[derive(Debug, Clone)]
pub enum Reply {
    Enrolment(TotpEnrolment),
    Codes(RecoveryCodes),
    Done,
}

#[derive(Debug, Clone)]
pub enum Message {
    Input(Field, String),
    ChangePassword,
    PasswordChanged(Result<(), ApiError>),
    Run(Action),
    Ran(Action, Result<Reply, ApiError>),
    Confirm,
    Confirmed(Result<(), ApiError>),
    /// Leaves the question asked, the confirmation, a rename or a removal.
    Cancel,
    /// Asks before removing the authenticator app, or stops asking.
    AskRemoveApp(bool),
    CancelApp,
    CopyCodes,
    SaveCodes,
    CodesSaved(Result<Option<PathBuf>, String>),
    CodesKept,
    KeysLoaded(Result<Vec<WebAuthnKey>, ApiError>),
    VaultLoaded(Result<Option<Vault>, ApiError>),
    StartRename(Uuid),
    /// Asks before removing a sign-in key.
    AskRemoveKey(Uuid),
    /// Opens the settings in the web app, to add a passkey or a sign-in key.
    OpenWeb,
    AddEncryptionKey,
    RemoveEncryptionKey(Uuid),
    RecoveryKey,
}

/// What the page asks of the browser.
pub enum Outcome {
    Task(Task<Message>),
    /// The account changed, such as its second factors: the browser reads it again.
    MeChanged(Task<Message>),
    /// Opens a dialog about the encryption keys.
    KeyDialog(Box<protect::Dialog>, Task<protect::Message>),
    /// Opens the settings in the web app.
    OpenWeb,
}

/// Where the authenticator app part is.
enum App {
    Overview,
    /// A new app's secret, waiting for its first code.
    Adding {
        enrolment: TotpEnrolment,
        qr: Option<qr_code::Data>,
        code: String,
    },
    /// Recovery codes to write down, shown once.
    Codes {
        codes: Vec<String>,
        saved_to: Option<PathBuf>,
    },
}

/// The question the server asked before an action: the user confirms it is them.
struct Confirming {
    action: Action,
    message: String,
    methods: Vec<AuthMethod>,
    secret: String,
    busy: bool,
    error: Option<String>,
}

/// A sign-in key being renamed.
struct Renaming {
    id: Uuid,
    name: String,
}

/// The password form.
#[derive(Default)]
struct Password {
    current: String,
    new: String,
    repeat: String,
    busy: bool,
    error: Option<String>,
    changed: bool,
}

/// The settings page, on its Account tab.
pub struct State {
    /// The server's host, against which the sign-in keys made for another address are told.
    host: String,
    two_factor: bool,
    passkeys: bool,
    encryption: bool,
    password: Password,
    app: App,
    asking_app_removal: bool,
    busy: bool,
    error: Option<String>,
    confirming: Option<Confirming>,
    keys: Vec<WebAuthnKey>,
    renaming: Option<Renaming>,
    removing: Option<Uuid>,
    /// The encryption keys' vault: \`None\` while it loads, \`Some(None)\` before the first key.
    vault: Option<Option<Vault>>,
}

impl State {
    /// Opens the page and reads what it shows.
    pub fn open(client: &Client) -> (Self, Task<Message>) {
        let host = url::Url::parse(client.base_url())
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        let state = Self {
            host,
            two_factor: client.supports(features::TWO_FACTOR),
            passkeys: client.supports(features::PASSKEYS),
            encryption: client.supports(features::PROTECTED_FOLDERS),
            password: Password::default(),
            app: App::Overview,
            asking_app_removal: false,
            busy: false,
            error: None,
            confirming: None,
            keys: Vec::new(),
            renaming: None,
            removing: None,
            vault: None,
        };
        let mut tasks = Vec::new();
        if state.passkeys {
            tasks.push(load_keys(client));
        }
        if state.encryption {
            tasks.push(state.load_vault(client));
        }
        (state, Task::batch(tasks))
    }

    fn load_vault(&self, client: &Client) -> Task<Message> {
        let client = client.clone();
        Task::perform(async move { client.vault().await }, Message::VaultLoaded)
    }

    /// Reads the encryption keys again, after a dialog may have changed them.
    pub fn reload_vault(&mut self, client: &Client) -> Task<Message> {
        if self.encryption {
            self.load_vault(client)
        } else {
            Task::none()
        }
    }

    /// Whether something waits for an answer, which Escape leaves first.
    pub fn is_asking(&self) -> bool {
        self.confirming.is_some()
            || self.renaming.is_some()
            || self.removing.is_some()
            || self.asking_app_removal
    }

    /// The dialog confirming it is the user, while it is open.
    pub fn dialog(&self) -> Option<Element<'_, Message>> {
        self.confirming.as_ref().map(confirm_dialog)
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        let task = match message {
            Message::Input(field, value) => {
                self.input(field, value);
                Task::none()
            }
            Message::ChangePassword => self.change_password(client),
            Message::PasswordChanged(result) => {
                self.password.busy = false;
                match result {
                    Ok(()) => {
                        self.password = Password {
                            changed: true,
                            ..Password::default()
                        };
                    }
                    Err(err) => self.password.error = Some(sentence(&err.to_string())),
                }
                Task::none()
            }
            Message::Run(action) => {
                if self.busy {
                    return Outcome::Task(Task::none());
                }
                self.busy = true;
                self.error = None;
                self.asking_app_removal = false;
                self.removing = None;
                if matches!(action, Action::RenameKey(..)) {
                    self.renaming = None;
                }
                run(client, action)
            }
            Message::Ran(action, result) => return self.ran(client, action, result),
            Message::Confirm => self.confirm(client),
            Message::Confirmed(Ok(())) => {
                let Some(confirming) = self.confirming.take() else {
                    return Outcome::Task(Task::none());
                };
                self.busy = true;
                run(client, confirming.action)
            }
            Message::Confirmed(Err(err)) => {
                if let Some(confirming) = &mut self.confirming {
                    confirming.busy = false;
                    confirming.error = Some(sentence(&err.to_string()));
                }
                Task::none()
            }
            Message::Cancel => {
                self.confirming = None;
                self.renaming = None;
                self.removing = None;
                self.asking_app_removal = false;
                Task::none()
            }
            Message::AskRemoveApp(asking) => {
                self.asking_app_removal = asking;
                Task::none()
            }
            Message::CancelApp => {
                self.app = App::Overview;
                Task::none()
            }
            Message::CopyCodes => match &self.app {
                App::Codes { codes, .. } => iced::clipboard::write(codes.join("\n")),
                _ => Task::none(),
            },
            Message::SaveCodes => match &self.app {
                App::Codes { codes, .. } => save_codes(codes.join("\n")),
                _ => Task::none(),
            },
            Message::CodesSaved(Ok(path)) => {
                if let App::Codes { saved_to, .. } = &mut self.app {
                    *saved_to = path;
                }
                Task::none()
            }
            Message::CodesSaved(Err(err)) => {
                self.error = Some(err);
                Task::none()
            }
            Message::CodesKept => {
                self.app = App::Overview;
                Task::none()
            }
            Message::KeysLoaded(Ok(keys)) => {
                self.keys = keys;
                Task::none()
            }
            Message::KeysLoaded(Err(err)) | Message::VaultLoaded(Err(err)) => {
                self.error = Some(err.to_string());
                Task::none()
            }
            Message::VaultLoaded(Ok(vault)) => {
                self.vault = Some(vault);
                Task::none()
            }
            Message::StartRename(id) => {
                let Some(key) = self.keys.iter().find(|key| key.id == id) else {
                    return Outcome::Task(Task::none());
                };
                self.removing = None;
                self.renaming = Some(Renaming {
                    id,
                    name: key.name.clone(),
                });
                Task::batch([
                    operation::focus(RENAME_INPUT),
                    operation::select_all(RENAME_INPUT),
                ])
            }
            Message::AskRemoveKey(id) => {
                self.renaming = None;
                self.removing = Some(id);
                Task::none()
            }
            Message::OpenWeb => return Outcome::OpenWeb,
            Message::AddEncryptionKey => {
                let Some(vault) = self.vault.clone() else {
                    return Outcome::Task(Task::none());
                };
                let (dialog, task) = protect::Dialog::add_key(client, vault);
                return Outcome::KeyDialog(Box::new(dialog), task);
            }
            Message::RemoveEncryptionKey(id) => {
                let Some(Some(vault)) = self.vault.clone() else {
                    return Outcome::Task(Task::none());
                };
                return match protect::Dialog::remove_key(vault, id) {
                    Some(dialog) => Outcome::KeyDialog(Box::new(dialog), Task::none()),
                    None => Outcome::Task(Task::none()),
                };
            }
            Message::RecoveryKey => {
                let Some(Some(vault)) = self.vault.clone() else {
                    return Outcome::Task(Task::none());
                };
                let (dialog, task) = protect::Dialog::create_recovery(vault);
                return Outcome::KeyDialog(Box::new(dialog), task);
            }
        };
        Outcome::Task(task)
    }

    fn input(&mut self, field: Field, value: String) {
        match field {
            Field::CurrentPassword => self.password.current = value,
            Field::NewPassword => self.password.new = value,
            Field::RepeatPassword => self.password.repeat = value,
            Field::AppCode => {
                if let App::Adding { code, .. } = &mut self.app {
                    *code = value;
                }
            }
            Field::Confirmation => {
                if let Some(confirming) = &mut self.confirming {
                    confirming.secret = value;
                }
            }
            Field::KeyName => {
                if let Some(renaming) = &mut self.renaming {
                    renaming.name = value;
                }
            }
        }
        if matches!(
            field,
            Field::CurrentPassword | Field::NewPassword | Field::RepeatPassword
        ) {
            self.password.error = None;
            self.password.changed = false;
        }
    }

    fn change_password(&mut self, client: &Client) -> Task<Message> {
        let password = &mut self.password;
        if password.busy {
            return Task::none();
        }
        let problem = if password.current.is_empty() {
            Some("Enter your current password.")
        } else {
            password_problem(&password.new, &password.repeat)
        };
        if let Some(problem) = problem {
            password.error = Some(problem.to_owned());
            return Task::none();
        }
        password.busy = true;
        password.error = None;
        let (client, current, new) = (
            client.clone(),
            password.current.clone(),
            password.new.clone(),
        );
        Task::perform(
            async move { client.change_password(&current, &new).await },
            Message::PasswordChanged,
        )
    }

    /// Takes in what the server answered an action.
    fn ran(&mut self, client: &Client, action: Action, result: Result<Reply, ApiError>) -> Outcome {
        self.busy = false;
        let reply = match result {
            Ok(reply) => reply,
            Err(ApiError::ReauthenticationRequired { message, methods }) => {
                self.confirming = Some(Confirming {
                    action,
                    message,
                    methods,
                    secret: String::new(),
                    busy: false,
                    error: None,
                });
                return Outcome::Task(operation::focus(CONFIRM_INPUT));
            }
            Err(err) => {
                self.error = Some(sentence(&err.to_string()));
                return Outcome::Task(Task::none());
            }
        };
        match reply {
            Reply::Enrolment(enrolment) => {
                let qr = qr_code::Data::new(enrolment.uri.as_bytes()).ok();
                self.app = App::Adding {
                    enrolment,
                    qr,
                    code: String::new(),
                };
                Outcome::Task(operation::focus(APP_CODE_INPUT))
            }
            // Confirming an app on an account that has a key already brings no new codes.
            Reply::Codes(RecoveryCodes { codes }) => {
                self.app = if codes.is_empty() {
                    App::Overview
                } else {
                    App::Codes {
                        codes,
                        saved_to: None,
                    }
                };
                Outcome::MeChanged(Task::none())
            }
            Reply::Done => match action {
                Action::RenameKey(..) => Outcome::Task(load_keys(client)),
                Action::RemoveKey(_) => Outcome::MeChanged(load_keys(client)),
                _ => Outcome::MeChanged(Task::none()),
            },
        }
    }

    /// Confirms it is the user with what they typed, then runs the action again.
    fn confirm(&mut self, client: &Client) -> Task<Message> {
        let Some(confirming) = &mut self.confirming else {
            return Task::none();
        };
        let secret = confirming.secret.trim().to_owned();
        let by_password = confirming.methods.contains(&AuthMethod::Password);
        let by_code = confirming.methods.contains(&AuthMethod::Totp);
        if confirming.busy || secret.is_empty() || !(by_password || by_code) {
            return Task::none();
        }
        confirming.busy = true;
        confirming.error = None;
        let client = client.clone();
        Task::perform(
            async move {
                let proof = if by_password {
                    Confirmation::Password(&secret)
                } else {
                    Confirmation::Code(&secret)
                };
                client.reauthenticate(proof).await
            },
            Message::Confirmed,
        )
    }

    pub fn view<'a>(&'a self, me: &'a Me) -> Element<'a, Message> {
        let mut cards = column![profile_card(me)].spacing(CARD_GAP);
        if !me.external {
            cards = cards.push(self.password_card());
        }
        cards = cards.push(self.security_card(me));
        column![tabs(), scrollable(cards).height(Length::Fill)]
            .spacing(CARD_GAP)
            .into()
    }

    fn password_card(&self) -> Element<'_, Message> {
        let password = &self.password;
        let secret = |label: &'static str, value: &'static str, field: Field, typed: &'_ str| {
            let _ = value;
            let input = text_input("", typed)
                .secure(true)
                .style(theme::input)
                .padding(parts::FIELD_PADDING)
                .on_input(move |value| Message::Input(field, value))
                .on_submit(Message::ChangePassword);
            container(parts::field(label, input, None)).width(Length::Fill)
        };
        let fields = row![
            secret(
                "Current password",
                "",
                Field::CurrentPassword,
                &password.current
            ),
            secret("New password", "", Field::NewPassword, &password.new),
            secret(
                "Repeat new password",
                "",
                Field::RepeatPassword,
                &password.repeat
            ),
        ]
        .spacing(GAP);
        let mut body = column![fields].spacing(GAP);
        if let Some(error) = &password.error {
            body = body.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        if password.changed {
            body = body.push(parts::alert_info(
                Icon::Check,
                "Your password was changed.",
                None,
            ));
        }
        let label = if password.busy {
            "Changing…"
        } else {
            "Change password"
        };
        body = body.push(parts::primary(
            None,
            label,
            (!password.busy).then_some(Message::ChangePassword),
        ));
        card("Change password", body)
    }

    fn security_card<'a>(&'a self, me: &'a Me) -> Element<'a, Message> {
        let mut body = column![].spacing(CARD_GAP);
        if let Some(error) = &self.error {
            body = body.push(parts::alert_error(Icon::Error, error.as_str(), None));
        }
        match &self.app {
            App::Adding {
                enrolment,
                qr,
                code,
            } => {
                return card(
                    TWO_FACTOR,
                    body.push(self.add_app(enrolment, qr.as_ref(), code)),
                );
            }
            App::Codes { codes, saved_to } => {
                return card(
                    TWO_FACTOR,
                    body.push(show_codes(codes, saved_to.as_deref())),
                );
            }
            App::Overview => {}
        }
        if me.external {
            body = body.push(para(
                "Signed in through your identity provider. Your sign-in, two-factor and passkeys \
                 are managed there.",
            ));
        } else if self.two_factor {
            let factors = &me.second_factors;
            body = body
                .push(para(if factors.enabled() {
                    "On. Signing in with your password also takes a passkey, a security key or a \
                     code from your authenticator app."
                } else {
                    "Off. Add an authenticator app or a passkey, so that a stolen password alone \
                     cannot open your account."
                }))
                .push(self.app_section(factors.totp));
            if self.passkeys {
                body = body.push(self.sign_in_keys());
            }
        }
        if self.encryption {
            body = body.push(self.encryption_keys());
        }
        if !me.external && self.two_factor && me.second_factors.enabled() {
            body = body.push(self.recovery_codes(me.second_factors.recovery_codes_left));
        }
        card(TWO_FACTOR, body)
    }

    fn app_section(&self, on: bool) -> Element<'_, Message> {
        let busy = self.busy;
        let (line, actions): (&str, Element<'_, Message>) = if !on {
            (
                "Codes from an app such as Google Authenticator, Aegis or 1Password.",
                parts::outlined(
                    None,
                    "Set up an authenticator app",
                    (!busy).then_some(Message::Run(Action::StartApp)),
                ),
            )
        } else if self.asking_app_removal {
            (
                "Set up. Signing in can take a code from it.",
                row![
                    text("Remove the authenticator app?"),
                    parts::small_outlined("Keep it", Some(Message::AskRemoveApp(false))),
                    parts::danger("Remove", (!busy).then_some(Message::Run(Action::RemoveApp))),
                ]
                .spacing(GAP)
                .align_y(Alignment::Center)
                .into(),
            )
        } else {
            (
                "Set up. Signing in can take a code from it.",
                parts::danger("Remove", Some(Message::AskRemoveApp(true))),
            )
        };
        section("Authenticator app", column![para(line), actions])
    }

    fn add_app<'a>(
        &'a self,
        enrolment: &'a TotpEnrolment,
        qr: Option<&'a qr_code::Data>,
        code: &'a str,
    ) -> Element<'a, Message> {
        let qr = qr.map(|data| {
            container(
                qr_code(data)
                    .cell_size(QR_CELL)
                    .style(|_theme| qr_code::Style {
                        cell: Color::BLACK,
                        background: Color::WHITE,
                    }),
            )
            .padding(GAP)
            .style(|_theme| container::Style {
                background: Some(Color::WHITE.into()),
                ..container::Style::default()
            })
        });
        let input = text_input("123456", code)
            .id(APP_CODE_INPUT)
            .style(theme::input)
            .padding(parts::FIELD_PADDING)
            .width(Length::Fixed(200.0))
            .on_input(|value| Message::Input(Field::AppCode, value))
            .on_submit(Message::Run(Action::ConfirmApp(code.trim().to_owned())));
        let turn_on = (!self.busy && !code.trim().is_empty())
            .then(|| Message::Run(Action::ConfirmApp(code.trim().to_owned())));
        section(
            "Authenticator app",
            column![
                para("Scan this code with your authenticator app."),
                qr,
                row![
                    para("Can't scan it? Enter this key instead:"),
                    text(grouped(&enrolment.secret)).font(theme::MONO),
                ]
                .spacing(SECTION_GAP),
                parts::field("Code from the app", input, None),
                row![
                    parts::outlined(None, "Cancel", Some(Message::CancelApp)),
                    parts::primary(None, "Turn on", turn_on),
                ]
                .spacing(GAP),
            ],
        )
    }

    fn sign_in_keys(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(SECTION_GAP);
        for key in &self.keys {
            list = list.push(self.sign_in_key(key));
        }
        section(
            "Sign-in keys",
            column![
                para(
                    "A passkey signs you in on its own, with your fingerprint, face or PIN. A \
                     security key, such as a YubiKey, takes the place of the app's code after \
                     your password. They are added in the web app, which can ask your system \
                     for one."
                ),
                list,
                parts::outlined(
                    Some(Icon::Web),
                    "Add a passkey or security key in the web app",
                    Some(Message::OpenWeb),
                ),
            ],
        )
    }

    fn sign_in_key<'a>(&'a self, key: &'a WebAuthnKey) -> Element<'a, Message> {
        let busy = self.busy;
        let (name, actions): (Element<'a, Message>, Element<'a, Message>) = match &self.renaming {
            Some(renaming) if renaming.id == key.id => {
                let save = Message::Run(Action::RenameKey(key.id, renaming.name.trim().to_owned()));
                (
                    text_input("Name", &renaming.name)
                        .id(RENAME_INPUT)
                        .style(theme::input)
                        .padding(parts::FIELD_PADDING)
                        .on_input(|value| Message::Input(Field::KeyName, value))
                        .on_submit(save.clone())
                        .into(),
                    row![
                        parts::small_outlined("Cancel", Some(Message::Cancel)),
                        parts::small_outlined("Save", (!busy).then_some(save)),
                    ]
                    .spacing(GAP)
                    .into(),
                )
            }
            _ if self.removing == Some(key.id) => (
                text(key.name.as_str()).font(theme::MEDIUM).into(),
                row![
                    text("Remove it?"),
                    parts::small_outlined("Keep it", Some(Message::Cancel)),
                    parts::danger(
                        "Remove",
                        (!busy).then_some(Message::Run(Action::RemoveKey(key.id)))
                    ),
                ]
                .spacing(GAP)
                .align_y(Alignment::Center)
                .into(),
            ),
            _ => (
                text(key.name.as_str()).font(theme::MEDIUM).into(),
                row![
                    parts::small_outlined("Rename", Some(Message::StartRename(key.id))),
                    parts::small_outlined("Remove", Some(Message::AskRemoveKey(key.id))),
                ]
                .spacing(GAP)
                .into(),
            ),
        };
        let elsewhere = (!self.host.is_empty() && !self.host.eq_ignore_ascii_case(&key.rp_id))
            .then(|| {
                text(format!("Works only on {}", key.rp_id))
                    .size(parts::SECOND_LINE_SIZE)
                    .style(theme::danger_text)
            });
        key_row(
            column![name, small(key_details(key))]
                .push(elsewhere)
                .spacing(2.0),
            actions,
        )
    }

    fn encryption_keys(&self) -> Element<'_, Message> {
        let intro = para(
            "Security keys that encrypt your protected folders and open them again, each with \
             its PIN. They never sign you in, even when they are on the same key as a sign-in key.",
        );
        let Some(vault) = &self.vault else {
            return section(
                "Encryption keys",
                column![intro, small("Loading…".to_owned())],
            );
        };
        let keys = vault.as_ref().map(folder_keys).unwrap_or_default();
        let mut list = column![].spacing(SECTION_GAP);
        if keys.is_empty() {
            list = list.push(para("No encryption keys yet."));
        }
        for key in &keys {
            let added = vault
                .as_ref()
                .and_then(|vault| {
                    vault
                        .unlockers
                        .iter()
                        .find(|unlocker| unlocker.id == key.id)
                })
                .map(|unlocker| {
                    format!("Encryption key · added {}", short_date(unlocker.created_at))
                })
                .unwrap_or_default();
            let remove = security_key::SUPPORTED.then(|| {
                parts::small_outlined("Remove", Some(Message::RemoveEncryptionKey(key.id)))
            });
            list = list.push(key_row(
                column![text(key.name.clone()).font(theme::MEDIUM), small(added)].spacing(2.0),
                row![].push(remove).into(),
            ));
        }
        let mut body = column![intro, list].spacing(GAP);
        if !security_key::SUPPORTED {
            return section(
                "Encryption keys",
                body.push(para(
                    "Encryption keys can be added and removed in this app on Linux and macOS, for now.",
                )),
            );
        }
        if let Some(vault) = vault {
            let (line, action) = match recovery_unlocker(vault) {
                Some(unlocker) => (
                    format!("Recovery key created {}.", short_date(unlocker.created_at)),
                    "Replace the recovery key…",
                ),
                None => (
                    "No recovery key: if you lose your encryption keys, the folders they protect \
                     are lost."
                        .to_owned(),
                    "Create a recovery key…",
                ),
            };
            body = body.push(parts::alert_info(
                Icon::Info,
                line,
                Some(parts::small_outlined(action, Some(Message::RecoveryKey))),
            ));
        }
        body = body.push(parts::outlined(
            Some(Icon::Key),
            "Add an encryption key…",
            Some(Message::AddEncryptionKey),
        ));
        section("Encryption keys", body)
    }

    fn recovery_codes(&self, left: u32) -> Element<'_, Message> {
        let line = text(recovery_left(left)).style(if left <= FEW_RECOVERY_CODES {
            theme::danger_text
        } else {
            theme::muted_text
        });
        section(
            "Recovery codes",
            column![
                line,
                parts::outlined(
                    None,
                    "New recovery codes",
                    (!self.busy).then_some(Message::Run(Action::NewCodes)),
                ),
            ],
        )
    }
}

const TWO_FACTOR: &str = "Two-factor sign-in and passkeys";

/// The tabs of the page; only Account for now.
fn tabs<'a>() -> Element<'a, Message> {
    let account = column![
        row![
            Icon::Person.view(ICON_SIZE, Tint::Accent),
            text("Account").style(theme::accent_text),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center),
        container(space().height(TAB_LINE))
            .width(Length::Fill)
            .style(theme::accent),
    ]
    .spacing(8.0)
    .width(Length::Shrink);
    column![account, parts::divider()].into()
}

fn profile_card(me: &Me) -> Element<'_, Message> {
    let initial: String = me
        .username
        .chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default();
    let avatar = container(text(initial).size(AVATAR_TEXT).font(theme::BOLD))
        .center(AVATAR)
        .style(theme::avatar);
    let role = if me.is_admin {
        "Administrator"
    } else {
        "Member"
    };
    let badge = container(
        text(role)
            .size(parts::SECOND_LINE_SIZE)
            .style(theme::accent_text),
    )
    .padding(BADGE_PADDING)
    .style(theme::badge);
    let method = if me.external {
        "Single sign-on"
    } else {
        "Password"
    };
    let shown = |value: &str| {
        container(text(value.to_owned()))
            .padding(parts::FIELD_PADDING)
            .width(Length::Fill)
            .style(theme::field)
    };
    card(
        "Profile",
        column![
            row![
                avatar,
                column![
                    text(me.username.as_str())
                        .size(CARD_TITLE)
                        .font(theme::SEMIBOLD),
                    badge
                ]
                .spacing(4.0)
            ]
            .spacing(GAP)
            .align_y(Alignment::Center),
            parts::field("Username", shown(&me.username), None),
            parts::field("Sign-in method", shown(method), None),
        ],
    )
}

/// The dialog asking the user to confirm it is them before the action they started.
fn confirm_dialog(confirming: &Confirming) -> Element<'_, Message> {
    let by_password = confirming.methods.contains(&AuthMethod::Password);
    let by_code = confirming.methods.contains(&AuthMethod::Totp);
    let mut lines = column![parts::dialog_head(
        "Confirm it is you",
        confirming.message.as_str()
    ),];
    let mut buttons = vec![parts::outlined(None, "Cancel", Some(Message::Cancel))];
    if by_password || by_code {
        let label = if by_password {
            "Password"
        } else {
            "Code from your authenticator app"
        };
        let input = text_input("", &confirming.secret)
            .id(CONFIRM_INPUT)
            .secure(by_password)
            .style(theme::input)
            .padding(parts::FIELD_PADDING)
            .on_input(|value| Message::Input(Field::Confirmation, value))
            .on_submit(Message::Confirm);
        lines = lines.push(parts::field(label, input, None));
        let label = if confirming.busy {
            "Confirming…"
        } else {
            "Confirm"
        };
        buttons.push(parts::primary(
            None,
            label,
            (!confirming.busy).then_some(Message::Confirm),
        ));
    } else {
        lines = lines.push(para(
            "This account confirms it is you with a passkey or the identity provider, which the \
             desktop app cannot use yet. Do this in the web app instead.",
        ));
        buttons.push(parts::primary(
            Some(Icon::Web),
            "Open in web",
            Some(Message::OpenWeb),
        ));
    }
    if let Some(error) = &confirming.error {
        lines = lines.push(parts::alert_error(Icon::Error, error.as_str(), None));
    }
    parts::dialog(parts::DIALOG_WIDTH, lines, buttons)
}

/// Recovery codes, shown once, with ways to keep them.
fn show_codes<'a>(
    codes: &'a [String],
    saved_to: Option<&'a std::path::Path>,
) -> Element<'a, Message> {
    let list = codes.iter().fold(column![].spacing(4.0), |list, code| {
        list.push(text(code.as_str()).font(theme::MONO))
    });
    let saved = saved_to.map(|path| small(format!("Saved to {}", path.display())));
    section(
        "Recovery codes",
        column![
            parts::alert_info(
                Icon::Info,
                "Save these recovery codes somewhere safe. Each one signs you in once if you lose \
                 your passkeys, security keys and authenticator app. They are shown only now.",
                None,
            ),
            list,
            saved,
            row![
                parts::outlined(Some(Icon::Copy), "Copy", Some(Message::CopyCodes)),
                parts::outlined(Some(Icon::Download), "Save…", Some(Message::SaveCodes)),
                parts::primary(None, "I saved them", Some(Message::CodesKept)),
            ]
            .spacing(GAP),
        ],
    )
}

fn card<'a>(title: &'a str, body: Column<'a, Message>) -> Element<'a, Message> {
    container(
        column![
            text(title).size(CARD_TITLE).font(theme::SEMIBOLD),
            body.spacing(GAP)
        ]
        .spacing(GAP),
    )
    .padding(CARD_PADDING)
    .width(Length::Fill)
    .style(theme::card)
    .into()
}

/// A part of a card under its own heading.
fn section<'a>(heading: &'a str, body: Column<'a, Message>) -> Element<'a, Message> {
    column![text(heading).font(theme::MEDIUM), body.spacing(GAP)]
        .spacing(SECTION_GAP)
        .into()
}

/// A key in a list: what it is, then what can be done with it.
fn key_row<'a>(about: Column<'a, Message>, actions: Element<'a, Message>) -> Element<'a, Message> {
    container(
        row![about.width(Length::Fill), actions]
            .spacing(GAP)
            .align_y(Alignment::Center),
    )
    .padding(ROW_PADDING)
    .width(Length::Fill)
    .style(theme::field)
    .into()
}

fn para<'a>(line: impl text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(line).style(theme::muted_text)
}

fn small<'a>(line: String) -> iced::widget::Text<'a> {
    text(line)
        .size(parts::SECOND_LINE_SIZE)
        .style(theme::muted_text)
}

fn load_keys(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { client.webauthn_keys().await },
        Message::KeysLoaded,
    )
}

/// Asks the server for \`action\`.
fn run(client: &Client, action: Action) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move {
            let reply = match &action {
                Action::StartApp => client.start_totp().await.map(Reply::Enrolment),
                Action::ConfirmApp(code) => client.confirm_totp(code).await.map(Reply::Codes),
                Action::RemoveApp => client.remove_totp().await.map(|()| Reply::Done),
                Action::NewCodes => client.new_recovery_codes().await.map(Reply::Codes),
                Action::RenameKey(id, name) => {
                    client.rename_webauthn(*id, name).await.map(|_| Reply::Done)
                }
                Action::RemoveKey(id) => client.remove_webauthn(*id).await.map(|()| Reply::Done),
            };
            (action, reply)
        },
        |(action, reply)| Message::Ran(action, reply),
    )
}

/// Asks where to save the recovery codes, over the app's window, and writes them there.
fn save_codes(codes: String) -> Task<Message> {
    crate::file_dialog::over_window(|dialog| dialog.set_file_name(CODES_FILE)).then(move |dialog| {
        let codes = codes.clone();
        Task::perform(
            async move {
                let Some(handle) = dialog.save_file().await else {
                    return Ok(None);
                };
                let path = handle.path().to_path_buf();
                tokio::fs::write(&path, format!("Home Space recovery codes\n\n{codes}\n"))
                    .await
                    .map_err(|err| format!("{}: {err}", path.display()))?;
                Ok(Some(path))
            },
            Message::CodesSaved,
        )
    })
}

/// What kind of key it is, and when it was added and last used.
fn key_details(key: &WebAuthnKey) -> String {
    let kind = match key.kind {
        WebAuthnKind::Passkey => "Passkey",
        WebAuthnKind::SecurityKey => "Security key",
    };
    let used = key.last_used_at.map_or_else(
        || "never used".to_owned(),
        |at| format!("last used {}", short_date(at)),
    );
    format!("{kind} · added {} · {used}", short_date(key.created_at))
}

/// How many recovery codes are left, as a sentence.
fn recovery_left(left: u32) -> String {
    match left {
        0 => "No recovery codes left. Make new ones so you can still sign in if you lose your \
              devices."
            .to_owned(),
        1 => "1 recovery code left.".to_owned(),
        _ => format!("{left} recovery codes left."),
    }
}

/// The key in groups of four, as apps show them, so it is easier to type.
fn grouped(secret: &str) -> String {
    secret
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A server message as a sentence: capitalised, with a full stop.
fn sentence(message: &str) -> String {
    let mut chars = message.trim().chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut sentence: String = first.to_uppercase().chain(chars).collect();
    if !sentence.ends_with(['.', '!', '?']) {
        sentence.push('.');
    }
    sentence
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use protocol::{NodeId, SecondFactors, UnlockerKind, UserId, VaultUnlocker};
    use time::OffsetDateTime;

    use super::*;

    fn me(external: bool, totp: bool) -> Me {
        Me {
            id: UserId(Uuid::nil()),
            username: "alice".to_owned(),
            lost_and_found_id: NodeId(Uuid::nil()),
            is_admin: true,
            external,
            second_factors: SecondFactors {
                totp,
                webauthn: 0,
                recovery_codes_left: 2,
            },
        }
    }

    fn page() -> State {
        State {
            host: "files.example.com".to_owned(),
            two_factor: true,
            passkeys: true,
            encryption: true,
            password: Password::default(),
            app: App::Overview,
            asking_app_removal: false,
            busy: false,
            error: None,
            confirming: None,
            keys: Vec::new(),
            renaming: None,
            removing: None,
            vault: None,
        }
    }

    fn vault(names: &[&str], recovery: bool) -> Vault {
        let unlocker = |kind, name: Option<&str>, n: u8| VaultUnlocker {
            id: Uuid::from_bytes([n; 16]),
            kind,
            name: name.map(str::to_owned),
            credential_id: name.map(|_| vec![n; 16].into()),
            rp_id: name.map(|_| "files.example.com".to_owned()),
            prf_salt: name.map(|_| vec![n; 32].into()),
            wrapped_vault_key: vec![0; protocol::WRAPPED_LEN].into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
        let mut unlockers: Vec<VaultUnlocker> = names
            .iter()
            .zip(1u8..)
            .map(|(name, n)| unlocker(UnlockerKind::SecurityKey, Some(name), n))
            .collect();
        if recovery {
            unlockers.push(unlocker(UnlockerKind::Recovery, None, 99));
        }
        Vault {
            epoch: 1,
            unlockers,
        }
    }

    fn sign_in_key(name: &str, rp_id: &str) -> WebAuthnKey {
        WebAuthnKey {
            id: Uuid::from_u128(7),
            name: name.to_owned(),
            kind: WebAuthnKind::SecurityKey,
            rp_id: rp_id.to_owned(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_used_at: None,
        }
    }

    #[test]
    fn the_account_tab_shows_the_profile_password_and_two_factor() {
        let state = page();
        let me = me(false, false);
        let mut ui = simulator(state.view(&me));
        assert!(ui.find("Account").is_ok());
        assert!(ui.find("Administrator").is_ok());
        assert!(ui.find("Current password").is_ok());
        assert!(ui.find("Set up an authenticator app").is_ok());
        assert!(ui.find("Sign-in keys").is_ok());
        assert!(ui.find("Encryption keys").is_ok());
        assert!(
            ui.find("Recovery codes").is_err(),
            "only with two-factor on"
        );
    }

    #[test]
    fn an_identity_provider_account_keeps_only_its_encryption_keys_here() {
        let state = page();
        let me = me(true, false);
        let mut ui = simulator(state.view(&me));
        assert!(ui.find("Single sign-on").is_ok());
        assert!(ui.find("Current password").is_err());
        assert!(ui.find("Set up an authenticator app").is_err());
        assert!(ui.find("Encryption keys").is_ok());
    }

    #[cfg(not(windows))]
    #[test]
    fn the_encryption_keys_are_listed_with_the_recovery_key_and_open_their_dialogs() {
        let mut state = page();
        state.vault = Some(Some(vault(&["YubiKey"], true)));
        let me = me(false, false);
        let mut ui = simulator(state.view(&me));
        assert!(ui.find("YubiKey").is_ok());
        assert!(ui.find("Replace the recovery key…").is_ok());
        assert!(ui.find("Add an encryption key…").is_ok());
        drop(ui);

        state.vault = Some(None);
        let mut ui = simulator(state.view(&me));
        assert!(ui.find("No encryption keys yet.").is_ok());
        assert!(
            ui.find("Create a recovery key…").is_err(),
            "nothing to recover yet"
        );
    }

    #[test]
    fn a_sign_in_key_is_renamed_or_removed_after_asking() {
        let mut state = page();
        state.keys = vec![
            sign_in_key("Phone", "files.example.com"),
            WebAuthnKey {
                id: Uuid::from_u128(8),
                ..sign_in_key("Old laptop", "old.example.com")
            },
        ];
        let me = me(false, true);
        let mut ui = simulator(state.view(&me));
        assert!(ui.find("Works only on old.example.com").is_ok());
        drop(ui);

        let _ = state.update_offline(Message::AskRemoveKey(Uuid::from_u128(7)));
        let mut ui = simulator(state.view(&me));
        assert!(ui.find("Remove it?").is_ok());
        assert!(ui.find("Keep it").is_ok());
        drop(ui);
        let _ = state.update_offline(Message::Cancel);
        assert!(state.removing.is_none());

        let _ = state.update_offline(Message::StartRename(Uuid::from_u128(7)));
        assert_eq!(
            state
                .renaming
                .as_ref()
                .map(|renaming| renaming.name.as_str()),
            Some("Phone")
        );
        assert!(simulator(state.view(&me)).find("Save").is_ok());
    }

    #[test]
    fn an_action_the_server_wants_confirmed_asks_for_the_password_and_runs_again() {
        let mut state = page();
        let _ = state.ran_offline(
            Action::RemoveApp,
            Err(ApiError::ReauthenticationRequired {
                message: "Confirm it is you to change two-factor sign-in.".to_owned(),
                methods: vec![AuthMethod::Password, AuthMethod::Totp],
            }),
        );
        let dialog = state.dialog().expect("the dialog is open");
        let mut ui = simulator(dialog);
        assert!(ui.find("Confirm it is you").is_ok());
        assert!(ui.find("Password").is_ok());
        drop(ui);
        assert!(state.is_asking());
        let _ = state.update_offline(Message::Cancel);
        assert!(state.dialog().is_none());
    }

    #[test]
    fn the_key_reads_in_groups_of_four() {
        assert_eq!(grouped("GEZDGNBVGY3TQOJQ"), "GEZD GNBV GY3T QOJQ");
        assert_eq!(grouped("ABCDEF"), "ABCD EF");
    }

    #[test]
    fn the_codes_left_read_as_a_sentence() {
        assert_eq!(recovery_left(1), "1 recovery code left.");
        assert_eq!(recovery_left(9), "9 recovery codes left.");
        assert!(recovery_left(0).starts_with("No recovery codes left"));
    }

    #[test]
    fn server_messages_read_as_sentences() {
        assert_eq!(
            sentence("the current password is wrong"),
            "The current password is wrong."
        );
        assert_eq!(sentence("Already done."), "Already done.");
    }
}
