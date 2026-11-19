//! Changing nodes and their versions inside a [`Tx`]. Each step writes what it is told to;
//! the file services decide which steps a change takes.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DEVICE_SETTING, DbError, NewContent, Node, NodeId, Tx};

/// What a file holds after another one was folded into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileState {
    pub size: i64,
    pub sha256: Option<Vec<u8>>,
    pub mime: String,
    pub version: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl Tx {
    /// Inserts a file named `name` in `parent` holding `content`, last changed at its
    /// `modified_at` (now when `None`).
    pub async fn insert_file(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        content: &NewContent,
    ) -> Result<Node, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "INSERT INTO nodes \
             (id, owner_id, parent_id, kind, name, size, sha256, mime, updated_at) \
             VALUES ($1, $2, $3, 'file', $4, $5, $6, $7, COALESCE($8, now())) RETURNING ",
            node_columns!(),
            ""
        ))
        .bind(NodeId::new())
        .bind(self.owner)
        .bind(parent)
        .bind(name)
        .bind(content.size)
        .bind(&content.sha256)
        .bind(&content.mime)
        .bind(content.modified_at)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(node)
    }

    /// Puts node `id` in `parent` under `name`, taking it out of the recycle bin if it was
    /// there. With `touch`, it counts as changed now.
    pub async fn relocate(
        &mut self,
        id: NodeId,
        parent: Option<NodeId>,
        name: &str,
        touch: bool,
    ) -> Result<Node, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "UPDATE nodes SET name = $3, parent_id = $4, trashed_at = NULL, \
             updated_at = CASE WHEN $5 THEN now() ELSE updated_at END \
             WHERE id = $1 AND owner_id = $2 RETURNING ",
            node_columns!(),
            ""
        ))
        .bind(id)
        .bind(self.owner)
        .bind(name)
        .bind(parent)
        .bind(touch)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(node)
    }

    /// Sets when node `id` was created and last changed.
    pub async fn set_dates(
        &mut self,
        id: NodeId,
        created_at: OffsetDateTime,
        updated_at: OffsetDateTime,
    ) -> Result<Node, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "UPDATE nodes SET created_at = $3, updated_at = $4 \
             WHERE id = $1 AND owner_id = $2 RETURNING ",
            node_columns!(),
            ""
        ))
        .bind(id)
        .bind(self.owner)
        .bind(created_at)
        .bind(updated_at)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(node)
    }

    /// Records the current content of `file` as a version under its current number, made on
    /// the device that made the content. Returns the version's id.
    pub async fn archive_current(&mut self, file: &Node) -> Result<Uuid, DbError> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO file_versions \
             (id, node_id, owner_id, version, size, sha256, mime, modified_at, edited_on) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(file.id)
        .bind(self.owner)
        .bind(file.version)
        .bind(file.size)
        .bind(file.sha256.clone().unwrap_or_default())
        .bind(&file.mime)
        .bind(file.updated_at)
        .bind(file.edited_on)
        .execute(&mut *self.conn)
        .await?;
        Ok(id)
    }

    /// Records `content`, last changed at `modified_at`, as version `version` of `file`, made
    /// on this transaction's device. Returns the version's id.
    pub async fn insert_version(
        &mut self,
        file: NodeId,
        version: i32,
        content: &NewContent,
        modified_at: OffsetDateTime,
    ) -> Result<Uuid, DbError> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO file_versions \
             (id, node_id, owner_id, version, size, sha256, mime, modified_at, edited_on) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULLIF(current_setting($9, true), '')::uuid)",
        )
        .bind(id)
        .bind(file)
        .bind(self.owner)
        .bind(version)
        .bind(content.size)
        .bind(&content.sha256)
        .bind(&content.mime)
        .bind(modified_at)
        .bind(DEVICE_SETTING)
        .execute(&mut *self.conn)
        .await?;
        Ok(id)
    }

    /// Gives the current content of `file` the next version number.
    pub async fn bump_version(&mut self, file: NodeId) -> Result<Node, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "UPDATE nodes SET version = version + 1 WHERE id = $1 AND owner_id = $2 RETURNING ",
            node_columns!(),
            ""
        ))
        .bind(file)
        .bind(self.owner)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(node)
    }

    /// Makes `content` the current content of `file` under the next version number, last
    /// changed at its `modified_at` (now when `None`).
    pub async fn set_content(
        &mut self,
        file: NodeId,
        content: &NewContent,
    ) -> Result<Node, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "UPDATE nodes SET size = $3, sha256 = $4, mime = $5, version = version + 1, \
             updated_at = COALESCE($6, now()) WHERE id = $1 AND owner_id = $2 RETURNING ",
            node_columns!(),
            ""
        ))
        .bind(file)
        .bind(self.owner)
        .bind(content.size)
        .bind(&content.sha256)
        .bind(&content.mime)
        .bind(content.modified_at)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(node)
    }

    /// Folds file `source` into file `into`: the versions of `source` become versions of
    /// `into` with their numbers raised by `version_offset`, `source` is removed, and `into`
    /// takes `state`.
    pub async fn absorb(
        &mut self,
        source: NodeId,
        into: NodeId,
        version_offset: i32,
        state: &FileState,
    ) -> Result<Node, DbError> {
        sqlx::query(
            "UPDATE file_versions SET node_id = $3, version = version + $4 \
             WHERE node_id = $1 AND owner_id = $2",
        )
        .bind(source)
        .bind(self.owner)
        .bind(into)
        .bind(version_offset)
        .execute(&mut *self.conn)
        .await?;
        sqlx::query("DELETE FROM nodes WHERE id = $1 AND owner_id = $2")
            .bind(source)
            .bind(self.owner)
            .execute(&mut *self.conn)
            .await?;
        let node = sqlx::query_as::<_, Node>(concat!(
            "UPDATE nodes SET size = $3, sha256 = $4, mime = $5, version = $6, \
             created_at = $7, updated_at = $8 WHERE id = $1 AND owner_id = $2 RETURNING ",
            node_columns!(),
            ""
        ))
        .bind(into)
        .bind(self.owner)
        .bind(state.size)
        .bind(&state.sha256)
        .bind(&state.mime)
        .bind(state.version)
        .bind(state.created_at)
        .bind(state.updated_at)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(node)
    }

    /// Removes `folder` if nothing is left in it, binned files included. Returns whether it
    /// was removed.
    pub async fn remove_if_empty(&mut self, folder: NodeId) -> Result<bool, DbError> {
        let removed = sqlx::query(
            "DELETE FROM nodes WHERE id = $1 AND owner_id = $2 \
             AND NOT EXISTS (SELECT 1 FROM nodes child WHERE child.parent_id = $1)",
        )
        .bind(folder)
        .bind(self.owner)
        .execute(&mut *self.conn)
        .await?;
        Ok(removed.rows_affected() > 0)
    }

    /// Records this transaction's device as the source of `file`. Without a device nothing is
    /// recorded.
    pub async fn record_source_device(&mut self, file: NodeId) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO node_properties (node_id, source_device) \
             SELECT $1, name FROM devices \
             WHERE user_id = $2 AND id = NULLIF(current_setting($3, true), '')::uuid \
             ON CONFLICT (node_id) DO UPDATE SET source_device = EXCLUDED.source_device",
        )
        .bind(file)
        .bind(self.owner)
        .bind(DEVICE_SETTING)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }
}
