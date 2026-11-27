//! Where password folders are in the tree, and what they hold.

use protocol::ProtectionState;
use uuid::Uuid;

use super::{EnclosingPasswordFolder, FolderFile, FolderNode};
use crate::{Db, DbError, NodeId, Tx, UserId};

/// The password folder that is `$1`, a node of the user `$2`, or holds it.
const ENCLOSING_PASSWORD_FOLDER: &str = "WITH RECURSIVE up AS ( \
         SELECT id, parent_id FROM nodes WHERE id = $1 AND owner_id = $2 \
         UNION ALL \
         SELECT n.id, n.parent_id FROM nodes n JOIN up ON n.id = up.parent_id \
     ) SELECT p.node_id, p.recipient, p.state FROM protected_folders p \
     JOIN up ON p.node_id = up.id WHERE p.lock = 'password' LIMIT 1";

impl Db {
    /// The password folder that is `node` or holds it, if any.
    pub async fn enclosing_password_folder(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Option<EnclosingPasswordFolder>, DbError> {
        let found = sqlx::query_as::<_, EnclosingPasswordFolder>(ENCLOSING_PASSWORD_FOLDER)
            .bind(node)
            .bind(user)
            .fetch_optional(self.pool())
            .await?;
        Ok(found)
    }

    /// The user's password folders.
    pub async fn password_folders(&self, user: UserId) -> Result<Vec<NodeId>, DbError> {
        let folders = sqlx::query_scalar(
            "SELECT node_id FROM protected_folders WHERE owner_id = $1 AND lock = 'password'",
        )
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(folders)
    }

    /// The user's password folders that are `node` or below it.
    pub async fn password_folders_under(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Vec<NodeId>, DbError> {
        let folders = sqlx::query_scalar(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
                 WHERE n.kind = 'folder' \
             ) SELECT p.node_id FROM protected_folders p JOIN subtree s ON s.id = p.node_id \
             WHERE p.lock = 'password'",
        )
        .bind(node)
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(folders)
    }

    /// For each of the user's folders `folders` that is a password folder or inside one, that
    /// password folder: what holds the nodes in it.
    pub async fn password_folders_over(
        &self,
        user: UserId,
        folders: &[NodeId],
    ) -> Result<Vec<(NodeId, NodeId)>, DbError> {
        if folders.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<Uuid> = folders.iter().map(|folder| folder.0).collect();
        let found = sqlx::query_as(
            "WITH RECURSIVE up AS ( \
                 SELECT id AS origin, id FROM nodes WHERE owner_id = $1 AND id = ANY($2) \
                 UNION ALL \
                 SELECT up.origin, n.parent_id FROM nodes n JOIN up ON n.id = up.id \
                 WHERE n.parent_id IS NOT NULL \
             ) SELECT DISTINCT up.origin, p.node_id FROM up \
             JOIN protected_folders p ON p.node_id = up.id AND p.lock = 'password'",
        )
        .bind(user)
        .bind(&ids)
        .fetch_all(self.pool())
        .await?;
        Ok(found)
    }

    /// Whether any of the user's `nodes` is a protected folder or inside one.
    pub async fn any_protected(&self, user: UserId, nodes: &[NodeId]) -> Result<bool, DbError> {
        let ids: Vec<Uuid> = nodes.iter().map(|node| node.0).collect();
        let found = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM nodes n WHERE n.owner_id = $1 AND n.id = ANY($2) \
             AND (in_protected_folder(n.id) \
                  OR EXISTS (SELECT 1 FROM protected_folders p WHERE p.node_id = n.id)))",
        )
        .bind(user)
        .bind(&ids)
        .fetch_one(self.pool())
        .await?;
        Ok(found)
    }

    /// The password folders of `tenant`'s users whose content the server is still encrypting
    /// or decrypting, with their state.
    pub async fn converting_password_folders(
        &self,
        tenant: crate::TenantId,
    ) -> Result<Vec<(UserId, NodeId, ProtectionState)>, DbError> {
        let folders = sqlx::query_as(
            "SELECT p.owner_id, p.node_id, p.state FROM protected_folders p \
             JOIN users u ON u.id = p.owner_id \
             WHERE u.tenant_id = $1 AND p.lock = 'password' AND p.state <> 'protected' \
             ORDER BY p.created_at",
        )
        .bind(tenant)
        .fetch_all(self.pool())
        .await?;
        Ok(folders)
    }

    /// The files inside the user's folder `node`, with the ids of their older versions.
    pub async fn folder_files(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Vec<FolderFile>, DbError> {
        let rows: Vec<(NodeId, Option<Uuid>)> = sqlx::query_as(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id, kind FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id, n.kind FROM nodes n JOIN subtree s ON n.parent_id = s.id \
             ) SELECT s.id, v.id FROM subtree s \
             LEFT JOIN file_versions v ON v.node_id = s.id \
             WHERE s.kind = 'file' ORDER BY s.id, v.version",
        )
        .bind(node)
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        let mut files: Vec<FolderFile> = Vec::new();
        for (id, version) in rows {
            if files.last().is_none_or(|file| file.node_id != id) {
                files.push(FolderFile {
                    node_id: id,
                    versions: Vec::new(),
                });
            }
            if let (Some(file), Some(version)) = (files.last_mut(), version) {
                file.versions.push(version);
            }
        }
        Ok(files)
    }

    /// The nodes inside the user's folder `node`, not the folder itself, parents before their
    /// children.
    pub async fn folder_nodes(
        &self,
        user: UserId,
        node: NodeId,
    ) -> Result<Vec<FolderNode>, DbError> {
        let nodes = sqlx::query_as::<_, FolderNode>(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id, parent_id, kind, name, 0 AS depth FROM nodes \
                 WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id, n.parent_id, n.kind, n.name, s.depth + 1 FROM nodes n \
                 JOIN subtree s ON n.parent_id = s.id \
             ) SELECT id, parent_id, kind, name FROM subtree WHERE depth > 0 \
             ORDER BY depth, id",
        )
        .bind(node)
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(nodes)
    }
}

impl Tx {
    /// The password folder that is `node` or holds it; `None` for the root.
    pub async fn password_folder_of(
        &mut self,
        node: Option<NodeId>,
    ) -> Result<Option<EnclosingPasswordFolder>, DbError> {
        let Some(node) = node else {
            return Ok(None);
        };
        let found = sqlx::query_as::<_, EnclosingPasswordFolder>(ENCLOSING_PASSWORD_FOLDER)
            .bind(node)
            .bind(self.owner)
            .fetch_optional(&mut *self.conn)
            .await?;
        Ok(found)
    }
}
