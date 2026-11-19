//! The recycle bin: deleting into it, restoring from it, and emptying it.

use uuid::Uuid;

use db::{MoveBatch, NodeId, OnConflict, TrashOutcome, TrashedNode, UserId};
use events::DomainEvent;

use crate::{Actor, FilesError, content::ContentStore, emitted};

/// A user's recycle bin.
#[derive(Clone, Debug)]
pub struct TrashService {
    pub(crate) content: ContentStore,
}

impl TrashService {
    pub(crate) fn new(content: ContentStore) -> Self {
        Self { content }
    }

    /// The same service, recording `device` as the maker of the changes made through it.
    pub(crate) fn acting(&self, device: Option<Uuid>) -> Self {
        Self {
            content: self.content.acting(device),
        }
    }

    /// Deletes a node by moving its files to the recycle bin; their content stays stored.
    pub async fn delete(&self, actor: Actor, id: NodeId) -> Result<TrashOutcome, FilesError> {
        self.acting(actor.device).delete_as(actor.user, id).await
    }

    /// Lists the files in the recycle bin, most recently deleted first.
    pub async fn trash(&self, owner: UserId) -> Result<Vec<TrashedNode>, FilesError> {
        Ok(self.content.db.list_trash(owner).await?)
    }

    /// Moves binned files to Lost & Found; see [`Db::restore`]. Clients never see the bin
    /// folder, so files still in it report no parent.
    pub async fn restore(
        &self,
        actor: Actor,
        ids: &[NodeId],
        on_conflict: OnConflict,
    ) -> Result<MoveBatch, FilesError> {
        self.acting(actor.device)
            .restore_as(actor.user, ids, on_conflict)
            .await
    }

    /// Permanently deletes every binned file and its stored content.
    pub async fn empty_trash(&self, actor: Actor) -> Result<(), FilesError> {
        self.acting(actor.device).empty_trash_as(actor.user).await
    }

    /// Permanently deletes one binned file and its stored content.
    pub async fn purge_trashed(&self, actor: Actor, id: NodeId) -> Result<(), FilesError> {
        self.acting(actor.device)
            .purge_trashed_as(actor.user, id)
            .await
    }

    async fn delete_as(&self, owner: UserId, id: NodeId) -> Result<TrashOutcome, FilesError> {
        let outcome = self.content.db.trash_node(owner, id).await?;
        self.content
            .events
            .publish(vec![
                DomainEvent::NodeDeleted(outcome.node.clone()),
                DomainEvent::TrashChanged { owner },
            ])
            .await;
        Ok(outcome)
    }

    async fn restore_as(
        &self,
        owner: UserId,
        ids: &[NodeId],
        on_conflict: OnConflict,
    ) -> Result<MoveBatch, FilesError> {
        let mut batch = self.content.db.restore(owner, ids, on_conflict).await?;
        self.content.apply_blob_ops(owner, &batch.blob_ops).await;
        let bin = self.content.db.special_folders(owner).await?.trash_id;
        let binned = batch
            .deleted
            .iter_mut()
            .chain(
                batch
                    .conflicts
                    .iter_mut()
                    .map(|conflict| &mut conflict.source),
            )
            .filter(|node| node.parent_id == Some(bin));
        for node in binned {
            node.parent_id = None;
        }
        let mut events = emitted::moved_batch(owner, &batch, !batch.moved.is_empty());
        events.extend(emitted::restored_content(&batch, owner));
        self.content.events.publish(events).await;
        Ok(batch)
    }

    async fn empty_trash_as(&self, owner: UserId) -> Result<(), FilesError> {
        let deleted = self.content.db.empty_trash(owner).await?;
        self.content.remove_content(owner, deleted).await;
        self.content
            .events
            .publish_one(DomainEvent::TrashChanged { owner })
            .await;
        Ok(())
    }

    async fn purge_trashed_as(&self, owner: UserId, id: NodeId) -> Result<(), FilesError> {
        let deleted = self.content.db.purge_trashed(owner, id).await?;
        self.content.remove_content(owner, deleted).await;
        self.content
            .events
            .publish_one(DomainEvent::TrashChanged { owner })
            .await;
        Ok(())
    }
}
