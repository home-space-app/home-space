//! What users read and copy of the shares other users made for them.

use time::OffsetDateTime;
use tokio::fs;
use uuid::Uuid;

use db::{Node, NodeId, ReceivedShare, SharedAccess, UserId};
use files::{Actor, ArchiveEntry, Names, SavedCopy};

use crate::{ReceivedDetails, ShareError, ShareService, service::live};

impl ShareService {
    /// The share `id` made for `user`: `NotFound` when there is none, `Gone` once it has
    /// expired.
    pub async fn received(&self, user: UserId, id: Uuid) -> Result<ReceivedShare, ShareError> {
        live(self.db.received_share(user, id).await?)
    }

    /// The live shares made for `user` that still hold items.
    pub async fn received_all(&self, user: UserId) -> Result<Vec<ReceivedDetails>, ShareError> {
        let shares = self
            .db
            .received_shares(user, OffsetDateTime::now_utc())
            .await?;
        let ids: Vec<Uuid> = shares.iter().map(|share| share.id).collect();
        let mut items = self.db.share_items(&ids).await?;
        Ok(shares
            .into_iter()
            .map(|share| ReceivedDetails {
                items: items.remove(&share.id).unwrap_or_default(),
                share,
            })
            .filter(|details| !details.items.is_empty())
            .collect())
    }

    /// The children of the folder `parent` inside the share.
    pub async fn received_children(
        &self,
        user: UserId,
        id: Uuid,
        parent: NodeId,
    ) -> Result<Vec<Node>, ShareError> {
        let share = self.received(user, id).await?;
        self.children(&share, parent).await
    }

    /// A file of the share, opened for download.
    pub async fn received_file(
        &self,
        user: UserId,
        id: Uuid,
        node: NodeId,
    ) -> Result<(Node, fs::File), ShareError> {
        let share = self.received(user, id).await?;
        self.file(&share, node).await
    }

    /// Everything the share holds, or one folder inside it, with the owner's name to name
    /// the archive after.
    pub async fn received_archive(
        &self,
        user: UserId,
        id: Uuid,
        folder: Option<NodeId>,
    ) -> Result<(String, Vec<ArchiveEntry>), ShareError> {
        let share = self.received(user, id).await?;
        let entries = self.archive(&share, folder).await?;
        Ok((share.owner_name, entries))
    }

    /// Saves copies of shared items into `user`'s folder `parent`, made on `device`, storing
    /// names as `names`, those of `parent`, say.
    #[allow(clippy::too_many_arguments)]
    pub async fn copy_received(
        &self,
        user: UserId,
        device: Option<Uuid>,
        id: Uuid,
        node_ids: &[NodeId],
        parent: Option<NodeId>,
        names: &Names,
    ) -> Result<SavedCopy, ShareError> {
        let share = self.received(user, id).await?;
        let actor = Actor::new(user, device);
        Ok(self
            .nodes
            .copy_shared(actor, share.root(), node_ids, parent, names)
            .await?)
    }
}
