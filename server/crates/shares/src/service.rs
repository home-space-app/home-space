use time::OffsetDateTime;
use tokio::fs;

use auth::SharePasswords;
use db::{Db, Node, NodeId, NodeKind, SharedAccess, TenantId};
use files::{ArchiveEntry, NodeService};

use crate::ShareError;

/// The shares of one tenant.
#[derive(Clone, Debug)]
pub struct ShareService {
    pub(crate) db: Db,
    tenant: TenantId,
    /// Hashes link passwords and signs the proof that one was given, with the tenant's keys.
    pub(crate) passwords: SharePasswords,
    pub(crate) nodes: NodeService,
}

impl ShareService {
    pub fn new(db: Db, tenant: TenantId, passwords: SharePasswords, nodes: NodeService) -> Self {
        Self {
            db,
            tenant,
            passwords,
            nodes,
        }
    }

    pub(crate) fn tenant(&self) -> TenantId {
        self.tenant
    }

    /// The children of the folder `parent` inside `share`.
    pub(crate) async fn children(
        &self,
        share: &impl SharedAccess,
        parent: NodeId,
    ) -> Result<Vec<Node>, ShareError> {
        Ok(self.db.shared_children(share.root(), parent).await?)
    }

    /// A file of `share`, opened for download; a folder is `NotFound`.
    pub(crate) async fn file(
        &self,
        share: &impl SharedAccess,
        id: NodeId,
    ) -> Result<(Node, fs::File), ShareError> {
        let root = share.root();
        let node = self.db.shared_node(root, id).await?;
        if node.kind != NodeKind::File {
            return Err(ShareError::NotFound);
        }
        Ok(self.nodes.download(root.owner_id, node.id).await?)
    }

    /// Everything `share` holds, or one folder inside it, as archive entries.
    pub(crate) async fn archive(
        &self,
        share: &impl SharedAccess,
        folder: Option<NodeId>,
    ) -> Result<Vec<ArchiveEntry>, ShareError> {
        Ok(self.nodes.share_archive(share.root(), folder).await?)
    }
}

/// `share`, unless it has expired.
pub(crate) fn live<S: SharedAccess>(share: S) -> Result<S, ShareError> {
    if share.is_expired(OffsetDateTime::now_utc()) {
        Err(ShareError::Gone)
    } else {
        Ok(share)
    }
}
