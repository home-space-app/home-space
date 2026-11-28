//! The security-key steps of the protected folder dialog: protecting a folder with a security
//! key, unlocking one (or opening it with the recovery key of the folder keys when the key is
//! lost), and the user's folder keys: adding one, removing one, and creating the recovery key.
//!
//! Each step that asks the key runs [`SecurityKey`] off the interface's thread, with the PIN the
//! user typed here; while it waits, the dialog says which key to touch and for what.

use std::sync::Arc;

use iced::{
    Element, Task,
    widget::{Column, checkbox, column, operation, row, text, text_input},
};
use zeroize::Zeroizing;

use api_client::{ApiError, Client};
use client_core::protected::{
    FolderKey, ProtectedError, RotationProof, VaultProof, enable_folder_key, folder_keys,
    new_vault_recovery_key, open_with_vault, protect_with_vault, recovery_unlocker, rotate_vault,
    with_vault,
};
use protocol::{MAX_KEY_NAME_LEN, Node, ProtectedFolder, UserId, Vault};
use uuid::Uuid;

use super::{
    CANCEL, Derived, Dialog, FIRST_INPUT, Field, Message, Outcome, Pending, Step, Tell, WORKING,
    derive, derive_telling, note, secret_field, send,
};
use crate::{
    browser::parts,
    icons::Icon,
    locale::short_date,
    protected::security_key::{KeyError, KeyRing, SecurityKey},
    theme,
};

pub(crate) const NO_KEY: &str =
    "This folder cannot be protected securely without a security key. Add one first.";
pub(crate) const ONE_KEY: &str =
    "Only this key opens the folder: if you lose it, the files are lost.";
const LOADING: &str = "Loading your security keys…";
const TOUCH: &str = "Touch your security key…";

/// The vault, once the server listed it: `None` before the user's first folder key.
pub(super) enum Loaded {
    Waiting,
    Vault(Option<Vault>),
}

impl Loaded {
    fn vault(&self) -> Option<&Vault> {
        match self {
            Self::Vault(vault) => vault.as_ref(),
            Self::Waiting => None,
        }
    }
}

pub(super) enum KeyStep {
    /// Protecting `node` with the vault.
    Protect {
        node: Node,
        vault: Loaded,
        pin: String,
    },
    /// Unlocking `folder` with a folder key, or with the vault's recovery key.
    Unlock {
        folder: ProtectedFolder,
        name: String,
        vault: Loaded,
        pin: String,
        recovering: bool,
        recovery: String,
    },
    /// The user's folder keys and recovery key.
    List { vault: Loaded },
    /// A new recovery key, made after opening the vault with a key the user has or with the
    /// recovery key from before.
    NewRecovery {
        vault: Vault,
        pin: String,
        recovering: bool,
        recovery: String,
    },
    /// The new key's name and PIN; `vault` is `None` for the first key, which starts it. Another
    /// key opens the vault with a key the user has (`existing` is its PIN) or the recovery key.
    Add {
        vault: Option<Vault>,
        name: String,
        pin: String,
        existing: String,
        recovering: bool,
        recovery: String,
    },
    /// Removing `key`; when it is `lost`, the vault key is replaced too, which needs every key
    /// that stays (`pins`, in the order of [`staying`]) or, with none, the recovery key.
    Remove {
        key: FolderKey,
        vault: Vault,
        lost: bool,
        pins: Vec<String>,
        recovery: String,
    },
}

impl KeyStep {
    pub(super) fn field(&mut self, field: Field) -> Option<&mut String> {
        match (self, field) {
            (
                Self::Protect { pin, .. } | Self::Unlock { pin, .. } | Self::Add { pin, .. },
                Field::Pin,
            ) => Some(pin),
            (Self::NewRecovery { pin, .. }, Field::ExistingPin) => Some(pin),
            (
                Self::Unlock { recovery, .. }
                | Self::NewRecovery { recovery, .. }
                | Self::Add { recovery, .. }
                | Self::Remove { recovery, .. },
                Field::Recovery,
            ) => Some(recovery),
            (Self::Remove { pins, .. }, Field::KeyPin(index)) => pins.get_mut(index),
            (Self::Add { existing, .. }, Field::ExistingPin) => Some(existing),
            (Self::Add { name, .. }, Field::KeyName) => Some(name),
            _ => None,
        }
    }

    pub(super) fn folder(&self) -> Option<&ProtectedFolder> {
        match self {
            Self::Unlock { folder, .. } => Some(folder),
            _ => None,
        }
    }

    pub(super) fn name(&self) -> &str {
        match self {
            Self::Protect { node, .. } => &node.name,
            Self::Unlock { name, .. } => name,
            _ => "",
        }
    }
}

/// The RP id new folder keys are made for: the host of the server this app is signed in to.
fn rp_id(client: &Client) -> Result<String, String> {
    url::Url::parse(client.base_url())
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .ok_or_else(|| format!("{} has no host name for a security key.", client.base_url()))
}

fn load_vault(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(async move { client.vault().await }, Message::VaultLoaded)
}

fn pin(value: &str) -> Zeroizing<String> {
    Zeroizing::new(value.to_owned())
}

fn message(err: impl std::fmt::Display) -> String {
    err.to_string()
}

/// The folder keys of `vault` that stay once `removed` is gone.
fn staying(vault: &Vault, removed: &FolderKey) -> Vec<FolderKey> {
    folder_keys(vault)
        .into_iter()
        .filter(|key| key.id != removed.id)
        .collect()
}

/// The names of `keys` as a choice: “A” or “B”, or “A”, “B” or “C”.
fn either(keys: &[FolderKey]) -> String {
    let names: Vec<String> = keys.iter().map(|key| format!("“{}”", key.name)).collect();
    match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Asks for one of `keys`, the user's folder keys, by name.
fn plug_one_of(keys: &[FolderKey]) -> String {
    match keys {
        [key] => format!("Plug in “{}”, the key you already have.", key.name),
        [] => "Plug in a key you already have.".to_owned(),
        _ => format!(
            "Plug in one of the keys you already have: {}.",
            either(keys)
        ),
    }
}

/// What to say when looking for one of `keys` failed: which keys would do when the one
/// plugged in is none of them.
fn not_yours(err: KeyError, keys: &[FolderKey]) -> String {
    match err {
        KeyError::NotEnabled => format!("{err} {}", plug_one_of(keys)),
        err => err.to_string(),
    }
}

fn key_error(err: KeyError) -> ProtectedError {
    ProtectedError::SecurityKey(err.to_string())
}

/// What opens the vault before changing it: the PIN of a key the user has, or the recovery key.
enum Proof {
    Pin(Zeroizing<String>),
    RecoveryKey(Zeroizing<String>),
}

impl Proof {
    /// The proof the user typed; `None` while it is empty.
    fn new(recovering: bool, pin: &str, recovery: &str) -> Option<Self> {
        let (typed, make): (&str, fn(Zeroizing<String>) -> Self) = if recovering {
            (recovery, Self::RecoveryKey)
        } else {
            (pin, Self::Pin)
        };
        (!typed.is_empty()).then(|| make(Zeroizing::new(typed.to_owned())))
    }

    /// Finds the key the PIN is for among those plugged in, keeping it in `found`. With
    /// `tell`, waits for one to be plugged in when none is, asking the user for it.
    fn open<'a>(
        &'a self,
        keys: &'a [FolderKey],
        found: &'a mut Option<SecurityKey>,
        tell: Option<&Tell>,
    ) -> Result<VaultProof<'a, SecurityKey>, String> {
        match self {
            Self::RecoveryKey(text) => Ok(VaultProof::RecoveryKey(text)),
            Self::Pin(pin) => {
                let looked = match tell {
                    Some(tell) => SecurityKey::holding_or_wait(keys, pin.clone(), || {
                        tell.say(plug_one_of(keys));
                    }),
                    None => SecurityKey::holding(keys, pin.clone()),
                };
                let (key, index) = looked.map_err(|err| not_yours(err, keys))?;
                if let Some(tell) = tell {
                    tell.say(format!(
                        "Touch “{}”, the key you already have.",
                        keys[index].name
                    ));
                }
                Ok(VaultProof::Key {
                    auth: found.insert(key),
                    key: &keys[index],
                })
            }
        }
    }
}

impl Dialog {
    fn key(step: KeyStep) -> Self {
        Self::with(Step::Key(step))
    }

    /// Asks for the PIN of a folder key to protect `node` with.
    pub fn protect_with_key(client: &Client, node: Node) -> (Self, Task<Message>) {
        let dialog = Self::key(KeyStep::Protect {
            node,
            vault: Loaded::Waiting,
            pin: String::new(),
        });
        (
            dialog,
            Task::batch([load_vault(client), operation::focus(FIRST_INPUT)]),
        )
    }

    /// Asks for the PIN of a folder key to unlock `folder`, named `name`, with.
    pub(super) fn unlock_with_key(
        client: &Client,
        folder: ProtectedFolder,
        name: String,
    ) -> (Self, Task<Message>) {
        let dialog = Self::key(KeyStep::Unlock {
            folder,
            name,
            vault: Loaded::Waiting,
            pin: String::new(),
            recovering: false,
            recovery: String::new(),
        });
        (
            dialog,
            Task::batch([load_vault(client), operation::focus(FIRST_INPUT)]),
        )
    }

    /// Lists the user's folder keys.
    pub fn folder_keys(client: &Client) -> (Self, Task<Message>) {
        (
            Self::key(KeyStep::List {
                vault: Loaded::Waiting,
            }),
            load_vault(client),
        )
    }

    pub(super) fn vault_loaded(&mut self, loaded: Result<Option<Vault>, ApiError>) -> Outcome {
        match loaded {
            Ok(found) => {
                if let Step::Key(
                    KeyStep::Protect { vault, .. }
                    | KeyStep::Unlock { vault, .. }
                    | KeyStep::List { vault },
                ) = &mut self.step
                {
                    *vault = Loaded::Vault(found);
                }
                Outcome::Task(Task::none())
            }
            Err(err) => self.fail(err.to_string()),
        }
    }

    pub(super) fn use_recovery(&mut self, use_it: bool) -> Outcome {
        if let Step::Key(
            KeyStep::Unlock { recovering, .. }
            | KeyStep::NewRecovery { recovering, .. }
            | KeyStep::Add { recovering, .. },
        ) = &mut self.step
        {
            *recovering = use_it;
            self.error = None;
        }
        Outcome::Task(operation::focus(FIRST_INPUT))
    }

    pub(super) fn show_keys(&mut self, client: &Client) -> Outcome {
        if self.busy {
            crate::protected::security_key::cancel();
        }
        self.error = None;
        self.busy = false;
        self.step = Step::Key(KeyStep::List {
            vault: Loaded::Waiting,
        });
        Outcome::Task(load_vault(client))
    }

    /// Goes on to add a security key: opening the vault first when there is one.
    pub(super) fn add_key(&mut self, client: &Client) -> Outcome {
        let vault = match &self.step {
            Step::Key(KeyStep::Protect { node, vault, .. }) => {
                self.return_to = Some(node.clone());
                vault
            }
            Step::Key(KeyStep::List { vault }) => vault,
            _ => return Outcome::Task(Task::none()),
        };
        let Loaded::Vault(vault) = vault else {
            return Outcome::Task(Task::none());
        };
        if let Err(err) = rp_id(client) {
            return self.fail(err);
        }
        self.error = None;
        let recovering = vault
            .as_ref()
            .is_some_and(|vault| folder_keys(vault).is_empty());
        self.step = Step::Key(KeyStep::Add {
            vault: vault.clone(),
            name: String::new(),
            pin: String::new(),
            existing: String::new(),
            recovering,
            recovery: String::new(),
        });
        Outcome::Task(operation::focus(FIRST_INPUT))
    }

    /// Goes on to create the recovery key, opening the vault first.
    pub(super) fn create_recovery(&mut self) -> Outcome {
        let vault = match &self.step {
            Step::Key(KeyStep::Protect { vault, node, .. }) => {
                self.return_to = Some(node.clone());
                vault
            }
            Step::Key(KeyStep::List { vault }) => vault,
            _ => return Outcome::Task(Task::none()),
        };
        let Some(vault) = vault.vault().cloned() else {
            return Outcome::Task(Task::none());
        };
        self.error = None;
        let recovering = folder_keys(&vault).is_empty();
        self.step = Step::Key(KeyStep::NewRecovery {
            vault,
            pin: String::new(),
            recovering,
            recovery: String::new(),
        });
        Outcome::Task(operation::focus(FIRST_INPUT))
    }

    pub(super) fn remove_key(&mut self, id: Uuid) -> Outcome {
        let Step::Key(KeyStep::List { vault }) = &self.step else {
            return Outcome::Task(Task::none());
        };
        let Some(vault) = vault.vault() else {
            return Outcome::Task(Task::none());
        };
        let Some(key) = folder_keys(vault).into_iter().find(|key| key.id == id) else {
            return Outcome::Task(Task::none());
        };
        self.error = None;
        let pins = vec![String::new(); staying(vault, &key).len()];
        self.step = Step::Key(KeyStep::Remove {
            key,
            vault: vault.clone(),
            lost: false,
            pins,
            recovery: String::new(),
        });
        Outcome::Task(Task::none())
    }

    /// Whether the key to remove is lost, so that the vault key is replaced too.
    pub(super) fn lost(&mut self, is_lost: bool) -> Outcome {
        if let Step::Key(KeyStep::Remove { lost, .. }) = &mut self.step {
            *lost = is_lost;
            self.error = None;
            if is_lost {
                return Outcome::Task(operation::focus(FIRST_INPUT));
            }
        }
        Outcome::Task(Task::none())
    }

    /// Starts what the step asks of the key or the server; `None` when it is not ready.
    pub(super) fn key_submit(&mut self, client: &Client, user: UserId) -> Option<Task<Message>> {
        let Step::Key(step) = &self.step else {
            return None;
        };
        match step {
            KeyStep::Protect {
                node,
                vault,
                pin: typed,
            } => {
                let vault = vault.vault()?.clone();
                let keys = folder_keys(&vault);
                if keys.is_empty() || typed.is_empty() {
                    return None;
                }
                let (id, pin) = (node.id, pin(typed));
                Some(derive(move || {
                    let (mut key, index) =
                        SecurityKey::holding(&keys, pin).map_err(|err| not_yours(err, &keys))?;
                    let proof = VaultProof::Key {
                        auth: &mut key,
                        key: &keys[index],
                    };
                    let (request, keys) = with_vault(&vault, user, proof, |vault_key| {
                        protect_with_vault(&vault, vault_key, id)
                    })
                    .map_err(message)?;
                    Ok(Derived::Protect {
                        request,
                        keys: Arc::new(keys),
                        recovery: None,
                    })
                }))
            }
            KeyStep::Unlock {
                folder,
                vault,
                pin: typed,
                recovering,
                recovery,
                ..
            } => {
                let vault = vault.vault()?.clone();
                let folder = folder.clone();
                if *recovering {
                    let typed = Zeroizing::new(recovery.clone());
                    return Some(derive(move || {
                        let proof = VaultProof::<SecurityKey>::RecoveryKey(&typed);
                        let keys = with_vault(&vault, user, proof, |vault_key| {
                            open_with_vault(&vault, vault_key, &folder)
                        })
                        .map_err(message)?;
                        Ok(Derived::Unlock {
                            keys: Arc::new(keys),
                            key: None,
                        })
                    }));
                }
                if typed.is_empty() {
                    return None;
                }
                let pin = pin(typed);
                Some(derive(move || {
                    let keys = folder_keys(&vault);
                    let (mut key, index) =
                        SecurityKey::holding(&keys, pin).map_err(|err| not_yours(err, &keys))?;
                    let path = key.path().to_owned();
                    let proof = VaultProof::Key {
                        auth: &mut key,
                        key: &keys[index],
                    };
                    let opened = with_vault(&vault, user, proof, |vault_key| {
                        open_with_vault(&vault, vault_key, &folder)
                    })
                    .map_err(message)?;
                    Ok(Derived::Unlock {
                        keys: Arc::new(opened),
                        key: Some(path),
                    })
                }))
            }
            KeyStep::List { .. } => None,
            KeyStep::NewRecovery {
                vault,
                pin: typed,
                recovering,
                recovery,
            } => {
                let vault = vault.clone();
                let proof = Proof::new(*recovering, typed, recovery)?;
                Some(derive(move || {
                    let keys = folder_keys(&vault);
                    let mut found = None;
                    let proof = proof.open(&keys, &mut found, None)?;
                    let (request, key) = with_vault(&vault, user, proof, |vault_key| {
                        new_vault_recovery_key(&vault, vault_key, user)
                    })
                    .map_err(message)?;
                    Ok(Derived::Unlocker {
                        request,
                        recovery: Some(key.to_display().to_string()),
                    })
                }))
            }
            KeyStep::Add {
                vault,
                name,
                pin: typed,
                existing,
                recovering,
                recovery,
            } => {
                let name = name.trim().to_owned();
                let vault = vault.clone();
                let proof = match &vault {
                    Some(_) => Some(Proof::new(*recovering, existing, recovery)?),
                    None => None,
                };
                if name.is_empty() || typed.is_empty() {
                    return None;
                }
                let pin = pin(typed);
                if name.chars().count() > MAX_KEY_NAME_LEN {
                    self.error = Some(format!(
                        "A key's name has at most {MAX_KEY_NAME_LEN} characters."
                    ));
                    return None;
                }
                let rp_id = match rp_id(client) {
                    Ok(rp_id) => rp_id,
                    Err(err) => {
                        self.error = Some(err);
                        return None;
                    }
                };
                // The key that opens the vault and the new one may be plugged in together, or one
                // after the other: each waits to be plugged in, and the dialog says which.
                Some(derive_telling(move |tell| {
                    let add = |vault: Option<(&Vault, &vault_core::VaultKey)>,
                               existing: &[FolderKey],
                               opened_with: Option<String>| {
                        let swap = match opened_with {
                            Some(name) => format!("Unplug “{name}” and plug in the new key."),
                            None => "Plug in the new key.".to_owned(),
                        };
                        let mut key =
                            SecurityKey::without_or_wait(existing, pin, || tell.say(swap))
                                .map_err(key_error)?;
                        tell.say("Touch the new key, twice.");
                        let credential = key.make_credential(&rp_id).map_err(key_error)?;
                        enable_folder_key(&mut key, vault, user, &name, &credential, &rp_id)
                    };
                    let request = match (&vault, proof) {
                        (Some(vault), Some(proof)) => {
                            let keys = folder_keys(vault);
                            let mut found = None;
                            let proof = proof.open(&keys, &mut found, Some(&tell))?;
                            let opened_with = match &proof {
                                VaultProof::Key { key, .. } => Some(key.name.clone()),
                                VaultProof::RecoveryKey(_) => None,
                            };
                            with_vault(vault, user, proof, |vault_key| {
                                add(Some((vault, vault_key)), &keys, opened_with)
                            })
                        }
                        _ => add(None, &[], None),
                    }
                    .map_err(message)?;
                    Ok(Derived::Unlocker {
                        request,
                        recovery: None,
                    })
                }))
            }
            KeyStep::Remove {
                key, lost: false, ..
            } => {
                let pending = Pending::RemoveUnlocker(key.id);
                self.pending = Some(pending.clone());
                Some(send(client, pending))
            }
            KeyStep::Remove {
                key,
                vault,
                lost: true,
                pins,
                recovery,
            } => {
                let staying = staying(vault, key);
                if staying.is_empty() && recovery.is_empty() || pins.iter().any(String::is_empty) {
                    return None;
                }
                let wanted: Vec<(FolderKey, Zeroizing<String>)> = staying
                    .into_iter()
                    .zip(pins.iter().map(|typed| pin(typed)))
                    .collect();
                let (vault, removed) = (vault.clone(), key.id);
                let recovery = Zeroizing::new(recovery.clone());
                let client = client.clone();
                Some(Task::perform(
                    async move {
                        // Every security-key folder, as the server lists it now.
                        let folders = client.protected_folders().await.map_err(message)?;
                        let rotate = move || {
                            let mut ring = KeyRing::gather(&wanted).map_err(message)?;
                            let proof = match wanted.first() {
                                Some((key, _)) => RotationProof::Key(key),
                                None => RotationProof::RecoveryKey(&recovery),
                            };
                            let rotation =
                                rotate_vault(&mut ring, &vault, user, proof, &folders, &[removed])
                                    .map_err(message)?;
                            Ok(Derived::Rotation {
                                request: rotation.request,
                                recovery: rotation
                                    .recovery
                                    .map(|(request, key)| (request, key.to_display().to_string())),
                            })
                        };
                        tokio::task::spawn_blocking(rotate).await.map_err(message)?
                    },
                    Message::Derived,
                ))
            }
        }
    }

    /// Takes in that the server changed the folder keys as asked.
    pub(super) fn vault_sent(&mut self, client: &Client) -> Outcome {
        self.busy = false;
        match self.pending.take() {
            Some(Pending::AddUnlocker {
                recovery: Some(key),
                ..
            }) => {
                self.error = None;
                self.step = Step::RecoveryKey {
                    name: String::new(),
                    key,
                    kept: false,
                    saved_to: None,
                    vault: true,
                };
                Outcome::Task(Task::none())
            }
            Some(Pending::AddUnlocker { recovery: None, .. }) => self.back(client),
            // The vault key is replaced; the recovery key that replaces the old one goes next.
            Some(Pending::Rotate {
                recovery: Some((request, key)),
                ..
            }) => {
                let pending = Pending::AddUnlocker {
                    request,
                    recovery: Some(key),
                };
                self.pending = Some(pending.clone());
                self.busy = true;
                Outcome::Task(send(client, pending))
            }
            _ => self.show_keys(client),
        }
    }

    /// Goes back to where the change of the folder keys started: protecting a folder, or the
    /// list of keys.
    pub(super) fn back(&mut self, client: &Client) -> Outcome {
        match self.return_to.take() {
            Some(node) => {
                let (dialog, task) = Self::protect_with_key(client, node);
                self.step = dialog.step;
                self.error = None;
                self.busy = false;
                Outcome::Task(task)
            }
            None => self.show_keys(client),
        }
    }

    pub(super) fn key_view<'a>(
        &'a self,
        step: &'a KeyStep,
    ) -> (Column<'a, Message>, Vec<Element<'a, Message>>) {
        let working = |label: &'static str| if self.busy { WORKING } else { label };
        let touching = |label: &'static str| if self.busy { TOUCH } else { label };
        let submit = (!self.busy).then_some(Message::Submit);
        let cancel = parts::outlined(None, CANCEL, Some(Message::Cancel));
        // A change of the keys goes back to the list it started from.
        let back = || {
            let to = if self.return_to.is_some() {
                Message::Cancel
            } else {
                Message::ShowKeys
            };
            parts::outlined(None, CANCEL, Some(to))
        };
        match step {
            KeyStep::Protect { node, vault, pin } => {
                let mut lines = column![parts::dialog_head(
                    format!("Protect “{}” with a security key", node.name),
                    "Its files and names are encrypted on this computer before they are \
                     uploaded. Only your security keys open them, each with its PIN.",
                )];
                let ready = match vault {
                    Loaded::Waiting => {
                        lines = lines.push(note(LOADING));
                        false
                    }
                    Loaded::Vault(vault) => {
                        let keys = vault.as_ref().map(folder_keys).unwrap_or_default();
                        let recovery = vault.as_ref().and_then(recovery_unlocker).is_some();
                        if keys.is_empty() {
                            lines = lines.push(parts::alert_info(
                                Icon::Info,
                                NO_KEY,
                                Some(parts::small_outlined("Add a key…", Some(Message::AddKey))),
                            ));
                        } else {
                            lines = lines
                                .push(uses_keys(&keys))
                                .push(secret_field(pin_label(&keys), pin, Field::Pin, true))
                                .push(parts::alert_info(
                                    Icon::Info,
                                    "The previous versions of the files in this folder will be \
                                     deleted. Deleting something inside a protected folder deletes \
                                     it for good.",
                                    None,
                                ));
                            if keys.len() == 1 && !recovery {
                                lines = lines.push(
                                    column![
                                        note(ONE_KEY),
                                        row![
                                            parts::ghost(
                                                Some(Icon::Key),
                                                "Add a key…",
                                                Some(Message::AddKey)
                                            ),
                                            parts::ghost(
                                                None,
                                                "Create a recovery key…",
                                                Some(Message::CreateRecovery)
                                            ),
                                        ]
                                        .spacing(8),
                                    ]
                                    .spacing(4),
                                );
                            }
                        }
                        !keys.is_empty()
                    }
                };
                let lines = lines.push(
                    self.busy
                        .then(|| note("Touch your security key to protect the folder.")),
                );
                let protect = parts::primary(
                    Some(Icon::Lock),
                    touching("Protect"),
                    submit.filter(|_| ready),
                );
                (lines, vec![cancel, protect])
            }
            KeyStep::Unlock {
                name,
                vault,
                pin,
                recovering,
                recovery,
                ..
            } => {
                let mut lines = column![parts::dialog_title(format!("Unlock “{name}”"))];
                let has_recovery = vault.vault().and_then(recovery_unlocker).is_some();
                lines = match (vault, *recovering) {
                    (Loaded::Waiting, _) => lines.push(note(LOADING)),
                    (Loaded::Vault(None), _) => lines.push(note(
                        "You have no security keys for protected folders any more, so nothing \
                         opens this folder.",
                    )),
                    (Loaded::Vault(Some(vault)), false) => {
                        let keys = folder_keys(vault);
                        lines
                            .push((!keys.is_empty()).then(|| uses_keys(&keys)))
                            .push(secret_field(pin_label(&keys), pin, Field::Pin, true))
                            .push(has_recovery.then(|| {
                                parts::ghost(
                                    None,
                                    "Lost your security key?",
                                    Some(Message::UseRecovery(true)),
                                )
                            }))
                    }
                    (Loaded::Vault(Some(_)), true) => lines
                        .push(note(
                            "Type the recovery key of your security keys. It opens every folder \
                             you protected with them.",
                        ))
                        .push(secret_field(
                            "Recovery key",
                            recovery,
                            Field::Recovery,
                            true,
                        ))
                        .push(parts::ghost(
                            None,
                            "Use a security key",
                            Some(Message::UseRecovery(false)),
                        )),
                };
                let lines = lines.push(
                    (self.busy && !*recovering)
                        .then(|| note("Touch your security key to unlock the folder.")),
                );
                let label = if *recovering {
                    working("Unlock")
                } else {
                    touching("Unlock")
                };
                let ready = vault.vault().is_some();
                (
                    lines,
                    vec![
                        cancel,
                        parts::primary(None, label, submit.filter(|_| ready)),
                    ],
                )
            }
            KeyStep::List { vault } => {
                let mut lines = column![parts::dialog_head(
                    "Security keys for protected folders",
                    "These keys open the folders you protect with a security key, each with its \
                     PIN and a touch. They are not the keys you sign in with.",
                )];
                let (keys, recovery) = match vault {
                    Loaded::Waiting => {
                        lines = lines.push(note(LOADING));
                        (Vec::new(), None)
                    }
                    Loaded::Vault(vault) => (
                        vault.as_ref().map(folder_keys).unwrap_or_default(),
                        vault.as_ref().and_then(recovery_unlocker),
                    ),
                };
                let loaded = matches!(vault, Loaded::Vault(_));
                if loaded && keys.is_empty() {
                    lines = lines.push(note("No security keys yet."));
                }
                for key in keys {
                    let added = vault
                        .vault()
                        .and_then(|vault| {
                            vault
                                .unlockers
                                .iter()
                                .find(|unlocker| unlocker.id == key.id)
                        })
                        .map(|unlocker| format!("Added {}", short_date(unlocker.created_at)))
                        .unwrap_or_default();
                    lines = lines.push(
                        row![
                            Icon::Key.view(crate::icons::ICON_SIZE, crate::icons::Tint::Muted),
                            column![
                                text(key.name.clone()).font(theme::MEDIUM),
                                text(added)
                                    .size(parts::SECOND_LINE_SIZE)
                                    .style(theme::muted_text),
                            ]
                            .width(iced::Length::Fill),
                            parts::small_outlined(
                                "Remove",
                                (!self.busy).then_some(Message::RemoveKey(key.id))
                            ),
                        ]
                        .spacing(12)
                        .align_y(iced::Alignment::Center),
                    );
                }
                if vault.vault().is_some() {
                    let (line, action) = match recovery {
                        Some(unlocker) => (
                            format!("Recovery key created {}.", short_date(unlocker.created_at)),
                            "Replace the recovery key…",
                        ),
                        None => (
                            "No recovery key: if you lose your security keys, the folders they \
                             protect are lost."
                                .to_owned(),
                            "Create a recovery key…",
                        ),
                    };
                    lines = lines.push(parts::alert_info(
                        Icon::Info,
                        line,
                        Some(parts::small_outlined(action, Some(Message::CreateRecovery))),
                    ));
                }
                let add = parts::primary(
                    Some(Icon::Key),
                    "Add a key…",
                    loaded.then_some(Message::AddKey),
                );
                (
                    lines,
                    vec![parts::outlined(None, "Close", Some(Message::Cancel)), add],
                )
            }
            KeyStep::NewRecovery {
                vault,
                pin,
                recovering,
                recovery,
            } => {
                let lines = column![parts::dialog_head(
                    "Create a recovery key",
                    "It opens the folders you protect with your security keys when you lose \
                     them. A new one replaces the one from before.",
                )];
                let (lines, by_key) = proof_fields(lines, vault, pin, recovering, recovery, true);
                let lines =
                    lines.push((self.busy && by_key).then(|| note("Touch your security key.")));
                let label = if by_key {
                    touching("Create")
                } else {
                    working("Create")
                };
                (lines, vec![back(), parts::primary(None, label, submit)])
            }
            KeyStep::Add {
                vault,
                name,
                pin,
                existing,
                recovering,
                recovery,
            } => {
                let name_input = text_input("YubiKey", name)
                    .style(theme::input)
                    .padding(parts::FIELD_PADDING)
                    .on_input(|value| Message::Input(Field::KeyName, value))
                    .on_submit(Message::Submit)
                    .id(FIRST_INPUT);
                let explain = if vault.is_some() {
                    "A key you already have opens your folder keys, or the recovery key does; \
                     then the new key is added. Plug them in together, or one after the other \
                     when asked. The new key needs a PIN."
                } else {
                    "Plug in the key to add, alone. It needs a PIN; you touch it twice: once to \
                     make its folder key, once to check it."
                };
                let mut lines = column![
                    parts::dialog_head("Add a security key", explain),
                    parts::field("Name", name_input, None),
                    secret_field("PIN of the new key", pin, Field::Pin, false),
                ];
                let mut by_key = false;
                if let Some(vault) = vault {
                    let (with_proof, uses_key) =
                        proof_fields(lines, vault, existing, recovering, recovery, false);
                    lines = with_proof;
                    by_key = uses_key;
                }
                let touch = if by_key {
                    "Touch the key you already have, then the new one twice."
                } else {
                    "Touch the new security key, twice."
                };
                let touch = self.told.as_deref().unwrap_or(touch);
                let lines = lines.push(self.busy.then(|| note(touch)));
                (
                    lines,
                    vec![
                        back(),
                        parts::primary(Some(Icon::Key), touching("Add key"), submit),
                    ],
                )
            }
            KeyStep::Remove {
                key,
                vault,
                lost,
                pins,
                recovery,
            } => {
                let mut lines = column![
                    parts::dialog_head(
                        format!("Remove “{}”?", key.name),
                        "It no longer opens your protected folders. Someone who finds a lost \
                         key still needs its PIN.",
                    ),
                    checkbox(*lost)
                        .label("It is lost, or someone else may have it")
                        .on_toggle_maybe((!self.busy).then_some(Message::Lost)),
                ];
                let staying = staying(vault, key);
                let has_recovery = recovery_unlocker(vault).is_some();
                let mut ready = true;
                if *lost {
                    lines = lines.push(note(
                        "The vault key is replaced too, so that a copy of your vault from before \
                         opens nothing, even with this key and its PIN.",
                    ));
                    if staying.is_empty() {
                        if has_recovery {
                            lines = lines.push(secret_field(
                                "Recovery key",
                                recovery,
                                Field::Recovery,
                                true,
                            ));
                        } else {
                            ready = false;
                            lines = lines.push(note(
                                "Nothing else opens your vault, so there is nothing to replace \
                                 its key with.",
                            ));
                        }
                    } else {
                        lines = lines.push(note(
                            "Plug in every other key: each one is asked for the new vault key.",
                        ));
                        for (index, (other, typed)) in staying.iter().zip(pins).enumerate() {
                            lines = lines.push(key_pin_field(&other.name, typed, index));
                        }
                        if has_recovery {
                            lines = lines.push(note(
                                "Your recovery key is replaced as well; the new one is shown \
                                 once.",
                            ));
                        }
                    }
                    let touches = match staying.as_slice() {
                        [] => None,
                        [only] => Some(format!("Touch “{}” twice.", only.name)),
                        [first, ..] => Some(format!(
                            "Touch “{}” twice, then each of your other keys once.",
                            first.name
                        )),
                    };
                    lines = lines.push(touches.filter(|_| self.busy).map(|line| {
                        text(line)
                            .size(parts::SECOND_LINE_SIZE)
                            .style(theme::muted_text)
                    }));
                }
                let label = match (*lost, staying.is_empty()) {
                    (false, _) => working("Remove"),
                    (true, true) => working("Remove and replace the vault key"),
                    (true, false) => touching("Remove and replace the vault key"),
                };
                (
                    lines,
                    vec![back(), parts::danger(label, submit.filter(|_| ready))],
                )
            }
        }
    }
}

/// Names the keys a dialog asks for: the user's only one, or any of theirs.
fn uses_keys<'a>(keys: &[FolderKey]) -> Element<'a, Message> {
    let line = match keys {
        [key] => format!("Uses your security key “{}”.", key.name),
        _ => format!(
            "Uses one of your security keys: {}. Only one is needed.",
            either(keys)
        ),
    };
    row![
        Icon::Key.view(crate::icons::ICON_SIZE, crate::icons::Tint::Muted),
        text(line).font(theme::MEDIUM),
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The label of the PIN field for `keys`: the PIN of the only one, or of whichever is plugged in.
fn pin_label(keys: &[FolderKey]) -> String {
    match keys {
        [key] => format!("PIN of “{}”", key.name),
        [] => "PIN of your security key".to_owned(),
        _ => "PIN of the key you use".to_owned(),
    }
}

/// The PIN field of the key named `name`, the `index`th that stays; the first takes the focus.
fn key_pin_field<'a>(name: &str, typed: &'a str, index: usize) -> Element<'a, Message> {
    let input = text_input("", typed)
        .secure(true)
        .style(theme::input)
        .padding(parts::FIELD_PADDING)
        .on_input(move |value| Message::Input(Field::KeyPin(index), value))
        .on_submit(Message::Submit);
    let input = if index == 0 {
        input.id(FIRST_INPUT)
    } else {
        input
    };
    parts::field(format!("PIN of “{name}”"), input, None)
}

/// The fields that open the vault before a change: the PIN of a key the user has, or the
/// recovery key, with a switch between them; whether it is a key. `first` puts the focus there.
fn proof_fields<'a>(
    lines: Column<'a, Message>,
    vault: &Vault,
    pin: &'a str,
    recovering: &bool,
    recovery: &'a str,
    first: bool,
) -> (Column<'a, Message>, bool) {
    let has_recovery = recovery_unlocker(vault).is_some();
    let has_keys = !folder_keys(vault).is_empty();
    if *recovering || !has_keys {
        let lines = lines
            .push(secret_field(
                "Recovery key",
                recovery,
                Field::Recovery,
                first,
            ))
            .push(has_keys.then(|| {
                parts::ghost(
                    None,
                    "Use a security key you have",
                    Some(Message::UseRecovery(false)),
                )
            }));
        (lines, false)
    } else {
        let lines = lines
            .push(secret_field(
                "PIN of a key you already have",
                pin,
                Field::ExistingPin,
                first,
            ))
            .push(has_recovery.then(|| {
                parts::ghost(
                    None,
                    "Use the recovery key",
                    Some(Message::UseRecovery(true)),
                )
            }));
        (lines, true)
    }
}

#[cfg(test)]
mod tests {
    use protocol::{UnlockerKind, VaultUnlocker};
    use time::OffsetDateTime;

    use super::*;

    fn keys(names: &[&str]) -> Vec<FolderKey> {
        let unlockers = names
            .iter()
            .zip(1u8..)
            .map(|(name, n)| VaultUnlocker {
                id: Uuid::from_bytes([n; 16]),
                kind: UnlockerKind::SecurityKey,
                name: Some((*name).to_owned()),
                credential_id: Some(vec![n; 16].into()),
                rp_id: Some("localhost".to_owned()),
                prf_salt: Some(vec![n; 32].into()),
                wrapped_vault_key: vec![0; protocol::WRAPPED_LEN].into(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            })
            .collect();
        folder_keys(&Vault {
            epoch: 1,
            unlockers,
        })
    }

    #[test]
    fn the_keys_that_would_do_are_named_as_a_choice() {
        assert_eq!(
            plug_one_of(&keys(&["YubiKey"])),
            "Plug in “YubiKey”, the key you already have."
        );
        assert_eq!(
            plug_one_of(&keys(&["111", "1231"])),
            "Plug in one of the keys you already have: “111” or “1231”."
        );
        assert_eq!(either(&keys(&["A", "B", "C"])), "“A”, “B” or “C”");
    }

    #[test]
    fn a_key_that_is_not_one_of_yours_says_which_would_do() {
        let yours = keys(&["YubiKey"]);
        assert_eq!(
            not_yours(KeyError::NotEnabled, &yours),
            "This security key is not one of your folder keys. Plug in “YubiKey”, the key you \
             already have."
        );
        assert_eq!(not_yours(KeyError::WrongPin, &yours), "Wrong PIN.");
    }
}
