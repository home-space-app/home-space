//! The dialogs of protected folders: protecting a folder with a password, unlocking it,
//! changing or recovering its password, unprotecting it, the recovery key shown once,
//! confirming it is the user before such a change, and deleting from the recycle bin what was
//! there before the folder was protected.
//!
//! Deriving a key from a password takes about a second, so it runs in the background.

use std::{path::PathBuf, sync::Arc};

use iced::{
    Element, Length, Task,
    widget::{Column, checkbox, column, operation, text, text_input},
};

use api_client::{ApiError, Client, Confirmation};
use client_core::{
    accounts::password_problem,
    protected::{
        PasswordProof, change_folder_password, protect_with_password, unlock_password_folder,
    },
};
use protocol::{
    AuthMethod, ChangeFolderPassword, FolderLock, Node, NodeId, ProtectFolder, ProtectedFolder,
    TrashItem,
};

use super::parts;
use crate::{icons::Icon, protected::Keys, theme};

const FIRST_INPUT: &str = "protect-first";
const WORKING: &str = "Working…";
const CANCEL: &str = "Cancel";
const KEY_FILE: &str = "home-space-folder-recovery-key.txt";
const SECURITY_KEY_LATER: &str = "Security keys come to the desktop app in a later version; until then this folder cannot be opened here.";
const NO_WAY_TO_CONFIRM: &str = "This account confirms it is you with a passkey or the identity provider, which the desktop app cannot use yet. Sign out and sign in again, then try once more.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Password,
    Repeat,
    NewPassword,
    NewRepeat,
    /// The account password or code that confirms it is the user.
    Confirmation,
}

#[derive(Debug, Clone)]
pub enum Message {
    Input(Field, String),
    /// Uses the folder's recovery key instead of a forgotten password.
    Forgot,
    Submit,
    Cancel,
    Derived(Result<Derived, String>),
    /// What the server answered the change.
    Sent(Result<Option<ProtectedFolder>, ApiError>),
    Confirmed(Result<(), ApiError>),
    CopyKey,
    SaveKey,
    KeySaved(Result<Option<PathBuf>, String>),
    Kept(bool),
    Purged(Result<(), ApiError>),
}

/// What a password opened or made, worked out in the background.
#[derive(Clone)]
pub enum Derived {
    Protect {
        request: ProtectFolder,
        keys: Keys,
        recovery: String,
    },
    Unlock {
        keys: Keys,
    },
    Change {
        request: ChangeFolderPassword,
        keys: Keys,
        recovery: Option<String>,
    },
}

impl std::fmt::Debug for Derived {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Protect { .. } => "Derived::Protect",
            Self::Unlock { .. } => "Derived::Unlock",
            Self::Change { .. } => "Derived::Change",
        })
    }
}

/// What the browser takes from the dialog.
pub enum Outcome {
    Task(Task<Message>),
    Closed,
    /// The folder's keys are open: a folder just protected, unlocked, or with a new password.
    /// The dialog stays when it still shows a new recovery key.
    Opened {
        folder: ProtectedFolder,
        keys: Keys,
        close: bool,
    },
    /// The folder is being unprotected now; the dialog closed.
    Unprotecting(ProtectedFolder),
    /// The folder is no longer protected; the dialog closed.
    Removed(NodeId),
}

enum Step {
    Protect {
        node: Node,
        password: String,
        repeat: String,
    },
    Unlock {
        folder: ProtectedFolder,
        name: String,
        password: String,
    },
    /// A new password, proven with the current one or, `recovering`, with the recovery key.
    Change {
        folder: ProtectedFolder,
        name: String,
        current: String,
        recovering: bool,
        password: String,
        repeat: String,
    },
    Unprotect {
        folder: ProtectedFolder,
        name: String,
    },
    /// Asks the user to confirm it is them, then goes back to `back` (closing when none).
    Confirm {
        message: String,
        methods: Vec<AuthMethod>,
        secret: String,
        back: Option<Box<Step>>,
    },
    RecoveryKey {
        name: String,
        key: String,
        kept: bool,
        saved_to: Option<PathBuf>,
    },
    Binned {
        name: String,
        items: Vec<TrashItem>,
    },
    SecurityKey {
        name: String,
    },
}

/// What is sent to the server, kept to send again once the user confirmed it is them.
#[derive(Clone)]
enum Pending {
    Protect {
        request: ProtectFolder,
        keys: Keys,
        recovery: String,
        name: String,
    },
    Change {
        id: NodeId,
        request: ChangeFolderPassword,
        keys: Keys,
        recovery: Option<String>,
        name: String,
    },
    Unprotect(NodeId),
    Remove(NodeId),
}

/// A dialog about a protected folder.
pub struct Dialog {
    step: Step,
    busy: bool,
    error: Option<String>,
    pending: Option<Pending>,
}

impl Dialog {
    fn with(step: Step) -> Self {
        Self {
            step,
            busy: false,
            error: None,
            pending: None,
        }
    }

    /// Asks for a password to protect `node` with.
    pub fn protect(node: Node) -> (Self, Task<Message>) {
        let dialog = Self::with(Step::Protect {
            node,
            password: String::new(),
            repeat: String::new(),
        });
        (dialog, operation::focus(FIRST_INPUT))
    }

    /// Asks for the password of `folder`, named `name`.
    pub fn unlock(folder: ProtectedFolder, name: String) -> (Self, Task<Message>) {
        if folder.lock == FolderLock::SecurityKey {
            return (Self::with(Step::SecurityKey { name }), Task::none());
        }
        let dialog = Self::with(Step::Unlock {
            folder,
            name,
            password: String::new(),
        });
        (dialog, operation::focus(FIRST_INPUT))
    }

    /// Asks for the current and a new password of `folder`.
    pub fn change_password(folder: ProtectedFolder, name: String) -> (Self, Task<Message>) {
        let dialog = Self::with(Step::Change {
            folder,
            name,
            current: String::new(),
            recovering: false,
            password: String::new(),
            repeat: String::new(),
        });
        (dialog, operation::focus(FIRST_INPUT))
    }

    /// Confirms unprotecting `folder`.
    pub fn unprotect(folder: ProtectedFolder, name: String) -> Self {
        Self::with(Step::Unprotect { folder, name })
    }

    /// Asks the user to confirm it is them, then removes the protection of folder `id`, whose
    /// content is decrypted already.
    pub fn confirm_removal(
        id: NodeId,
        message: String,
        methods: Vec<AuthMethod>,
    ) -> (Self, Task<Message>) {
        let dialog = Self {
            pending: Some(Pending::Remove(id)),
            ..Self::with(Step::Confirm {
                message,
                methods,
                secret: String::new(),
                back: None,
            })
        };
        (dialog, operation::focus(FIRST_INPUT))
    }

    /// Offers to delete for good `items`, the files of the newly protected folder `name` that
    /// are in the recycle bin, still plain.
    pub fn binned(name: String, items: Vec<TrashItem>) -> Self {
        Self::with(Step::Binned { name, items })
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        match message {
            Message::Input(field, value) => {
                self.input(field, value);
                Outcome::Task(Task::none())
            }
            Message::Forgot => {
                if let Step::Unlock { folder, name, .. } = &self.step {
                    self.step = Step::Change {
                        folder: folder.clone(),
                        name: name.clone(),
                        current: String::new(),
                        recovering: true,
                        password: String::new(),
                        repeat: String::new(),
                    };
                    self.error = None;
                    return Outcome::Task(operation::focus(FIRST_INPUT));
                }
                Outcome::Task(Task::none())
            }
            Message::Cancel => Outcome::Closed,
            Message::Submit if self.busy => Outcome::Task(Task::none()),
            Message::Submit => self.submit(client),
            Message::Derived(Ok(derived)) => self.derived(client, derived),
            Message::Derived(Err(err)) => self.fail(err),
            Message::Sent(Ok(folder)) => self.sent(folder),
            Message::Sent(Err(ApiError::ReauthenticationRequired { message, methods })) => {
                self.busy = false;
                let back = std::mem::replace(
                    &mut self.step,
                    Step::Binned {
                        name: String::new(),
                        items: Vec::new(),
                    },
                );
                self.step = Step::Confirm {
                    message,
                    methods,
                    secret: String::new(),
                    back: Some(Box::new(back)),
                };
                Outcome::Task(operation::focus(FIRST_INPUT))
            }
            Message::Sent(Err(err)) => {
                self.pending = None;
                self.fail(err.to_string())
            }
            Message::Confirmed(Ok(())) => {
                if let Step::Confirm { back, .. } = &mut self.step
                    && let Some(back) = back.take()
                {
                    self.step = *back;
                }
                match self.pending.clone() {
                    Some(pending) => Outcome::Task(send(client, pending)),
                    None => Outcome::Closed,
                }
            }
            Message::Confirmed(Err(err)) => self.fail(err.to_string()),
            Message::CopyKey => match &self.step {
                Step::RecoveryKey { key, .. } => Outcome::Task(iced::clipboard::write(key.clone())),
                _ => Outcome::Task(Task::none()),
            },
            Message::SaveKey => match &self.step {
                Step::RecoveryKey { name, key, .. } => {
                    Outcome::Task(save_key(name.clone(), key.clone()))
                }
                _ => Outcome::Task(Task::none()),
            },
            Message::KeySaved(result) => {
                if let Step::RecoveryKey { saved_to, .. } = &mut self.step {
                    match result {
                        Ok(Some(path)) => *saved_to = Some(path),
                        Ok(None) => {}
                        Err(err) => self.error = Some(err),
                    }
                }
                Outcome::Task(Task::none())
            }
            Message::Kept(kept) => {
                if let Step::RecoveryKey { kept: was, .. } = &mut self.step {
                    *was = kept;
                }
                Outcome::Task(Task::none())
            }
            Message::Purged(Ok(())) => Outcome::Closed,
            Message::Purged(Err(err)) => self.fail(err.to_string()),
        }
    }

    fn input(&mut self, field: Field, value: String) {
        let target = match (&mut self.step, field) {
            (Step::Protect { password, .. }, Field::Password)
            | (Step::Unlock { password, .. }, Field::Password)
            | (
                Step::Change {
                    current: password, ..
                },
                Field::Password,
            )
            | (Step::Change { password, .. }, Field::NewPassword) => password,
            (Step::Protect { repeat, .. }, Field::Repeat)
            | (Step::Change { repeat, .. }, Field::NewRepeat) => repeat,
            (Step::Confirm { secret, .. }, Field::Confirmation) => secret,
            _ => return,
        };
        *target = value;
    }

    fn fail(&mut self, err: String) -> Outcome {
        self.busy = false;
        self.error = Some(err);
        Outcome::Task(Task::none())
    }

    fn submit(&mut self, client: &Client) -> Outcome {
        let task = match &self.step {
            Step::Protect {
                node,
                password,
                repeat,
            } => {
                if let Some(problem) = password_problem(password, repeat) {
                    return self.fail(problem.to_owned());
                }
                let (id, password) = (node.id, password.clone());
                derive(move || {
                    let new =
                        protect_with_password(id, &password).map_err(|err| err.to_string())?;
                    Ok(Derived::Protect {
                        request: new.request,
                        keys: Arc::new(new.keys),
                        recovery: new.recovery_key.to_display().to_string(),
                    })
                })
            }
            Step::Unlock {
                folder, password, ..
            } => {
                let (folder, password) = (folder.clone(), password.clone());
                derive(move || {
                    unlock_password_folder(&folder, PasswordProof::Password(&password))
                        .map(|keys| Derived::Unlock {
                            keys: Arc::new(keys),
                        })
                        .map_err(|err| err.to_string())
                })
            }
            Step::Change {
                folder,
                current,
                recovering,
                password,
                repeat,
                ..
            } => {
                if let Some(problem) = password_problem(password, repeat) {
                    return self.fail(problem.to_owned());
                }
                let (folder, current, password) =
                    (folder.clone(), current.clone(), password.clone());
                let recovering = *recovering;
                derive(move || {
                    let proof = if recovering {
                        PasswordProof::RecoveryKey(&current)
                    } else {
                        PasswordProof::Password(&current)
                    };
                    let message = |err: client_core::protected::ProtectedError| err.to_string();
                    let keys = unlock_password_folder(&folder, proof).map_err(message)?;
                    let change =
                        change_folder_password(&folder, proof, &password).map_err(message)?;
                    Ok(Derived::Change {
                        request: change.request,
                        keys: Arc::new(keys),
                        recovery: change.recovery_key.map(|key| key.to_display().to_string()),
                    })
                })
            }
            Step::Unprotect { folder, .. } => {
                let pending = Pending::Unprotect(folder.node_id);
                self.pending = Some(pending.clone());
                send(client, pending)
            }
            Step::Confirm {
                methods, secret, ..
            } => {
                let secret = secret.trim().to_owned();
                if secret.is_empty() {
                    return Outcome::Task(Task::none());
                }
                let by_password = methods.contains(&AuthMethod::Password);
                if !by_password && !methods.contains(&AuthMethod::Totp) {
                    return Outcome::Task(Task::none());
                }
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
            Step::RecoveryKey { kept: true, .. } => return Outcome::Closed,
            Step::RecoveryKey { .. } => return Outcome::Task(Task::none()),
            Step::Binned { items, .. } => {
                let client = client.clone();
                let ids: Vec<NodeId> = items.iter().map(|item| item.node.id).collect();
                Task::perform(
                    async move {
                        for id in ids {
                            client.purge_trashed(id).await?;
                        }
                        Ok(())
                    },
                    Message::Purged,
                )
            }
            Step::SecurityKey { .. } => return Outcome::Closed,
        };
        self.busy = true;
        self.error = None;
        Outcome::Task(task)
    }

    /// Takes in what the password opened or made: an unlock is done; a change is sent.
    fn derived(&mut self, client: &Client, derived: Derived) -> Outcome {
        let name = self.name().to_owned();
        let pending = match derived {
            Derived::Unlock { keys } => {
                let Step::Unlock { folder, .. } = &self.step else {
                    return Outcome::Task(Task::none());
                };
                return Outcome::Opened {
                    folder: folder.clone(),
                    keys,
                    close: true,
                };
            }
            Derived::Protect {
                request,
                keys,
                recovery,
            } => Pending::Protect {
                request,
                keys,
                recovery,
                name,
            },
            Derived::Change {
                request,
                keys,
                recovery,
            } => {
                let Step::Change { folder, .. } = &self.step else {
                    return Outcome::Task(Task::none());
                };
                Pending::Change {
                    id: folder.node_id,
                    request,
                    keys,
                    recovery,
                    name,
                }
            }
        };
        self.pending = Some(pending.clone());
        Outcome::Task(send(client, pending))
    }

    /// Takes in the server's answer to what was pending.
    fn sent(&mut self, answer: Option<ProtectedFolder>) -> Outcome {
        self.busy = false;
        let Some(pending) = self.pending.take() else {
            return Outcome::Task(Task::none());
        };
        match (pending, answer) {
            (
                Pending::Protect {
                    keys,
                    recovery,
                    name,
                    ..
                },
                Some(folder),
            ) => {
                self.show_key(name, recovery);
                Outcome::Opened {
                    folder,
                    keys,
                    close: false,
                }
            }
            (
                Pending::Change {
                    keys,
                    recovery,
                    name,
                    ..
                },
                Some(folder),
            ) => {
                let close = recovery.is_none();
                if let Some(recovery) = recovery {
                    self.show_key(name, recovery);
                }
                Outcome::Opened {
                    folder,
                    keys,
                    close,
                }
            }
            (Pending::Unprotect(_), Some(folder)) => Outcome::Unprotecting(folder),
            (Pending::Remove(id), _) => Outcome::Removed(id),
            (_, None) => Outcome::Closed,
        }
    }

    fn show_key(&mut self, name: String, key: String) {
        self.error = None;
        self.step = Step::RecoveryKey {
            name,
            key,
            kept: false,
            saved_to: None,
        };
    }

    /// The name of the folder the dialog is about.
    fn name(&self) -> &str {
        match &self.step {
            Step::Protect { node, .. } => &node.name,
            Step::Unlock { name, .. }
            | Step::Change { name, .. }
            | Step::Unprotect { name, .. }
            | Step::RecoveryKey { name, .. }
            | Step::Binned { name, .. }
            | Step::SecurityKey { name } => name,
            Step::Confirm { .. } => "",
        }
    }

    /// The dialog panel, without the backdrop.
    pub fn view(&self) -> Element<'_, Message> {
        let working = |label: &'static str| if self.busy { WORKING } else { label };
        let submit = (!self.busy).then_some(Message::Submit);
        let cancel = parts::outlined(None, CANCEL, Some(Message::Cancel));
        let (lines, buttons): (Column<'_, Message>, Vec<Element<'_, Message>>) = match &self.step {
            Step::Protect {
                node,
                password,
                repeat,
            } => (
                column![
                    parts::dialog_head(
                        format!("Protect “{}” with a password", node.name),
                        "Its files and names are encrypted on this computer before they are \
                         uploaded. The server never sees the password.",
                    ),
                    secret_field("Password", password, Field::Password, true),
                    secret_field("Repeat the password", repeat, Field::Repeat, false),
                    parts::alert_info(
                        Icon::Info,
                        "The previous versions of the files in this folder will be deleted. \
                         Deleting something inside a protected folder deletes it for good.",
                        None,
                    ),
                    note(
                        "Without the password and the recovery key you get next, nobody can \
                         open the files, not even the server's administrator."
                    ),
                ],
                vec![
                    cancel,
                    parts::primary(Some(Icon::Lock), working("Protect"), submit),
                ],
            ),
            Step::Unlock { name, password, .. } => (
                column![
                    parts::dialog_title(format!("Unlock “{name}”")),
                    secret_field("Password", password, Field::Password, true),
                    parts::ghost(None, "Forgot the password?", Some(Message::Forgot)),
                ],
                vec![cancel, parts::primary(None, working("Unlock"), submit)],
            ),
            Step::Change {
                name,
                current,
                recovering,
                password,
                repeat,
                ..
            } => {
                let head: Element<'_, Message> = if *recovering {
                    parts::dialog_head(
                        format!("Set a new password for “{name}”"),
                        "Type the folder's recovery key, then choose a new password. You get a \
                         new recovery key, and the old one stops working.",
                    )
                } else {
                    parts::dialog_title(format!("Change the password of “{name}”"))
                };
                let proof = if *recovering {
                    "Recovery key"
                } else {
                    "Current password"
                };
                (
                    column![
                        head,
                        secret_field(proof, current, Field::Password, true),
                        secret_field("New password", password, Field::NewPassword, false),
                        secret_field("Repeat the new password", repeat, Field::NewRepeat, false),
                    ],
                    vec![
                        cancel,
                        parts::primary(None, working("Set password"), submit),
                    ],
                )
            }
            Step::Unprotect { name, .. } => (
                column![
                    parts::dialog_head(
                        format!("Unprotect “{name}”?"),
                        "Its files and names are decrypted on this computer and stored on the \
                         server in plain again, where the server can read them.",
                    ),
                    parts::alert_info(
                        Icon::Info,
                        "The previous versions of the files in this folder will be deleted.",
                        None,
                    ),
                    note(
                        "Keep the app open until it is done; if it stops, Finish unprotecting goes on."
                    ),
                ],
                vec![cancel, parts::danger(working("Unprotect"), submit)],
            ),
            Step::Confirm {
                message,
                methods,
                secret,
                ..
            } => {
                let label = if methods.contains(&AuthMethod::Password) {
                    Some("Your account password")
                } else if methods.contains(&AuthMethod::Totp) {
                    Some("Code from your authenticator app")
                } else {
                    None
                };
                let mut lines = column![parts::dialog_head("Confirm it's you", message.as_str())];
                lines = match label {
                    Some(label) => {
                        lines.push(secret_field(label, secret, Field::Confirmation, true))
                    }
                    None => lines.push(text(NO_WAY_TO_CONFIRM).style(theme::muted_text)),
                };
                let confirm = label.map(|_| parts::primary(None, working("Confirm"), submit));
                (lines, std::iter::once(cancel).chain(confirm).collect())
            }
            Step::RecoveryKey {
                name,
                key,
                kept,
                saved_to,
            } => {
                let saved = saved_to.as_ref().map(|path| {
                    text(format!("Saved to {}", path.display()))
                        .size(parts::SECOND_LINE_SIZE)
                        .style(theme::muted_text)
                });
                (
                    column![
                        parts::dialog_head(
                            "Save the recovery key",
                            format!(
                                "If you forget the password, this key is the only way to open \
                                 “{name}”. Keep it somewhere safe, away from this computer. It \
                                 is shown only now."
                            ),
                        ),
                        text(key.as_str()).font(theme::MONO).size(16),
                        iced::widget::row![
                            parts::outlined(Some(Icon::Copy), "Copy", Some(Message::CopyKey)),
                            parts::outlined(
                                Some(Icon::Download),
                                "Save to a file…",
                                Some(Message::SaveKey)
                            ),
                        ]
                        .spacing(8),
                    ]
                    .push(saved)
                    .push(
                        checkbox(*kept)
                            .label("I have saved the recovery key")
                            .on_toggle(Message::Kept),
                    ),
                    vec![parts::primary(
                        None,
                        "Done",
                        kept.then_some(Message::Submit),
                    )],
                )
            }
            Step::Binned { name, items } => {
                let listed: Vec<Element<'_, Message>> = items
                    .iter()
                    .take(10)
                    .map(|item| {
                        text(item.original_path.as_str())
                            .size(parts::SECOND_LINE_SIZE)
                            .style(theme::muted_text)
                            .into()
                    })
                    .collect();
                let more = (items.len() > 10).then(|| {
                    text(format!("and {} more", items.len() - 10))
                        .size(parts::SECOND_LINE_SIZE)
                        .style(theme::muted_text)
                });
                (
                    column![parts::dialog_head(
                        format!(
                            "Delete {} of “{name}” from the recycle bin?",
                            files(items.len())
                        ),
                        "They were deleted from the folder before it was protected, so they are \
                         still in the bin unencrypted. Deleting them for good cannot be undone.",
                    )]
                    .extend(listed)
                    .push(more),
                    vec![
                        parts::outlined(None, "Keep them", Some(Message::Cancel)),
                        parts::danger(working("Delete for good"), submit),
                    ],
                )
            }
            Step::SecurityKey { name } => (
                column![parts::dialog_head(
                    format!("“{name}” is protected with a security key"),
                    SECURITY_KEY_LATER,
                )],
                vec![parts::primary(None, "Close", Some(Message::Cancel))],
            ),
        };
        let lines = match &self.error {
            Some(error) => lines.push(parts::alert_error(Icon::Error, error.as_str(), None)),
            None => lines,
        };
        parts::dialog(parts::DIALOG_WIDTH, lines.width(Length::Fill), buttons)
    }
}

fn files(count: usize) -> String {
    if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    }
}

fn note(line: &str) -> Element<'_, Message> {
    text(line)
        .size(parts::SECOND_LINE_SIZE)
        .style(theme::muted_text)
        .into()
}

/// A field that hides what is typed; `first` takes the focus when the dialog opens.
fn secret_field<'a>(
    label: &'a str,
    value: &'a str,
    field: Field,
    first: bool,
) -> Element<'a, Message> {
    let input = text_input("", value)
        .secure(true)
        .style(theme::input)
        .padding(parts::FIELD_PADDING)
        .on_input(move |value| Message::Input(field, value))
        .on_submit(Message::Submit);
    let input = if first { input.id(FIRST_INPUT) } else { input };
    parts::field(label, input, None)
}

/// Works out `work`, which derives a key from a password, off the interface's thread.
fn derive(work: impl FnOnce() -> Result<Derived, String> + Send + 'static) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(work)
                .await
                .map_err(|err| err.to_string())?
        },
        Message::Derived,
    )
}

/// Sends `pending` to the server.
fn send(client: &Client, pending: Pending) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move {
            match pending {
                Pending::Protect { request, .. } => client.protect_folder(&request).await.map(Some),
                Pending::Change { id, request, .. } => {
                    client.change_folder_password(id, &request).await.map(Some)
                }
                Pending::Unprotect(id) => client.unprotect_folder(id).await.map(Some),
                Pending::Remove(id) => client.remove_protection(id).await.map(|()| None),
            }
        },
        Message::Sent,
    )
}

/// Asks where to save the recovery key of folder `name` and writes it there.
fn save_key(name: String, key: String) -> Task<Message> {
    Task::perform(
        async move {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_file_name(KEY_FILE)
                .save_file()
                .await
            else {
                return Ok(None);
            };
            let path = handle.path().to_path_buf();
            let content = format!("Home Space recovery key for the folder “{name}”\n\n{key}\n");
            tokio::fs::write(&path, content)
                .await
                .map_err(|err| format!("{}: {err}", path.display()))?;
            Ok(Some(path))
        },
        Message::KeySaved,
    )
}
