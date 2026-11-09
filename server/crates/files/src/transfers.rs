//! Files and folders sent to a device.
//!
//! A transfer holds its own copy of the bytes under `<root>/transfers/<id>`: files from the
//! sender's tree are linked in when the transfer is made, the others arrive through resumable
//! uploads. The copy is removed once the recipient has downloaded, saved or declined it, or the
//! sender cancels it; the row stays until it expires, so both sides can see what happened.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use time::OffsetDateTime;
use tokio::fs;
use uuid::Uuid;

use db::{
    ExpiredTransfer, NewTransfer, Node, NodeId, NodeKind, StoreOptions, StoreOutcome, Transfer,
    TransferItem, UserId,
};
use protocol::{CreateTransfer, TransferState};

use crate::{FileService, FilesError, service::new_content};

/// How long a transfer is kept when nothing else is configured.
pub const DEFAULT_TRANSFER_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// The most items one transfer may hold.
pub const MAX_TRANSFER_ITEMS: usize = 10_000;
const MAX_PATH_LEN: usize = 4096;
const MAX_NAME_CHARS: usize = 255;
const MAX_MESSAGE_CHARS: usize = 1000;
/// How long a transfer directory without a row is left alone, so a transfer still being made
/// is never swept.
const SWEEP_AFTER: Duration = Duration::from_secs(60 * 60);
/// How often a file of the sender's tree is linked again when it changes while being linked.
const LINK_ATTEMPTS: usize = 3;
/// How many numbered names are tried for a saved folder whose name is taken.
const MAX_NUMBERED_NAMES: usize = 1000;

/// A transfer saved into the recipient's tree.
#[derive(Debug)]
pub struct SavedTransfer {
    pub transfer: Transfer,
    /// The folders that were created, parents first.
    pub folders: Vec<Node>,
    /// What storing each file did.
    pub files: Vec<StoreOutcome>,
}

impl FileService {
    /// How long transfers are kept.
    pub fn transfer_ttl(&self) -> Duration {
        self.transfer_ttl
    }

    /// Sets how long transfers are kept and how many bytes one may hold (`None`: no limit).
    #[must_use]
    pub fn with_transfers(mut self, ttl: Duration, max_bytes: Option<u64>) -> Self {
        self.transfer_ttl = ttl;
        self.max_transfer_bytes = max_bytes;
        self
    }

    /// Makes a transfer from `sender` (on `device`, when named). Files of the sender's tree
    /// are copied at once; the others wait for their uploads.
    pub async fn create_transfer(
        &self,
        sender: UserId,
        device: Option<Uuid>,
        request: CreateTransfer,
    ) -> Result<(Transfer, Vec<TransferItem>), FilesError> {
        if !self
            .db
            .can_send_to(sender, request.recipient_id, request.recipient_device)
            .await?
        {
            return Err(FilesError::Forbidden);
        }
        let message = request
            .message
            .map(|message| message.trim().to_owned())
            .filter(|message| !message.is_empty());
        if message
            .as_ref()
            .is_some_and(|message| message.chars().count() > MAX_MESSAGE_CHARS)
        {
            return Err(invalid(format!(
                "the message is longer than {MAX_MESSAGE_CHARS} characters"
            )));
        }
        let id = Uuid::now_v7();
        let mut items = self.manifest(sender, id, request.items).await?;
        check_manifest(&items)?;
        let total = items
            .iter()
            .filter(|item| item.kind == NodeKind::File)
            .map(|item| u64::try_from(item.size).unwrap_or(0))
            .sum::<u64>();
        if let Some(limit) = self.max_transfer_bytes
            && total > limit
        {
            return Err(FilesError::TooLarge { limit });
        }
        if let Some(item) = items.iter().find(|item| {
            item.node_id.is_none()
                && u64::try_from(item.size).unwrap_or(u64::MAX) > self.max_upload_bytes
        }) {
            tracing::debug!(path = %item.path, "transfer file too large");
            return Err(FilesError::TooLarge {
                limit: self.max_upload_bytes,
            });
        }
        if let Err(err) = self.link_items(sender, id, &mut items).await {
            self.remove_transfer_files(id).await;
            return Err(err);
        }
        let ttl = time::Duration::try_from(self.transfer_ttl).unwrap_or(time::Duration::DAY);
        let new = NewTransfer {
            id,
            sender,
            sender_device: device,
            recipient: request.recipient_id,
            recipient_device: request.recipient_device,
            message,
            expires_at: OffsetDateTime::now_utc() + ttl,
            items,
        };
        let items = new.items.clone();
        match self.db.create_transfer(new).await {
            Ok(transfer) => Ok((transfer, items)),
            Err(err) => {
                self.remove_transfer_files(id).await;
                Err(match err {
                    db::DbError::Invalid => invalid("the transfer is not valid".to_owned()),
                    other => other.into(),
                })
            }
        }
    }

    /// A transfer `caller` sent or receives, with its items.
    pub async fn transfer(
        &self,
        caller: UserId,
        id: Uuid,
    ) -> Result<(Transfer, Vec<TransferItem>), FilesError> {
        let transfer = self.live_transfer(caller, id).await?;
        let items = self.db.transfer_items(id).await?;
        Ok((transfer, items))
    }

    /// The transfers waiting for `recipient`, only those sent to `device` when given.
    pub async fn incoming_transfers(
        &self,
        recipient: UserId,
        device: Option<Uuid>,
    ) -> Result<Vec<Transfer>, FilesError> {
        let now = OffsetDateTime::now_utc();
        let mut transfers = self.db.incoming_transfers(recipient, device).await?;
        transfers.retain(|transfer| transfer.expires_at > now);
        Ok(transfers)
    }

    /// The transfers `sender` sent.
    pub async fn outgoing_transfers(&self, sender: UserId) -> Result<Vec<Transfer>, FilesError> {
        let now = OffsetDateTime::now_utc();
        let mut transfers = self.db.outgoing_transfers(sender).await?;
        transfers.retain(|transfer| transfer.expires_at > now);
        Ok(transfers)
    }

    /// Opens a file of a ready transfer for its recipient.
    pub async fn open_transfer_item(
        &self,
        caller: UserId,
        id: Uuid,
        item: Uuid,
    ) -> Result<(TransferItem, fs::File), FilesError> {
        let transfer = self.received_transfer(caller, id).await?;
        require_ready(&transfer)?;
        let item = self
            .db
            .transfer_items(id)
            .await?
            .into_iter()
            .find(|candidate| candidate.id == item)
            .ok_or(FilesError::NotFound)?;
        if item.kind != NodeKind::File {
            return Err(FilesError::NotAFile);
        }
        let file = self.store.open_transfer_item(id, item.id).await?;
        Ok((item, file))
    }

    /// The recipient confirms a download; the bytes are removed.
    pub async fn confirm_transfer(&self, caller: UserId, id: Uuid) -> Result<Transfer, FilesError> {
        let transfer = self.received_transfer(caller, id).await?;
        require_ready(&transfer)?;
        self.finish(transfer, &[TransferState::Ready], TransferState::Downloaded)
            .await
    }

    /// The recipient declines a transfer; the bytes are removed.
    pub async fn decline_transfer(&self, caller: UserId, id: Uuid) -> Result<Transfer, FilesError> {
        let transfer = self.received_transfer(caller, id).await?;
        self.finish(
            transfer,
            &[TransferState::Uploading, TransferState::Ready],
            TransferState::Declined,
        )
        .await
    }

    /// The sender cancels a transfer; the bytes and unfinished uploads are removed.
    pub async fn cancel_transfer(&self, caller: UserId, id: Uuid) -> Result<Transfer, FilesError> {
        let transfer = self.live_transfer(caller, id).await?;
        if transfer.sender_id != caller {
            return Err(FilesError::Forbidden);
        }
        self.finish(
            transfer,
            &[TransferState::Uploading, TransferState::Ready],
            TransferState::Cancelled,
        )
        .await
    }

    /// Moves the files of a ready transfer into the recipient's tree under `parent` (the root
    /// when `None`), keeping its folders. A name that is taken at the top gets a number.
    pub async fn save_transfer(
        &self,
        caller: UserId,
        id: Uuid,
        parent: Option<NodeId>,
    ) -> Result<SavedTransfer, FilesError> {
        let mut transfer = self.received_transfer(caller, id).await?;
        require_ready(&transfer)?;
        if let Some(parent) = parent
            && self.db.get_node(caller, parent).await?.kind != NodeKind::Folder
        {
            return Err(FilesError::NotAFile);
        }
        let mut items = self.db.transfer_items(id).await?;
        items.sort_by(|a, b| {
            depth(&a.path)
                .cmp(&depth(&b.path))
                .then(a.path.cmp(&b.path))
        });
        if !self
            .db
            .finish_transfer(id, &[TransferState::Ready], TransferState::Saved)
            .await?
        {
            return Err(FilesError::Gone);
        }
        transfer.state = TransferState::Saved;
        let mut saved = SavedTransfer {
            transfer,
            folders: Vec::new(),
            files: Vec::new(),
        };
        let mut placed = HashMap::<String, NodeId>::new();
        for item in &items {
            let (dir, name) = split_path(&item.path);
            let into = match dir {
                Some(dir) => Some(*placed.get(dir).ok_or(FilesError::NotFound)?),
                None => parent,
            };
            let stored = match item.kind {
                NodeKind::Folder => self.save_folder(caller, into, name).await.map(|folder| {
                    placed.insert(item.path.clone(), folder.id);
                    saved.folders.push(folder);
                }),
                NodeKind::File => self
                    .save_file(caller, &saved.transfer, item, into, name)
                    .await
                    .map(|outcome| saved.files.push(outcome)),
            };
            if let Err(err) = stored {
                if saved.folders.is_empty() && saved.files.is_empty() {
                    if let Err(err) = self.db.reopen_transfer(id).await {
                        tracing::warn!(transfer_id = %id, error = %err, "failed to reopen transfer");
                    }
                } else {
                    tracing::warn!(transfer_id = %id, error = %err, "transfer saved in part");
                }
                return Err(err);
            }
        }
        self.remove_transfer_files(id).await;
        Ok(saved)
    }

    /// Removes the transfers whose time ran out, their bytes and their unfinished uploads,
    /// and transfer leftovers on disk.
    pub async fn expire_transfers(&self) -> Result<Vec<ExpiredTransfer>, FilesError> {
        let (expired, uploads) = self.db.expire_transfers(OffsetDateTime::now_utc()).await?;
        for id in uploads {
            self.uploads.remove(id);
            if let Err(err) = self.store.remove_partial(id).await {
                tracing::warn!(upload_id = %id, error = %err, "failed to remove transfer upload");
            }
        }
        for transfer in &expired {
            self.remove_transfer_files(transfer.id).await;
        }
        let live = self
            .db
            .transfer_ids()
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        self.store.sweep_transfers(&live, SWEEP_AFTER).await?;
        Ok(expired)
    }

    pub(crate) async fn remove_transfer_files(&self, id: Uuid) {
        if let Err(err) = self.store.remove_transfer(id).await {
            tracing::warn!(transfer_id = %id, error = %err, "failed to remove transfer files");
        }
    }

    /// Turns the requested items into the transfer's items, expanding folders of the
    /// sender's tree into everything in them.
    async fn manifest(
        &self,
        sender: UserId,
        transfer: Uuid,
        requested: Vec<protocol::NewTransferItem>,
    ) -> Result<Vec<TransferItem>, FilesError> {
        if requested.is_empty() {
            return Err(invalid("a transfer needs at least one item".to_owned()));
        }
        let mut items = Vec::with_capacity(requested.len());
        for request in requested {
            if !valid_path(&request.path) {
                return Err(invalid(format!("invalid path: {}", request.path)));
            }
            match request.node_id {
                Some(node) => {
                    for entry in self.db.subtree(sender, node).await? {
                        let node = entry.node;
                        let is_file = node.kind == NodeKind::File;
                        items.push(TransferItem {
                            id: Uuid::now_v7(),
                            transfer_id: transfer,
                            path: format!("{}{}", request.path, entry.rel),
                            kind: node.kind,
                            size: if is_file { node.size } else { 0 },
                            sha256: None,
                            mime: is_file.then_some(node.mime),
                            modified_at: Some(node.updated_at),
                            node_id: Some(node.id),
                        });
                    }
                }
                None => items.push(TransferItem {
                    id: Uuid::now_v7(),
                    transfer_id: transfer,
                    path: request.path,
                    kind: request.kind,
                    size: match request.kind {
                        NodeKind::File => i64::try_from(request.size).unwrap_or(i64::MAX),
                        NodeKind::Folder => 0,
                    },
                    sha256: None,
                    mime: request.mime.filter(|_| request.kind == NodeKind::File),
                    modified_at: request.modified_at,
                    node_id: None,
                }),
            }
            if items.len() > MAX_TRANSFER_ITEMS {
                return Err(invalid(format!(
                    "a transfer holds at most {MAX_TRANSFER_ITEMS} items"
                )));
            }
        }
        Ok(items)
    }

    /// Links the files taken from the sender's tree into the transfer and records their
    /// hashes. A file replaced while it is linked is linked again.
    async fn link_items(
        &self,
        sender: UserId,
        transfer: Uuid,
        items: &mut [TransferItem],
    ) -> Result<(), FilesError> {
        for item in items.iter_mut() {
            let Some(node) = item.node_id.filter(|_| item.kind == NodeKind::File) else {
                continue;
            };
            let mut linked = false;
            for _ in 0..LINK_ATTEMPTS {
                let before = self.db.get_node(sender, node).await?;
                self.store
                    .link_into_transfer(sender, node, transfer, item.id)
                    .await?;
                let after = self.db.get_node(sender, node).await?;
                if before.version == after.version && before.sha256 == after.sha256 {
                    item.size = after.size;
                    item.sha256 = Some(after.sha256.unwrap_or_default());
                    item.mime = Some(after.mime);
                    linked = true;
                    break;
                }
            }
            if !linked {
                return Err(FilesError::Conflict);
            }
        }
        Ok(())
    }

    /// A transfer `caller` can see that has not expired.
    async fn live_transfer(&self, caller: UserId, id: Uuid) -> Result<Transfer, FilesError> {
        let transfer = self.db.transfer(caller, id).await?;
        if transfer.expires_at <= OffsetDateTime::now_utc() {
            return Err(FilesError::Gone);
        }
        Ok(transfer)
    }

    /// A transfer sent to `caller` that has not expired.
    async fn received_transfer(&self, caller: UserId, id: Uuid) -> Result<Transfer, FilesError> {
        let transfer = self.live_transfer(caller, id).await?;
        if transfer.recipient_id != caller {
            return Err(FilesError::Forbidden);
        }
        Ok(transfer)
    }

    async fn finish(
        &self,
        mut transfer: Transfer,
        from: &[TransferState],
        to: TransferState,
    ) -> Result<Transfer, FilesError> {
        if !self.db.finish_transfer(transfer.id, from, to).await? {
            return Err(FilesError::Gone);
        }
        transfer.state = to;
        for upload in self.db.transfer_uploads(transfer.id).await? {
            self.abandon_upload(upload).await;
        }
        self.remove_transfer_files(transfer.id).await;
        Ok(transfer)
    }

    async fn save_folder(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
        name: &str,
    ) -> Result<Node, FilesError> {
        for n in 0..MAX_NUMBERED_NAMES {
            let candidate = numbered(name, n);
            match self.create_folder(owner, parent, &candidate).await {
                Err(FilesError::Conflict) => {}
                other => return other,
            }
        }
        Err(FilesError::Conflict)
    }

    async fn save_file(
        &self,
        owner: UserId,
        transfer: &Transfer,
        item: &TransferItem,
        parent: Option<NodeId>,
        name: &str,
    ) -> Result<StoreOutcome, FilesError> {
        let staged = self.store.adopt_transfer_item(
            transfer.id,
            item.id,
            u64::try_from(item.size).unwrap_or(0),
            item.sha256.clone().unwrap_or_default(),
        );
        let content = new_content(&staged, item.mime.as_deref(), name, item.modified_at);
        let options = StoreOptions {
            keep_both: true,
            ..StoreOptions::default()
        };
        let stored = self
            .db
            .store_file_with(owner, parent, name, content, options)
            .await;
        self.finish_store(owner, staged, stored).await
    }
}

fn invalid(message: String) -> FilesError {
    FilesError::InvalidTransfer(message)
}

fn require_ready(transfer: &Transfer) -> Result<(), FilesError> {
    match transfer.state {
        TransferState::Ready => Ok(()),
        TransferState::Uploading => Err(FilesError::NotReady),
        _ => Err(FilesError::Gone),
    }
}

/// Checks that paths are unique and every folder in a path is an item of the transfer.
fn check_manifest(items: &[TransferItem]) -> Result<(), FilesError> {
    let mut kinds = HashMap::with_capacity(items.len());
    for item in items {
        if !valid_path(&item.path) {
            return Err(invalid(format!("invalid path: {}", item.path)));
        }
        if kinds.insert(item.path.as_str(), item.kind).is_some() {
            return Err(invalid(format!("the path {} is listed twice", item.path)));
        }
    }
    for item in items {
        if let (Some(dir), _) = split_path(&item.path)
            && kinds.get(dir) != Some(&NodeKind::Folder)
        {
            return Err(invalid(format!(
                "the folder {dir} of {} is not an item of the transfer",
                item.path
            )));
        }
    }
    Ok(())
}

/// Whether `path` is relative, `/`-separated and every part of it is a valid node name.
pub(crate) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_LEN
        && !path.contains('\\')
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.chars().count() <= MAX_NAME_CHARS
                && !part.chars().any(char::is_control)
        })
}

/// The folder part and the name of a path.
fn split_path(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (Some(dir), name),
        None => (None, path),
    }
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

/// The name of a folder, or `name (n)` for n > 0.
fn numbered(name: &str, n: usize) -> String {
    if n == 0 {
        name.to_owned()
    } else {
        format!("{name} ({n})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_must_be_relative_and_clean() {
        for good in ["a.txt", "Photos/2026/a.jpg", "a b/c"] {
            assert!(valid_path(good), "{good}");
        }
        for bad in [
            "", "/a", "a/", "a//b", "../a", "a/../b", "./a", "a\\b", "a/\u{0}b",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(!valid_path(&"a".repeat(256)));
    }

    fn item(path: &str, kind: NodeKind) -> TransferItem {
        TransferItem {
            id: Uuid::nil(),
            transfer_id: Uuid::nil(),
            path: path.to_owned(),
            kind,
            size: 0,
            sha256: None,
            mime: None,
            modified_at: None,
            node_id: None,
        }
    }

    #[test]
    fn every_folder_of_a_path_must_be_an_item() {
        let folder = item("Photos", NodeKind::Folder);
        let photo = item("Photos/a.jpg", NodeKind::File);
        assert!(check_manifest(&[folder.clone(), photo.clone()]).is_ok());
        assert!(check_manifest(std::slice::from_ref(&photo)).is_err());
        assert!(check_manifest(&[item("Photos", NodeKind::File), photo]).is_err());
        assert!(check_manifest(&[folder.clone(), folder]).is_err());
    }

    #[test]
    fn numbered_names_follow_the_tree_convention() {
        assert_eq!(numbered("Photos", 0), "Photos");
        assert_eq!(numbered("Photos", 2), "Photos (2)");
    }
}
