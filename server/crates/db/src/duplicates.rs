//! Live files sharing their content with other live files or with previous versions.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, Node, NodeId, UserId};

macro_rules! duplicate_columns {
    () => {
        "n.id, n.owner_id, n.parent_id, n.kind, n.name, n.size, n.sha256, n.mime, n.version, \
         n.created_at, n.updated_at, n.edited_on"
    };
}

/// A live file holding the content of a previous version of another live file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, FromRow)]
pub struct OutdatedRow {
    /// The live file with the old content.
    pub file_id: NodeId,
    /// The file whose previous version matches it.
    pub source_id: NodeId,
    pub version: i32,
    pub archived_at: OffsetDateTime,
}

impl Db {
    /// The owner's live, non-empty files whose content another live file shares, ordered by
    /// content and then oldest first.
    pub async fn duplicate_nodes(&self, owner: UserId) -> Result<Vec<Node>, DbError> {
        let nodes = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            duplicate_columns!(),
            " FROM nodes n \
             WHERE n.owner_id = $1 AND n.kind = 'file' AND n.sha256 IS NOT NULL AND n.size > 0 \
             AND ",
            visible!("n"),
            " AND n.sha256 IN ( \
                 SELECT d.sha256 FROM nodes d \
                 WHERE d.owner_id = $1 AND d.kind = 'file' AND d.sha256 IS NOT NULL \
                 AND d.size > 0 AND ",
            visible!("d"),
            " GROUP BY d.sha256 HAVING count(*) > 1) \
             ORDER BY n.sha256, n.created_at, n.id"
        ))
        .bind(owner)
        .fetch_all(self.pool())
        .await?;
        Ok(nodes)
    }

    /// Live files whose content is a previous version of another live file that has changed
    /// since, keeping the newest such version per pair.
    pub async fn outdated_matches(&self, owner: UserId) -> Result<Vec<OutdatedRow>, DbError> {
        let rows = sqlx::query_as::<_, OutdatedRow>(concat!(
            "SELECT DISTINCT ON (n.id, src.id) \
                 n.id AS file_id, src.id AS source_id, fv.version, fv.archived_at \
             FROM nodes n \
             JOIN file_versions fv \
                 ON fv.owner_id = n.owner_id AND fv.sha256 = n.sha256 AND fv.node_id <> n.id \
             JOIN nodes src ON src.id = fv.node_id AND src.owner_id = n.owner_id \
             WHERE n.owner_id = $1 AND n.kind = 'file' AND n.size > 0 \
             AND src.sha256 IS DISTINCT FROM n.sha256 AND ",
            visible!("n"),
            " AND ",
            visible!("src"),
            " ORDER BY n.id, src.id, fv.version DESC"
        ))
        .bind(owner)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// The owner's live nodes among `ids`, in no particular order.
    pub async fn nodes_by_ids(&self, owner: UserId, ids: &[NodeId]) -> Result<Vec<Node>, DbError> {
        let ids = ids.iter().map(|id| id.0).collect::<Vec<Uuid>>();
        let nodes = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            duplicate_columns!(),
            " FROM nodes n WHERE n.owner_id = $1 AND n.id = ANY($2) AND ",
            visible!("n")
        ))
        .bind(owner)
        .bind(ids)
        .fetch_all(self.pool())
        .await?;
        Ok(nodes)
    }
}

#[cfg(test)]
mod tests;
