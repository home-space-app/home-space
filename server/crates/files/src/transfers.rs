//! Files and folders sent to a device.
//!
//! A transfer holds its own copy of the bytes under `<root>/transfers/<id>`: files from the
//! sender's tree are linked in when the transfer is made, the others arrive through resumable
//! uploads. The copy is removed once the recipient has downloaded, saved or declined it, or the
//! sender cancels it; the row stays until it expires, so both sides can see what happened.

use std::{collections::HashSet, path::PathBuf, time::Duration};

use time::OffsetDateTime;
use tokio::fs;
use uuid::Uuid;

use db::{
    ExpiredTransfer, IncomingScope, NewTransfer, Node, NodeKind, Transfer, TransferItem, UserId,
};
use protocol::{CreateTransfer, TransferState};

use events::DomainEvent;

use crate::{FilesError, StoreOutcome, UploadService, content::ContentStore};

mod manifest;
mod save;

use manifest::{check_manifest, invalid};

/// How long a transfer is kept when nothing else is configured.
pub const DEFAULT_TRANSFER_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// The most items one transfer may hold.
pub const MAX_TRANSFER_ITEMS: usize = 10_000;
const MAX_MESSAGE_CHARS: usize = 1000;
/// How long a transfer directory without a row is left alone, so a transfer still being made
/// is never swept.
const SWEEP_AFTER: Duration = Duration::from_secs(60 * 60);

/// A transfer saved into the recipient's tree.
#[derive(Debug)]
pub struct SavedTransfer {
    pub transfer: Transfer,
    /// The folders that were created, parents first.
    pub folders: Vec<Node>,
    /// What storing each file did.
    pub files: Vec<StoreOutcome>,
}

/// Files and folders sent to a device.
#[derive(Clone, Debug)]
pub struct TransferService {
    pub(crate) content: ContentStore,
    /// Ends the uploads of a transfer that is over.
    uploads: UploadService,
    transfer_ttl: Duration,
    max_transfer_bytes: Option<u64>,
}

impl TransferService {
    pub(crate) fn new(
        content: ContentStore,
        uploads: UploadService,
        transfer_ttl: Duration,
        max_transfer_bytes: Option<u64>,
    ) -> Self {
        Self {
            content,
            uploads,
            transfer_ttl,
            max_transfer_bytes,
        }
    }

    /// The same service, recording `device` as the maker of the changes made through it.
    pub(crate) fn acting(&self, device: Option<Uuid>) -> Self {
        Self {
            content: self.content.acting(device),
            uploads: self.uploads.clone(),
            transfer_ttl: self.transfer_ttl,
            max_transfer_bytes: self.max_transfer_bytes,
        }
    }

    /// How long transfers are kept.
    pub fn transfer_ttl(&self) -> Duration {
        self.transfer_ttl
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
            .content
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
                && u64::try_from(item.size).unwrap_or(u64::MAX) > self.content.max_upload_bytes
        }) {
            tracing::debug!(path = %item.path, "transfer file too large");
            return Err(FilesError::TooLarge {
                limit: self.content.max_upload_bytes,
            });
        }
        if let Err(err) = self.link_items(sender, id, &mut items).await {
            self.content.remove_transfer_files(id).await;
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
        match self.content.db.create_transfer(new).await {
            Ok(transfer) => {
                if transfer.state == TransferState::Ready {
                    self.content
                        .events
                        .publish_one(DomainEvent::TransferReady(transfer.clone()))
                        .await;
                }
                Ok((transfer, items))
            }
            Err(err) => {
                self.content.remove_transfer_files(id).await;
                Err(match err {
                    db::DbError::Invalid => invalid("the transfer is not valid".to_owned()),
                    // The sender or the recipient is disabled or marked for deletion.
                    db::DbError::NotFound => FilesError::Forbidden,
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
        let items = self.content.db.transfer_items(id).await?;
        Ok((transfer, items))
    }

    /// The transfers waiting for `recipient`, within `scope`.
    pub async fn incoming_transfers(
        &self,
        recipient: UserId,
        scope: IncomingScope,
    ) -> Result<Vec<Transfer>, FilesError> {
        let now = OffsetDateTime::now_utc();
        let mut transfers = self.content.db.incoming_transfers(recipient, scope).await?;
        transfers.retain(|transfer| transfer.expires_at > now);
        Ok(transfers)
    }

    /// The transfers `sender` sent from `device`; see [`db::Db::outgoing_transfers`].
    pub async fn outgoing_transfers(
        &self,
        sender: UserId,
        device: Option<Uuid>,
    ) -> Result<Vec<Transfer>, FilesError> {
        let now = OffsetDateTime::now_utc();
        let mut transfers = self.content.db.outgoing_transfers(sender, device).await?;
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
            .content
            .db
            .transfer_items(id)
            .await?
            .into_iter()
            .find(|candidate| candidate.id == item)
            .ok_or(FilesError::NotFound)?;
        if item.kind != NodeKind::File {
            return Err(FilesError::NotAFile);
        }
        let file = self.content.store.open_transfer_item(id, item.id).await?;
        Ok((item, file))
    }

    /// The items of a ready transfer for its recipient, each file with where its bytes are, to
    /// be sent as one archive.
    pub async fn transfer_archive(
        &self,
        caller: UserId,
        id: Uuid,
    ) -> Result<(Transfer, Vec<(TransferItem, PathBuf)>), FilesError> {
        let transfer = self.received_transfer(caller, id).await?;
        require_ready(&transfer)?;
        let items = self
            .content
            .db
            .transfer_items(id)
            .await?
            .into_iter()
            .map(|item| {
                let path = self.content.store.transfer_item_path(id, item.id);
                (item, path)
            })
            .collect();
        Ok((transfer, items))
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

    /// Removes the transfers whose time ran out, their bytes and their unfinished uploads,
    /// and transfer leftovers on disk.
    pub async fn expire_transfers(&self) -> Result<Vec<ExpiredTransfer>, FilesError> {
        let (expired, uploads) = self
            .content
            .db
            .expire_transfers(OffsetDateTime::now_utc())
            .await?;
        for id in uploads {
            self.uploads.forget_slot(id);
            if let Err(err) = self.content.store.remove_partial(id).await {
                tracing::warn!(upload_id = %id, error = %err, "failed to remove transfer upload");
            }
        }
        for transfer in &expired {
            self.content.remove_transfer_files(transfer.id).await;
            self.content
                .events
                .publish_one(DomainEvent::TransferChanged {
                    id: transfer.id,
                    sender: transfer.sender_id,
                    recipient: transfer.recipient_id,
                    state: TransferState::Expired,
                })
                .await;
        }
        let live = self
            .content
            .db
            .transfer_ids()
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        self.content
            .store
            .sweep_transfers(&live, SWEEP_AFTER)
            .await?;
        Ok(expired)
    }

    /// A transfer `caller` can see that has not expired.
    async fn live_transfer(&self, caller: UserId, id: Uuid) -> Result<Transfer, FilesError> {
        let transfer = self.content.db.transfer(caller, id).await?;
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
        if !self
            .content
            .db
            .finish_transfer(transfer.id, from, to)
            .await?
        {
            return Err(FilesError::Gone);
        }
        transfer.state = to;
        for upload in self.content.db.transfer_uploads(transfer.id).await? {
            self.uploads.abandon_upload(upload).await;
        }
        self.content.remove_transfer_files(transfer.id).await;
        self.content
            .events
            .publish_one(DomainEvent::transfer_changed(&transfer))
            .await;
        Ok(transfer)
    }
}

fn require_ready(transfer: &Transfer) -> Result<(), FilesError> {
    match transfer.state {
        TransferState::Ready => Ok(()),
        TransferState::Uploading => Err(FilesError::NotReady),
        _ => Err(FilesError::Gone),
    }
}
