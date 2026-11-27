use sqlx::PgConnection;
use uuid::Uuid;

use crate::{Db, DbError, Node, NodeId, UserId};

/// MIME type stored for folders.
pub const FOLDER_MIME: &str = "inode/directory";

macro_rules! node_columns {
    () => {
        "id, owner_id, parent_id, kind, name, size, sha256, mime, version, created_at, updated_at, \
         edited_on"
    };
}

mod locks;
mod types;
mod writes;
pub use types::{DeletedNodes, NewContent, NewNode, NodeProperties};
pub use writes::FileState;

impl Db {
    /// Inserts a node under a folder owned by `owner` (or at the root).
    pub async fn create_node(&self, owner: UserId, new: NewNode) -> Result<Node, DbError> {
        let mut tx = self.begin().await?;
        require_folder(&mut tx, owner, new.parent_id).await?;
        let node = sqlx::query_as::<_, Node>(concat!(
            "INSERT INTO nodes (id, owner_id, parent_id, kind, name, size, sha256, mime) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING ",
            node_columns!(),
            ""
        ))
        .bind(new.id)
        .bind(owner)
        .bind(new.parent_id)
        .bind(new.kind)
        .bind(new.name)
        .bind(new.size)
        .bind(new.sha256)
        .bind(new.mime)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(node)
    }

    /// Fetches a node owned by `owner`; `NotFound` for missing or foreign nodes.
    pub async fn get_node(&self, owner: UserId, id: NodeId) -> Result<Node, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            node_columns!(),
            " FROM nodes WHERE id = $1 AND owner_id = $2 AND ",
            visible!("nodes")
        ))
        .bind(id)
        .bind(owner)
        .fetch_one(self.pool())
        .await?;
        Ok(node)
    }

    /// Lists the children of a folder (or the root) ordered folders first, then by name.
    pub async fn list_children(
        &self,
        owner: UserId,
        parent_id: Option<NodeId>,
    ) -> Result<Vec<Node>, DbError> {
        self.require_folder(owner, parent_id).await?;
        let nodes = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            node_columns!(),
            " FROM nodes \
             WHERE owner_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND ",
            visible!("nodes"),
            " ORDER BY kind, name"
        ))
        .bind(owner)
        .bind(parent_id)
        .fetch_all(self.pool())
        .await?;
        Ok(nodes)
    }

    /// Lists the ancestors of a node from the root down to its parent.
    pub async fn list_ancestors(&self, owner: UserId, id: NodeId) -> Result<Vec<Node>, DbError> {
        let node = self.get_node(owner, id).await?;
        let ancestors = sqlx::query_as::<_, Node>(concat!(
            "WITH RECURSIVE chain AS ( \
                 SELECT ",
            node_columns!(),
            ", 1 AS depth FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT ",
            "n.id, n.owner_id, n.parent_id, n.kind, n.name, n.size, n.sha256, n.mime, \
                 n.version, n.created_at, n.updated_at, n.edited_on, c.depth + 1 \
                 FROM nodes n JOIN chain c ON n.id = c.parent_id AND n.owner_id = $2 \
             ) SELECT ",
            node_columns!(),
            " FROM chain ORDER BY depth DESC"
        ))
        .bind(node.parent_id)
        .bind(owner)
        .fetch_all(self.pool())
        .await?;
        Ok(ancestors)
    }

    /// The predefined properties of a node of `owner`; none of them set if it has no row.
    pub async fn node_properties(
        &self,
        owner: UserId,
        id: NodeId,
    ) -> Result<NodeProperties, DbError> {
        self.get_node(owner, id).await?;
        let properties = sqlx::query_as::<_, NodeProperties>(
            "SELECT source_device FROM node_properties WHERE node_id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(properties.unwrap_or_default())
    }

    /// Deletes a node and its descendants; returns the ids whose stored content is now unused.
    pub async fn delete_node(&self, owner: UserId, id: NodeId) -> Result<DeletedNodes, DbError> {
        let mut tx = self.begin().await?;
        let files = sqlx::query_scalar::<_, NodeId>(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id, kind FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id, n.kind FROM nodes n JOIN subtree s ON n.parent_id = s.id \
             ) SELECT id FROM subtree WHERE kind = 'file'",
        )
        .bind(id)
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        let versions = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM file_versions WHERE owner_id = $1 AND node_id = ANY($2)",
        )
        .bind(owner)
        .bind(files.iter().map(|file| file.0).collect::<Vec<_>>())
        .fetch_all(&mut *tx)
        .await?;
        let deleted = sqlx::query("DELETE FROM nodes WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        if deleted.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        tx.commit().await?;
        Ok(DeletedNodes { files, versions })
    }

    async fn require_folder(&self, owner: UserId, id: Option<NodeId>) -> Result<(), DbError> {
        let mut conn = self.pool().acquire().await?;
        require_folder(&mut conn, owner, id).await
    }

    /// Renames the user's node `node` from `from` to `to`; `false` when its name is no longer
    /// `from`, and `Conflict` when `to` is taken.
    pub async fn rename_node_from(
        &self,
        user: UserId,
        node: NodeId,
        from: &str,
        to: &str,
    ) -> Result<bool, DbError> {
        let renamed =
            sqlx::query("UPDATE nodes SET name = $4 WHERE id = $1 AND owner_id = $2 AND name = $3")
                .bind(node)
                .bind(user)
                .bind(from)
                .bind(to)
                .execute(self.pool())
                .await?
                .rows_affected();
        Ok(renamed == 1)
    }
}

pub(crate) async fn require_folder(
    conn: &mut PgConnection,
    owner: UserId,
    id: Option<NodeId>,
) -> Result<(), DbError> {
    match id {
        None => Ok(()),
        Some(id) => {
            let exists = sqlx::query_scalar::<_, bool>(concat!(
                "SELECT EXISTS (SELECT 1 FROM nodes \
                 WHERE id = $1 AND owner_id = $2 AND kind = 'folder' AND ",
                visible!("nodes"),
                ")"
            ))
            .bind(id)
            .bind(owner)
            .fetch_one(conn)
            .await?;
            if exists {
                Ok(())
            } else {
                Err(DbError::NotFound)
            }
        }
    }
}
