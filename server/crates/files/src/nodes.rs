//! A user's folders and files: creating, storing, reading, renaming and moving them.

use bytes::Bytes;
use futures_util::Stream;
use time::OffsetDateTime;
use tokio::fs;
use uuid::Uuid;

use db::{ChangeBatch, ChangesSince, Node, NodeId, NodeKind, NodeProperties, OnConflict, UserId};
use events::DomainEvent;

use crate::{
    Actor, BodyError, FilesError, MoveBatch, MoveOutcome, NodeUpdate, StoreOptions, StoreOutcome,
    content::{ContentStore, new_content},
    emitted, tree,
};

/// A file to store with [`NodeService::upload`]: in `parent` (the root when `None`) as `name`.
/// A missing `content_type` is guessed from the name; `modified_at` is when the content last
/// changed, if the client knows.
#[derive(Clone, Copy, Debug)]
pub struct NewFile<'a> {
    pub parent: Option<NodeId>,
    pub name: &'a str,
    pub content_type: Option<&'a str>,
    pub modified_at: Option<OffsetDateTime>,
}

/// A user's folders and files.
#[derive(Clone, Debug)]
pub struct NodeService {
    pub(crate) content: ContentStore,
}

impl NodeService {
    pub(crate) fn new(content: ContentStore) -> Self {
        Self { content }
    }

    /// The same service, recording `device` as the maker of the changes made through it.
    pub(crate) fn acting(&self, device: Option<Uuid>) -> Self {
        Self {
            content: self.content.acting(device),
        }
    }

    /// What changed in `owner`'s tree after the journal position `since`, `limit` changes at
    /// most; [`FilesError::ChangesPruned`] once those changes are no longer kept.
    pub async fn changes(
        &self,
        owner: UserId,
        since: Option<i64>,
        limit: i64,
    ) -> Result<ChangeBatch, FilesError> {
        match self.content.db.changes(owner, since, limit).await? {
            ChangesSince::Kept(batch) => Ok(batch),
            ChangesSince::Pruned => Err(FilesError::ChangesPruned),
        }
    }

    /// The largest file a single upload may hold.
    pub fn max_upload_bytes(&self) -> u64 {
        self.content.max_upload_bytes
    }

    /// Creates a folder under `parent` (or at the root).
    pub async fn create_folder(
        &self,
        actor: Actor,
        parent: Option<NodeId>,
        name: &str,
    ) -> Result<Node, FilesError> {
        self.acting(actor.device)
            .create_folder_as(actor.user, parent, name)
            .await
    }

    /// Stores a file streamed from `body`.
    ///
    /// A new name creates the file, identical content under an existing name changes nothing,
    /// and different content becomes a new version of the existing file. Content last changed
    /// at `modified_at`, before the existing file, is kept as one of its versions instead.
    pub async fn upload<S>(
        &self,
        actor: Actor,
        file: NewFile<'_>,
        body: S,
    ) -> Result<StoreOutcome, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
    {
        self.acting(actor.device)
            .upload_as(actor.user, file, body)
            .await
    }

    /// Replaces the content of an existing file; different content becomes a new version, or
    /// an older one when it was last changed before the current content. A missing content type
    /// is guessed from the file's name.
    pub async fn replace_content<S>(
        &self,
        actor: Actor,
        id: NodeId,
        content_type: Option<&str>,
        modified_at: Option<OffsetDateTime>,
        body: S,
    ) -> Result<StoreOutcome, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
    {
        self.acting(actor.device)
            .replace_content_as(actor.user, id, content_type, modified_at, body)
            .await
    }

    /// Opens a file for download.
    pub async fn download(
        &self,
        owner: UserId,
        id: NodeId,
    ) -> Result<(Node, fs::File), FilesError> {
        let node = self.content.db.get_node(owner, id).await?;
        if node.kind == NodeKind::File {
            let file = self.content.store.open_blob(owner, id).await?;
            Ok((node, file))
        } else {
            Err(FilesError::NotAFile)
        }
    }

    /// Returns a node's predefined properties.
    pub async fn properties(
        &self,
        owner: UserId,
        id: NodeId,
    ) -> Result<NodeProperties, FilesError> {
        Ok(self.content.db.node_properties(owner, id).await?)
    }

    /// Returns a node's metadata.
    pub async fn get(&self, owner: UserId, id: NodeId) -> Result<Node, FilesError> {
        Ok(self.content.db.get_node(owner, id).await?)
    }

    /// Lists the ancestors of a node from the root down to its parent.
    pub async fn ancestors(&self, owner: UserId, id: NodeId) -> Result<Vec<Node>, FilesError> {
        Ok(self.content.db.list_ancestors(owner, id).await?)
    }

    /// Lists a folder (or the root when `parent` is `None`).
    pub async fn list(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
    ) -> Result<Vec<Node>, FilesError> {
        Ok(self.content.db.list_children(owner, parent).await?)
    }

    /// Renames and/or moves a node.
    pub async fn update(
        &self,
        actor: Actor,
        id: NodeId,
        update: NodeUpdate,
    ) -> Result<MoveOutcome, FilesError> {
        self.acting(actor.device)
            .update_as(actor.user, id, update)
            .await
    }

    /// Moves several nodes into `parent` (the root when `None`). Folders merge into
    /// same-named folders; `on_conflict` decides what happens to files whose name is taken.
    pub async fn move_nodes(
        &self,
        actor: Actor,
        ids: &[NodeId],
        parent: Option<NodeId>,
        on_conflict: OnConflict,
    ) -> Result<MoveBatch, FilesError> {
        self.acting(actor.device)
            .move_nodes_as(actor.user, ids, parent, on_conflict)
            .await
    }

    async fn create_folder_as(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
        name: &str,
    ) -> Result<Node, FilesError> {
        let folder = self.content.insert_folder(owner, parent, name).await?;
        self.content
            .events
            .publish_one(DomainEvent::NodeCreated(folder.clone()))
            .await;
        Ok(folder)
    }

    async fn upload_as<S>(
        &self,
        owner: UserId,
        file: NewFile<'_>,
        body: S,
    ) -> Result<StoreOutcome, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
    {
        let NewFile {
            parent,
            name,
            content_type,
            modified_at,
        } = file;
        if let Some(parent) = parent {
            self.content.db.get_node(owner, parent).await?;
        }
        let staged = self.content.stage_within_quota(owner, body).await?;
        let stored = tree::store_file(
            &self.content.db,
            owner,
            parent,
            name,
            new_content(&staged, content_type, name, modified_at),
            StoreOptions::default(),
        )
        .await;
        self.content
            .publish_stored(self.content.finish_store(owner, staged, stored).await)
            .await
    }

    async fn replace_content_as<S>(
        &self,
        owner: UserId,
        id: NodeId,
        content_type: Option<&str>,
        modified_at: Option<OffsetDateTime>,
        body: S,
    ) -> Result<StoreOutcome, FilesError>
    where
        S: Stream<Item = Result<Bytes, BodyError>>,
    {
        let current = self.content.db.get_node(owner, id).await?;
        if current.kind != NodeKind::File {
            return Err(FilesError::NotAFile);
        }
        let staged = self.content.stage_within_quota(owner, body).await?;
        let stored = tree::replace_content(
            &self.content.db,
            owner,
            id,
            new_content(&staged, content_type, &current.name, modified_at),
        )
        .await;
        self.content
            .publish_stored(self.content.finish_store(owner, staged, stored).await)
            .await
    }

    async fn update_as(
        &self,
        owner: UserId,
        id: NodeId,
        update: NodeUpdate,
    ) -> Result<MoveOutcome, FilesError> {
        let outcome = tree::update_node(&self.content.db, owner, id, update).await?;
        self.content.apply_blob_ops(owner, &outcome.blob_ops).await;
        self.content.events.publish(emitted::moved(&outcome)).await;
        Ok(outcome)
    }

    async fn move_nodes_as(
        &self,
        owner: UserId,
        ids: &[NodeId],
        parent: Option<NodeId>,
        on_conflict: OnConflict,
    ) -> Result<MoveBatch, FilesError> {
        let batch = tree::move_nodes(&self.content.db, owner, ids, parent, on_conflict).await?;
        self.content.apply_blob_ops(owner, &batch.blob_ops).await;
        self.content
            .events
            .publish(emitted::moved_batch(owner, &batch, batch.binned))
            .await;
        Ok(batch)
    }
}
