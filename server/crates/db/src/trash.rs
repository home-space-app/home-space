use std::collections::HashMap;

use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    Db, DbError, DeletedNodes, MoveBatch, Node, NodeId, OnConflict, UserId,
    nodes::{ensure_not_descendant, lock_node, move_into},
};

/// Name of the folder restored files land in.
pub const LOST_AND_FOUND: &str = "Lost & Found";

macro_rules! node_columns {
    () => {
        "n.id, n.owner_id, n.parent_id, n.kind, n.name, n.size, n.sha256, n.mime, n.version, \
         n.created_at, n.updated_at, n.edited_on"
    };
}

/// The folders every user has besides their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, FromRow)]
pub struct SpecialFolders {
    /// Hidden root folder holding the binned files.
    pub trash_id: NodeId,
    /// Root folder that restored files land in.
    pub lost_and_found_id: NodeId,
}

/// A file in the recycle bin.
#[derive(Clone, Debug, FromRow)]
pub struct TrashedNode {
    #[sqlx(flatten)]
    pub node: Node,
    /// Where the file lived when it was deleted, e.g. `/Photos/2024/a.jpg`.
    pub original_path: String,
    pub trashed_at: OffsetDateTime,
}

/// What moving a node to the recycle bin did.
#[derive(Clone, Debug)]
pub struct TrashOutcome {
    /// The node that was deleted.
    pub node: Node,
    /// The files now in the bin: the node itself or every file below it.
    pub trashed: Vec<Node>,
}

impl Db {
    /// The ids of the user's recycle bin and Lost & Found folders.
    pub async fn special_folders(&self, owner: UserId) -> Result<SpecialFolders, DbError> {
        let mut conn = self.pool().acquire().await?;
        special_folders(&mut conn, owner).await
    }

    /// Deletes a node by moving its files to the recycle bin.
    ///
    /// A file moves there itself. A folder's files all move there, each remembering its path,
    /// and the folders are removed. Lost & Found, and folders containing it, cannot be deleted.
    pub async fn trash_node(&self, owner: UserId, id: NodeId) -> Result<TrashOutcome, DbError> {
        let mut tx = self.begin().await?;
        let node = lock_node(&mut tx, owner, id).await?;
        let folders = special_folders(&mut tx, owner).await?;
        match ensure_not_descendant(&mut tx, owner, id, folders.lost_and_found_id).await {
            Err(DbError::Cycle) => return Err(DbError::LostAndFound),
            other => other?,
        }
        let trashed = bin_subtree(&mut tx, owner, id, node.parent_id, folders.trash_id).await?;
        // The files have left the folders, so removing them takes nothing else along.
        sqlx::query("DELETE FROM nodes WHERE id = $1 AND owner_id = $2 AND kind = 'folder'")
            .bind(id)
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(TrashOutcome { node, trashed })
    }

    /// Lists the files in the recycle bin, most recently deleted first.
    pub async fn list_trash(&self, owner: UserId) -> Result<Vec<TrashedNode>, DbError> {
        let items = sqlx::query_as::<_, TrashedNode>(concat!(
            "SELECT ",
            node_columns!(),
            ", t.original_path, n.trashed_at \
             FROM nodes n JOIN trash_entries t ON t.node_id = n.id \
             WHERE n.owner_id = $1 AND n.trashed_at IS NOT NULL \
             ORDER BY n.trashed_at DESC, t.original_path, n.id"
        ))
        .bind(owner)
        .fetch_all(self.pool())
        .await?;
        Ok(items)
    }

    /// Moves binned files to Lost & Found.
    ///
    /// The files land directly in Lost & Found under their own names; `on_conflict` decides
    /// what happens to files whose name is taken there. Conflict paths are the original paths.
    /// Ids that are not binned files are ignored, unknown ids are `NotFound`. When two
    /// selected files share a name, the one deleted last ends up current.
    pub async fn restore(
        &self,
        owner: UserId,
        ids: &[NodeId],
        on_conflict: OnConflict,
    ) -> Result<MoveBatch, DbError> {
        let mut tx = self.begin().await?;
        let folders = special_folders(&mut tx, owner).await?;
        let mut unique = ids.to_vec();
        unique.sort_by_key(|id| id.0);
        unique.dedup();
        let raw = unique.iter().map(|id| id.0).collect::<Vec<Uuid>>();
        let known = sqlx::query_scalar::<_, i64>(concat!(
            "SELECT count(*) FROM nodes WHERE id = ANY($1) AND owner_id = $2 \
             AND (trashed_at IS NOT NULL OR ",
            visible!("nodes"),
            ")"
        ))
        .bind(&raw)
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
        if usize::try_from(known).ok() != Some(unique.len()) {
            return Err(DbError::NotFound);
        }
        // Oldest first, so that the newest of several same-named files ends up current.
        let binned = sqlx::query_as::<_, TrashedNode>(concat!(
            "SELECT ",
            node_columns!(),
            ", t.original_path, n.trashed_at \
             FROM nodes n JOIN trash_entries t ON t.node_id = n.id \
             WHERE n.id = ANY($1) AND n.owner_id = $2 AND n.trashed_at IS NOT NULL \
             ORDER BY n.trashed_at, t.original_path, n.id \
             FOR UPDATE OF n"
        ))
        .bind(&raw)
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        let target = lock_node(&mut tx, owner, folders.lost_and_found_id).await?;
        let paths = binned
            .iter()
            .map(|item| (item.node.id, item.original_path.clone()))
            .collect::<HashMap<_, _>>();
        let sources = binned.into_iter().map(|item| item.node).collect::<Vec<_>>();
        let source_ids = sources.iter().map(|node| node.id).collect::<Vec<_>>();
        let mut batch = move_into(&mut tx, owner, sources, Some(target), on_conflict, None).await?;
        for conflict in &mut batch.conflicts {
            if let Some(path) = paths.get(&conflict.source.id) {
                conflict.path = path.clone();
            }
        }
        if batch.conflicts.is_empty() || on_conflict != OnConflict::Abort {
            let restored = source_ids
                .iter()
                .filter(|id| batch.skipped.iter().all(|skipped| skipped.id != **id))
                .map(|id| id.0)
                .collect::<Vec<Uuid>>();
            sqlx::query("DELETE FROM trash_entries WHERE node_id = ANY($1)")
                .bind(restored)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(batch)
    }

    /// Permanently deletes every binned file; returns the ids whose content is now unused.
    pub async fn empty_trash(&self, owner: UserId) -> Result<DeletedNodes, DbError> {
        let mut tx = self.begin().await?;
        let deleted = delete_binned(&mut tx, owner, None).await?;
        tx.commit().await?;
        Ok(deleted)
    }

    /// Permanently deletes one binned file; `NotFound` when it is not in the bin.
    pub async fn purge_trashed(&self, owner: UserId, id: NodeId) -> Result<DeletedNodes, DbError> {
        let mut tx = self.begin().await?;
        let deleted = delete_binned(&mut tx, owner, Some(id)).await?;
        if deleted.files.is_empty() {
            return Err(DbError::NotFound);
        }
        tx.commit().await?;
        Ok(deleted)
    }
}

/// Moves the files at or below `id` to the recycle bin, remembering their paths.
///
/// `parent` is the current parent of `id`. Folders stay where they are; callers remove them.
/// Returns the binned files.
pub(crate) async fn bin_subtree(
    conn: &mut PgConnection,
    owner: UserId,
    id: NodeId,
    parent: Option<NodeId>,
    trash_id: NodeId,
) -> Result<Vec<Node>, DbError> {
    let directory = sqlx::query_scalar::<_, String>(
        "WITH RECURSIVE chain AS ( \
             SELECT id, parent_id, name, 1 AS depth FROM nodes \
             WHERE id = $1 AND owner_id = $2 \
             UNION ALL \
             SELECT n.id, n.parent_id, n.name, c.depth + 1 \
             FROM nodes n JOIN chain c ON n.id = c.parent_id AND n.owner_id = $2 \
             ) SELECT name FROM chain ORDER BY depth DESC",
    )
    .bind(parent)
    .bind(owner)
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .fold(String::new(), |path, name| format!("{path}/{name}"));
    let trashed = sqlx::query_as::<_, Node>(concat!(
        "WITH RECURSIVE subtree AS ( \
             SELECT id, kind, $3::text || '/' || name AS path FROM nodes \
             WHERE id = $1 AND owner_id = $2 \
             UNION ALL \
             SELECT n.id, n.kind, s.path || '/' || n.name \
             FROM nodes n JOIN subtree s ON n.parent_id = s.id \
             ), files AS ( \
             SELECT id, path FROM subtree WHERE kind = 'file' \
             ), entries AS ( \
             INSERT INTO trash_entries (node_id, original_path) SELECT id, path FROM files \
             ), unindexed AS ( \
             DELETE FROM search_entries WHERE node_id IN (SELECT id FROM files) \
             ) \
             UPDATE nodes n SET parent_id = $4, trashed_at = now() \
             WHERE n.id IN (SELECT id FROM files) RETURNING ",
        node_columns!(),
        ""
    ))
    .bind(id)
    .bind(owner)
    .bind(directory)
    .bind(trash_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(trashed)
}

pub(crate) async fn special_folders(
    conn: &mut PgConnection,
    owner: UserId,
) -> Result<SpecialFolders, DbError> {
    let folders = sqlx::query_as::<_, SpecialFolders>(
        "SELECT trash_id, lost_and_found_id FROM user_folders WHERE user_id = $1",
    )
    .bind(owner)
    .fetch_one(conn)
    .await?;
    Ok(folders)
}

/// Deletes the binned files of `owner`, or just `only`, with their versions.
async fn delete_binned(
    conn: &mut PgConnection,
    owner: UserId,
    only: Option<NodeId>,
) -> Result<DeletedNodes, DbError> {
    let files = sqlx::query_scalar::<_, NodeId>(
        "SELECT id FROM nodes \
         WHERE owner_id = $1 AND trashed_at IS NOT NULL AND ($2::uuid IS NULL OR id = $2) \
         FOR UPDATE",
    )
    .bind(owner)
    .bind(only)
    .fetch_all(&mut *conn)
    .await?;
    let raw = files.iter().map(|file| file.0).collect::<Vec<Uuid>>();
    let versions = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM file_versions WHERE owner_id = $1 AND node_id = ANY($2)",
    )
    .bind(owner)
    .bind(&raw)
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM nodes WHERE owner_id = $1 AND id = ANY($2)")
        .bind(owner)
        .bind(raw)
        .execute(conn)
        .await?;
    Ok(DeletedNodes { files, versions })
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use crate::{NewNode, NodeKind};

    use super::*;

    async fn file(db: &Db, owner: UserId, parent: Option<NodeId>, name: &str) -> Node {
        let new = NewNode {
            id: NodeId::new(),
            parent_id: parent,
            kind: NodeKind::File,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: "text/plain".to_owned(),
        };
        db.create_node(owner, new).await.expect("file")
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn users_get_a_hidden_trash_folder_named_after_its_id(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(crate::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let folders = db.special_folders(alice).await.expect("folders");
        let name = sqlx::query_scalar::<_, String>("SELECT name FROM nodes WHERE id = $1")
            .bind(folders.trash_id)
            .fetch_one(db.pool())
            .await
            .expect("name");
        assert_eq!(name, format!(".recycle-bin-{}", folders.trash_id));
        let root = db.list_children(alice, None).await.expect("root");
        assert_eq!(
            root.iter().map(|node| node.id).collect::<Vec<_>>(),
            [folders.lost_and_found_id]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deleting_a_user_removes_their_nodes_and_special_folders(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(crate::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let bob = db
            .create_user(crate::TenantId::DEFAULT, "bob", "x")
            .await
            .expect("bob")
            .id;
        let binned = file(&db, alice, None, "binned.txt").await;
        file(&db, alice, None, "live.txt").await;
        file(&db, bob, None, "bobs.txt").await;
        db.trash_node(alice, binned.id).await.expect("trash");

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(alice)
            .execute(db.pool())
            .await
            .expect("delete user");

        let left = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM nodes WHERE owner_id = $1")
            .bind(alice)
            .fetch_one(db.pool())
            .await
            .expect("count");
        assert_eq!(left, 0);
        let entries = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM trash_entries")
            .fetch_one(db.pool())
            .await
            .expect("count");
        assert_eq!(entries, 0);
        assert!(db.special_folders(bob).await.is_ok());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn trashing_removes_the_search_entries_and_restoring_allows_reindexing(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(crate::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let node = file(&db, alice, None, "a.txt").await;
        db.upsert_name_entry(alice, node.id, "a txt")
            .await
            .expect("index");
        db.trash_node(alice, node.id).await.expect("trash");
        assert!(
            db.search_nodes(alice, "a:*", 10)
                .await
                .expect("s")
                .is_empty()
        );
        assert!(
            db.nodes_missing_entries(10)
                .await
                .expect("m")
                .iter()
                .all(|n| n.id != node.id)
        );
        // A binned file cannot be indexed again until it is restored.
        db.upsert_name_entry(alice, node.id, "a txt")
            .await
            .expect("index");
        assert!(
            db.search_nodes(alice, "a:*", 10)
                .await
                .expect("s")
                .is_empty()
        );
        db.restore(alice, &[node.id], OnConflict::Abort)
            .await
            .expect("restore");
        db.upsert_name_entry(alice, node.id, "a txt")
            .await
            .expect("index");
        assert_eq!(db.search_nodes(alice, "a:*", 10).await.expect("s").len(), 1);
    }
}
