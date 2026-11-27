//! Protected folders in the browser: the folders `GET /protected` lists, the keys of the ones the
//! user unlocked (held in memory, wiped on Lock, after [`AUTO_LOCK_AFTER`] without activity, on
//! sign-out and when the page closes), and encrypting and decrypting their files.
//!
//! [`AUTO_LOCK_AFTER`]: client_core::protected::AUTO_LOCK_AFTER
//!
//! The page decrypts and encrypts whole files in memory, so a file larger than
//! [`MAX_IN_MEMORY`] cannot go into or out of a protected folder here.

use std::{collections::HashSet, rc::Rc, time::Duration};

use gloo_timers::callback::Interval;
use js_sys::{Array, Uint8Array};
use leptos::{prelude::*, task::spawn_local};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag, File, FilePropertyBag, Response};

use api_client::ApiError;
use client_core::app_link::AppLink;
use client_core::protected::{
    ENCRYPTED_MIME, ProtectedError, UnlockedFolders, enclosing, open_listing, open_name, seal_name,
    shown_mime,
};
use protocol::{
    ErrorCode, Node, NodeId, NodeKind, ProtectedFolder, TicketRequest, TrashItem, features,
};
use vault_core::{FolderKeys, Recipient, content};

use crate::{
    components::reconfirm::Reconfirm,
    session::{self, Auth},
    upload,
};

/// The largest file the page encrypts or decrypts, held in memory twice while it does.
pub const MAX_IN_MEMORY: f64 = 512.0 * 1024.0 * 1024.0;
/// [`MAX_IN_MEMORY`] as the user reads it.
pub const MAX_IN_MEMORY_LABEL: &str = "512 MB";
/// How often the page looks for folders to lock after inactivity.
const AUTO_LOCK_CHECK_MS: u32 = 15_000;
const TOO_LARGE: &str = "Files larger than 512 MB cannot be encrypted or decrypted in the browser. Use the desktop app.";
const READ_FAILED: &str = "The file could not be read.";
const DOWNLOAD_FAILED: &str = "The file could not be downloaded.";
const LOCKED_MEANWHILE: &str = "The folder locked meanwhile.";
const ENCRYPTING: &str = "Encrypting the files of this folder…";
const STOPPED: &str = "Encrypting the files of this folder stopped; unlock it and choose Finish protecting to continue.";
const DECRYPTING: &str = "Decrypting the files of this folder…";
const UNPROTECT_STOPPED: &str = "Decrypting the files of this folder stopped; unlock it and choose Finish unprotecting to continue.";

/// Which way a folder's content is being converted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Plain files and names become encrypted ones: the folder is being protected.
    Protect,
    /// Encrypted files and names become plain ones: the folder is being unprotected.
    Unprotect,
}

/// The protected folders and the keys of the unlocked ones, shared by every page.
#[derive(Clone, Copy)]
pub struct Vault {
    folders: RwSignal<Vec<ProtectedFolder>>,
    /// Changes whenever a folder locks or unlocks, for what shows their content.
    revision: RwSignal<u64>,
    unlocked: StoredValue<UnlockedFolders, LocalStorage>,
    /// How replacing a folder's content with its encrypted form is going.
    pub converting: RwSignal<Option<Conversion>>,
    /// The `blob:` URLs of decrypted files, by folder, revoked when it locks.
    urls: StoredValue<Vec<(NodeId, String)>, LocalStorage>,
}

/// Where encrypting the content of a folder being protected stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversion {
    pub folder: NodeId,
    pub message: String,
    pub failed: bool,
}

/// Makes the vault available to every page: it loads the folders once signed in, and locks
/// everything on sign-out.
pub fn provide(auth: Auth) -> Vault {
    let vault = Vault {
        folders: RwSignal::new(Vec::new()),
        revision: RwSignal::new(0),
        unlocked: StoredValue::new_local(UnlockedFolders::default()),
        converting: RwSignal::new(None),
        urls: StoredValue::new_local(Vec::new()),
    };
    provide_context(vault);
    VAULT.set(Some(vault));
    Effect::new(move |_| {
        if auth.user.read().is_some() {
            vault.reload();
        } else {
            vault.lock_all();
            vault.folders.set(Vec::new());
        }
    });
    Interval::new(AUTO_LOCK_CHECK_MS, move || vault.lock_idle()).forget();
    vault
}

thread_local! {
    static VAULT: std::cell::Cell<Option<Vault>> = const { std::cell::Cell::new(None) };
}

/// The vault [`provide`] made.
pub fn use_vault() -> Vault {
    expect_context::<Vault>()
}

/// The vault [`provide`] made, for a task spawned where there is no component to find it
/// through.
pub fn vault() -> Vault {
    VAULT.get().unwrap_or_else(use_vault)
}

/// The time on the page's monotonic clock.
fn now() -> Duration {
    let millis = web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now());
    Duration::from_secs_f64(millis.max(0.0) / 1000.0)
}

impl Vault {
    /// Whether the server keeps protected folders; without it, Protect is not offered.
    pub fn available() -> bool {
        session::client().supports(features::PROTECTED_FOLDERS)
    }

    /// Loads the protected folders again.
    pub fn reload(self) {
        spawn_local(async move {
            match session::client().protected_folders().await {
                Ok(folders) => self.folders.set(folders),
                // A server from before protected folders.
                Err(ApiError::Api {
                    code: ErrorCode::NotFound,
                    ..
                }) => self.folders.set(Vec::new()),
                Err(_) => {}
            }
        });
    }

    /// Keeps `folder` as the server now lists it, after protecting it or changing it.
    pub fn remember(self, folder: ProtectedFolder) {
        self.folders.update(|folders| {
            folders.retain(|known| known.node_id != folder.node_id);
            folders.push(folder);
        });
    }

    /// The protection of `id`, when it is a protected folder.
    pub fn protection_of(self, id: NodeId) -> Option<ProtectedFolder> {
        self.folders
            .read()
            .iter()
            .find(|folder| folder.node_id == id)
            .cloned()
    }

    /// The protection of `id` as known now, for event handlers.
    pub fn protection_now(self, id: NodeId) -> Option<ProtectedFolder> {
        untrack(|| self.protection_of(id))
    }

    /// The protected folder holding `path`, a folder and its ancestors.
    pub fn enclosing(self, path: impl IntoIterator<Item = NodeId>) -> Option<ProtectedFolder> {
        enclosing(&self.folders.read(), path).cloned()
    }

    /// Whether the user has any protected folder, as known now.
    pub fn any_protected(self) -> bool {
        !self.folders.read_untracked().is_empty()
    }

    /// Reads the folders and the lock state, so a memo or resource runs again when they change.
    pub fn track(self) {
        self.folders.track();
        self.revision.track();
    }

    /// How many folders are unlocked; reactive.
    pub fn unlocked_count(self) -> usize {
        self.revision.track();
        let now = now();
        self.unlocked.with_value(|unlocked| {
            unlocked
                .folders()
                .filter(|folder| unlocked.is_unlocked(*folder, now))
                .count()
        })
    }

    /// `listing`, of a folder inside the protected folder `folder`, with its names opened as
    /// [`open_listing`] does; `None` when `folder` is locked.
    pub fn opened(self, folder: NodeId, listing: Vec<Node>) -> Option<Vec<Node>> {
        let state = self.protection_now(folder)?.state;
        self.with_keys(folder, |keys| open_listing(keys, listing, state))
    }

    /// Whether `id` is unlocked; reactive.
    pub fn is_unlocked(self, id: NodeId) -> bool {
        self.revision.track();
        self.unlocked
            .with_value(|unlocked| unlocked.is_unlocked(id, now()))
    }

    /// Runs `action` with the keys of `id`, counting as activity in it; `None` when locked.
    pub fn with_keys<T>(self, id: NodeId, action: impl FnOnce(&FolderKeys) -> T) -> Option<T> {
        self.unlocked
            .try_update_value(|unlocked| unlocked.use_keys(id, now()).map(action))
            .flatten()
    }

    /// The checked public key of `id`, to encrypt uploads to; `None` when locked.
    pub fn recipient(self, id: NodeId) -> Option<Recipient> {
        self.unlocked
            .try_update_value(|unlocked| unlocked.recipient(id, now()))
            .flatten()
    }

    /// Keeps the keys of `folder` until it locks.
    pub fn unlock(self, folder: &ProtectedFolder, keys: FolderKeys) -> Result<(), ProtectedError> {
        self.unlocked
            .try_update_value(|unlocked| {
                unlocked.unlock(folder.node_id, keys, &folder.recipient, now())
            })
            .unwrap_or(Ok(()))?;
        self.bump();
        Ok(())
    }

    /// Keeps `url`, a `blob:` URL of a decrypted file of `folder`, until the folder locks.
    pub fn keep_url(self, folder: NodeId, url: String) {
        self.urls.update_value(|urls| urls.push((folder, url)));
    }

    /// Locks `id`, wiping its keys.
    pub fn lock(self, id: NodeId) {
        self.unlocked.update_value(|unlocked| {
            unlocked.lock(id);
        });
        self.locked(&[id]);
    }

    /// Locks every folder.
    pub fn lock_all(self) {
        let locked = self
            .unlocked
            .try_update_value(UnlockedFolders::lock_all)
            .unwrap_or_default();
        self.locked(&locked);
    }

    /// Starts unprotecting the unlocked folder `folder` (asking for a recent sign-in first when
    /// the server wants one), then decrypts its content as [`Vault::convert`] does.
    pub async fn unprotect(
        self,
        reconfirm: Reconfirm,
        folder: NodeId,
        on_done: impl Fn() + 'static,
    ) -> Result<(), String> {
        let client = session::client();
        let Some(unprotecting) = reconfirm
            .confirmed(|| client.unprotect_folder(folder))
            .await
            .map_err(|err| err.to_string())?
        else {
            return Ok(());
        };
        self.remember(unprotecting);
        self.convert(folder, Direction::Unprotect, on_done);
        Ok(())
    }

    /// Replaces the content of the unlocked folder `folder` with its encrypted form and marks it
    /// protected, or with its decrypted form and removes the protection, as `direction` says.
    /// Safe to run again after it stopped. `on_done` runs when it stops, done or not.
    pub fn convert(self, folder: NodeId, direction: Direction, on_done: impl Fn() + 'static) {
        if self
            .converting
            .read_untracked()
            .as_ref()
            .is_some_and(|conversion| !conversion.failed)
        {
            return;
        }
        let report = move |message: String, failed: bool| {
            self.converting.set(Some(Conversion {
                folder,
                message,
                failed,
            }));
        };
        let (starting, stopped) = match direction {
            Direction::Protect => (ENCRYPTING, STOPPED),
            Direction::Unprotect => (DECRYPTING, UNPROTECT_STOPPED),
        };
        report(starting.to_owned(), false);
        spawn_local(async move {
            let client = session::client();
            let outcome = match direction {
                Direction::Protect => match encrypt_content(self, folder, &report).await {
                    Ok(()) => client
                        .finish_protection(folder)
                        .await
                        .map(|protected| self.remember(protected))
                        .map_err(|err| err.to_string()),
                    Err(message) => Err(message),
                },
                Direction::Unprotect => match decrypt_content(self, folder, &report).await {
                    Ok(()) => client
                        .remove_protection(folder)
                        .await
                        .map(|()| self.forget(folder))
                        .map_err(|err| err.to_string()),
                    Err(message) => Err(message),
                },
            };
            match outcome {
                Ok(()) => self.converting.set(None),
                Err(message) => report(format!("{stopped} {message}"), true),
            }
            on_done();
        });
    }

    /// Drops `folder`, no longer protected, locking it.
    fn forget(self, folder: NodeId) {
        self.lock(folder);
        self.folders
            .update(|folders| folders.retain(|known| known.node_id != folder));
    }

    fn lock_idle(self) {
        let locked = self
            .unlocked
            .try_update_value(|unlocked| unlocked.lock_idle(now()))
            .unwrap_or_default();
        if !locked.is_empty() {
            self.locked(&locked);
        }
    }

    /// Revokes the decrypted files of the folders that locked, and shows them locked.
    fn locked(self, folders: &[NodeId]) {
        self.urls.update_value(|urls| {
            urls.retain(|(folder, url)| {
                let revoke = folders.contains(folder);
                if revoke {
                    let _ = web_sys::Url::revoke_object_url(url);
                }
                !revoke
            });
        });
        self.bump();
    }

    fn bump(self) {
        self.revision
            .update(|revision| *revision = revision.wrapping_add(1));
    }
}

/// Hands the protected folder `folder` to the desktop app through its `home-space://` link: a
/// folder protected with a security key unlocks only in the apps.
pub fn open_in_desktop(folder: NodeId) {
    let Some(location) = web_sys::window().map(|window| window.location()) else {
        return;
    };
    let Some(link) = location
        .origin()
        .ok()
        .and_then(|origin| AppLink::folder(&origin, folder))
    else {
        return;
    };
    let _ = location.set_href(&link.to_url());
}

/// `file` encrypted to `recipient`, as a file named `sealed_name` for the upload.
pub async fn encrypt_file(
    file: &File,
    recipient: &Recipient,
    sealed_name: &str,
) -> Result<File, String> {
    if file.size() > MAX_IN_MEMORY {
        return Err(TOO_LARGE.to_owned());
    }
    let plain = blob_bytes(file).await.ok_or(READ_FAILED)?;
    encrypted_file(&plain, recipient, sealed_name)
}

/// `plain` encrypted to `recipient`, as a file named `sealed_name`. Its time is now, so an
/// upload of it never continues an upload of another encryption of the same content.
pub fn encrypted_file(
    plain: &[u8],
    recipient: &Recipient,
    sealed_name: &str,
) -> Result<File, String> {
    let encrypted = content::encrypt_bytes(recipient, plain).map_err(|err| err.to_string())?;
    let parts = Array::of1(&Uint8Array::from(encrypted.as_slice()));
    let options = FilePropertyBag::new();
    options.set_type(ENCRYPTED_MIME);
    File::new_with_u8_array_sequence_and_options(&parts, sealed_name, &options)
        .map_err(|_| READ_FAILED.to_owned())
}

/// The content of file `id` as stored, through a download ticket.
pub async fn download_bytes(id: NodeId) -> Result<Vec<u8>, String> {
    let client = session::client();
    let ticket = client
        .ticket(&TicketRequest::Download { node_id: id })
        .await
        .map_err(|err| err.to_string())?;
    let window = web_sys::window().ok_or(DOWNLOAD_FAILED)?;
    let response: Response =
        JsFuture::from(window.fetch_with_str(&client.download_url(id, &ticket.ticket)))
            .await
            .ok()
            .and_then(|response| response.dyn_into().ok())
            .ok_or(DOWNLOAD_FAILED)?;
    if !response.ok() {
        return Err(format!(
            "{DOWNLOAD_FAILED} The server answered {}.",
            response.status()
        ));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|_| DOWNLOAD_FAILED)?)
        .await
        .map_err(|_| DOWNLOAD_FAILED)?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

/// Downloads file `id` of the protected folder `folder` and decrypts it with its keys.
pub async fn download_plain(
    vault: Vault,
    folder: NodeId,
    id: NodeId,
    size: i64,
) -> Result<Vec<u8>, String> {
    let encrypted = download_bytes_within(id, size).await?;
    vault
        .with_keys(folder, |keys| {
            content::decrypt_bytes(keys.identity(), &encrypted)
        })
        .ok_or_else(|| LOCKED_MEANWHILE.to_owned())?
        .map_err(|_| {
            "The file does not decrypt: it was changed, or written without the folder's key."
                .to_owned()
        })
}

/// A `blob:` URL of `plain`, shown as the type its name says.
pub fn blob_url(plain: &[u8], name: &str) -> Option<String> {
    let options = BlobPropertyBag::new();
    options.set_type(shown_mime(name));
    let blob = Blob::new_with_u8_array_sequence_and_options(
        &Array::of1(&Uint8Array::from(plain)),
        &options,
    )
    .ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

/// A `blob:` URL of `plain` without a type, for a player that reads the format from the bytes.
pub fn untyped_blob_url(plain: &[u8]) -> Option<String> {
    let blob = Blob::new_with_u8_array_sequence(&Array::of1(&Uint8Array::from(plain))).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

async fn blob_bytes(blob: &Blob) -> Option<Vec<u8>> {
    let buffer = JsFuture::from(blob.array_buffer()).await.ok()?;
    Some(Uint8Array::new(&buffer).to_vec())
}

/// Replaces every plain file and name below `root` with its encrypted form. Safe to run again
/// after it stopped: a name that opens is done; a plain file whose encrypted copy is there
/// already was uploaded and only needs deleting. Inside a protected folder a deletion is for
/// good, which takes the old versions too.
async fn encrypt_content(
    vault: Vault,
    root: NodeId,
    report: &impl Fn(String, bool),
) -> Result<(), String> {
    let client = session::client();
    let failed = |err: ApiError| err.to_string();
    let mut pending = vec![root];
    while let Some(folder) = pending.pop() {
        let children = client.list(Some(folder)).await.map_err(failed)?;
        let names = taken(&children);
        for child in children {
            let sealed = vault
                .with_keys(root, |keys| {
                    open_name(keys, &child)
                        .is_none()
                        .then(|| seal_name(keys, folder, child.kind, &child.name))
                        .transpose()
                })
                .ok_or(LOCKED_MEANWHILE)?
                .map_err(|err| format!("“{}”: {err}", child.name))?;
            match (child.kind, sealed) {
                (NodeKind::Folder, Some(sealed)) => {
                    client.rename(child.id, &sealed).await.map_err(failed)?;
                    pending.push(child.id);
                }
                (NodeKind::Folder, None) => pending.push(child.id),
                (NodeKind::File, Some(sealed)) => {
                    if !names.contains(&(NodeKind::File, sealed.clone())) {
                        report(format!("Encrypting “{}”…", child.name), false);
                        let plain = download_bytes_within(child.id, child.size).await?;
                        let recipient = vault.recipient(root).ok_or(LOCKED_MEANWHILE)?;
                        let file = encrypted_file(&plain, &recipient, &sealed)?;
                        let ignore: upload::OnProgress = Rc::new(|_| {});
                        upload::upload(&client, &file, Some(folder), &ignore)
                            .await
                            .map_err(failed)?;
                    }
                    client.delete(child.id).await.map_err(failed)?;
                }
                (NodeKind::File, None) => {}
            }
        }
    }
    Ok(())
}

/// Replaces every sealed file and name below `root` with its plain form, the reverse of
/// [`encrypt_content`] and as safe to run again: a name that does not open is done; an encrypted
/// file whose plain copy is there already was uploaded and only needs deleting, for good, as
/// the folder is still protected.
async fn decrypt_content(
    vault: Vault,
    root: NodeId,
    report: &impl Fn(String, bool),
) -> Result<(), String> {
    let client = session::client();
    let failed = |err: ApiError| err.to_string();
    let mut pending = vec![root];
    while let Some(folder) = pending.pop() {
        let children = client.list(Some(folder)).await.map_err(failed)?;
        let names = taken(&children);
        for child in children {
            let opened = vault
                .with_keys(root, |keys| open_name(keys, &child))
                .ok_or(LOCKED_MEANWHILE)?;
            match (child.kind, opened) {
                (NodeKind::Folder, Some(name)) => {
                    client.rename(child.id, &name).await.map_err(failed)?;
                    pending.push(child.id);
                }
                (NodeKind::Folder, None) => pending.push(child.id),
                (NodeKind::File, Some(name)) => {
                    if !names.contains(&(NodeKind::File, name.clone())) {
                        report(format!("Decrypting “{name}”…"), false);
                        let plain = download_plain(vault, root, child.id, child.size).await?;
                        let file = plain_file(&plain, &name)?;
                        let ignore: upload::OnProgress = Rc::new(|_| {});
                        upload::upload(&client, &file, Some(folder), &ignore)
                            .await
                            .map_err(failed)?;
                    }
                    client.delete(child.id).await.map_err(failed)?;
                }
                (NodeKind::File, None) => {}
            }
        }
    }
    Ok(())
}

/// The names in a listing, by kind: the server allows a file and a folder of the same name.
fn taken(children: &[Node]) -> HashSet<(NodeKind, String)> {
    children
        .iter()
        .map(|child| (child.kind, child.name.clone()))
        .collect()
}

/// `plain` as a file named `name`, without a type, so the server takes it from the name.
fn plain_file(plain: &[u8], name: &str) -> Result<File, String> {
    let parts = Array::of1(&Uint8Array::from(plain));
    File::new_with_u8_array_sequence(&parts, name).map_err(|_| READ_FAILED.to_owned())
}

/// Stores `plain` encrypted as the new content of `node`, a file of the protected folder
/// `folder` as a listing shows it (its name opened); the stored file, shown the same way.
pub async fn replace_plain(
    vault: Vault,
    folder: NodeId,
    node: &Node,
    plain: &[u8],
) -> Result<Node, String> {
    let parent = node.parent_id.ok_or(LOCKED_MEANWHILE)?;
    let sealed = vault
        .with_keys(folder, |keys| {
            seal_name(keys, parent, node.kind, &node.name)
        })
        .ok_or(LOCKED_MEANWHILE)?
        .map_err(|err| err.to_string())?;
    let recipient = vault.recipient(folder).ok_or(LOCKED_MEANWHILE)?;
    let encrypted = content::encrypt_bytes(&recipient, plain).map_err(|err| err.to_string())?;
    let client = session::client();
    let stored = upload::replace_content(&client, node.id, &sealed, ENCRYPTED_MIME, &encrypted)
        .await
        .map_err(|err| err.to_string())?;
    vault
        .opened(folder, vec![stored])
        .and_then(|mut shown| shown.pop())
        .ok_or_else(|| LOCKED_MEANWHILE.to_owned())
}

/// The files in the recycle bin that were deleted from inside the folder at `path`, such as
/// `/Photos/2024`: they are still plain, and outside the folder once it is protected.
pub async fn binned_from(path: &str) -> Result<Vec<TrashItem>, String> {
    let inside = format!("{path}/");
    let binned = session::client()
        .trash()
        .await
        .map_err(|err| err.to_string())?;
    Ok(binned
        .into_iter()
        .filter(|item| item.original_path.starts_with(&inside))
        .collect())
}

async fn download_bytes_within(id: NodeId, size: i64) -> Result<Vec<u8>, String> {
    if size as f64 > MAX_IN_MEMORY {
        return Err(TOO_LARGE.to_owned());
    }
    download_bytes(id).await
}
