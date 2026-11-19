//! Saving a transfer into the recipient's tree.

use std::collections::HashMap;

use uuid::Uuid;

use db::{NodeId, NodeKind, Transfer, TransferItem, UserId};
use events::DomainEvent;
use protocol::TransferState;

use super::{SavedTransfer, TransferService, manifest::split_path, require_ready};
use crate::{Actor, FilesError, StoreOptions, StoreOutcome, content::new_content, emitted, tree};

impl TransferService {
    /// Moves the files of a ready transfer into the recipient's tree under `parent` (the root
    /// when `None`), keeping its folders. A name that is taken at the top gets a number.
    pub async fn save_transfer(
        &self,
        actor: Actor,
        id: Uuid,
        parent: Option<NodeId>,
    ) -> Result<SavedTransfer, FilesError> {
        self.acting(actor.device)
            .save_transfer_as(actor.user, id, parent)
            .await
    }

    async fn save_file(
        &self,
        owner: UserId,
        transfer: &Transfer,
        item: &TransferItem,
        parent: Option<NodeId>,
        name: &str,
    ) -> Result<StoreOutcome, FilesError> {
        let staged = self.content.store.adopt_transfer_item(
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
        let stored =
            tree::store_file(&self.content.db, owner, parent, name, content, options).await;
        self.content.finish_store(owner, staged, stored).await
    }

    async fn save_transfer_as(
        &self,
        caller: UserId,
        id: Uuid,
        parent: Option<NodeId>,
    ) -> Result<SavedTransfer, FilesError> {
        let (transfer, items) = self.claim_for_saving(caller, id, parent).await?;
        let mut saved = SavedTransfer {
            transfer,
            folders: Vec::new(),
            files: Vec::new(),
        };
        if let Err(err) = self.place_items(caller, &mut saved, &items, parent).await {
            self.after_failed_save(&saved, &err).await;
            return Err(err);
        }
        self.content.remove_transfer_files(id).await;
        let mut events = vec![DomainEvent::transfer_changed(&saved.transfer)];
        events.extend(emitted::saved(&saved.folders, &saved.files));
        self.content.events.publish(events).await;
        Ok(saved)
    }

    /// Makes a transfer that failed before anything was saved ready to save again; one saved in
    /// part stays saved, with what it placed.
    async fn after_failed_save(&self, saved: &SavedTransfer, err: &FilesError) {
        let id = saved.transfer.id;
        if saved.folders.is_empty() && saved.files.is_empty() {
            if let Err(err) = self.content.db.reopen_transfer(id).await {
                tracing::warn!(transfer_id = %id, error = %err, "failed to reopen transfer");
            }
        } else {
            tracing::warn!(transfer_id = %id, error = %err, "transfer saved in part");
        }
    }

    /// Checks that `caller` can save ready transfer `id` into `parent` and marks it saved.
    /// Returns it with its items, each folder before what is in it.
    async fn claim_for_saving(
        &self,
        caller: UserId,
        id: Uuid,
        parent: Option<NodeId>,
    ) -> Result<(Transfer, Vec<TransferItem>), FilesError> {
        let mut transfer = self.received_transfer(caller, id).await?;
        require_ready(&transfer)?;
        if let Some(parent) = parent
            && self.content.db.get_node(caller, parent).await?.kind != NodeKind::Folder
        {
            return Err(FilesError::NotAFile);
        }
        let mut items = self.content.db.transfer_items(id).await?;
        let bytes = items
            .iter()
            .filter(|item| item.kind == NodeKind::File)
            .map(|item| u64::try_from(item.size).unwrap_or(0))
            .sum();
        // The transfer stays ready, so the recipient can free space and save it then.
        self.content.check_quota(caller, None, bytes).await?;
        items.sort_by(|a, b| {
            depth(&a.path)
                .cmp(&depth(&b.path))
                .then(a.path.cmp(&b.path))
        });
        if !self
            .content
            .db
            .finish_transfer(id, &[TransferState::Ready], TransferState::Saved)
            .await?
        {
            return Err(FilesError::Gone);
        }
        transfer.state = TransferState::Saved;
        Ok((transfer, items))
    }

    /// Saves `items` under `parent`, recording each in `saved`; stops at the first that fails.
    async fn place_items(
        &self,
        caller: UserId,
        saved: &mut SavedTransfer,
        items: &[TransferItem],
        parent: Option<NodeId>,
    ) -> Result<(), FilesError> {
        let mut placed = HashMap::<&str, NodeId>::new();
        for item in items {
            let (dir, name) = split_path(&item.path);
            let into = match dir {
                Some(dir) => Some(*placed.get(dir).ok_or(FilesError::NotFound)?),
                None => parent,
            };
            match item.kind {
                NodeKind::Folder => {
                    let folder = self.content.save_folder(caller, into, name).await?;
                    placed.insert(&item.path, folder.id);
                    saved.folders.push(folder);
                }
                NodeKind::File => {
                    let outcome = self
                        .save_file(caller, &saved.transfer, item, into, name)
                        .await?;
                    saved.files.push(outcome);
                }
            }
        }
        Ok(())
    }
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}
