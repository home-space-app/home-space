use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, Node, NodeId, UserId};

/// The latest change to one node, with its current state.
#[derive(Clone, Debug)]
pub struct NodeChange {
    pub seq: i64,
    pub id: NodeId,
    /// The node as it is now; `None` when it was deleted or moved to the recycle bin.
    pub node: Option<Node>,
    /// The device that made the change, when it was made from one.
    pub device_id: Option<Uuid>,
}

/// A page of an owner's change journal.
#[derive(Clone, Debug)]
pub struct ChangeBatch {
    /// One entry per changed node, in the order of their latest change.
    pub changes: Vec<NodeChange>,
    /// Where the next page starts: the seq of the last change returned, or where the
    /// journal ends when there were none.
    pub cursor: i64,
    /// Whether more changes follow `cursor`.
    pub more: bool,
}

impl Db {
    /// Lists the nodes of `owner` that changed after `since`, at most `limit` of them.
    ///
    /// Without `since` only the cursor where the journal ends is returned, for a client that
    /// is about to read the whole tree. A `since` older than what was pruned is
    /// [`DbError::Pruned`]: the changes in between are gone.
    pub async fn changes(
        &self,
        owner: UserId,
        since: Option<i64>,
        limit: i64,
    ) -> Result<ChangeBatch, DbError> {
        let pruned = sqlx::query_scalar::<_, i64>(
            "SELECT through FROM node_changes_pruned WHERE owner_id = $1",
        )
        .bind(owner)
        .fetch_optional(self.pool())
        .await?
        .unwrap_or(0);
        let Some(since) = since else {
            let end = sqlx::query_scalar::<_, Option<i64>>(
                "SELECT max(seq) FROM node_changes WHERE owner_id = $1",
            )
            .bind(owner)
            .fetch_one(self.pool())
            .await?;
            return Ok(ChangeBatch {
                changes: Vec::new(),
                cursor: end.unwrap_or(0).max(pruned),
                more: false,
            });
        };
        if since < pruned {
            return Err(DbError::Pruned);
        }
        let mut latest = sqlx::query_as::<_, (NodeId, i64, Option<Uuid>)>(
            "SELECT node_id, seq, device_id FROM ( \
                 SELECT DISTINCT ON (node_id) node_id, seq, device_id FROM node_changes c \
                 WHERE owner_id = $1 AND seq > $2 AND NOT EXISTS \
                 (SELECT 1 FROM user_folders uf WHERE uf.trash_id = c.node_id) \
                 ORDER BY node_id, seq DESC \
             ) latest ORDER BY seq LIMIT $3",
        )
        .bind(owner)
        .bind(since)
        .bind(limit + 1)
        .fetch_all(self.pool())
        .await?;
        let more = latest.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        latest.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        let ids = latest.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
        let mut nodes = sqlx::query_as::<_, Node>(concat!(
            "SELECT id, owner_id, parent_id, kind, name, size, sha256, mime, version, \
             created_at, updated_at, edited_on FROM nodes \
             WHERE owner_id = $1 AND id = ANY($2) AND ",
            visible!("nodes")
        ))
        .bind(owner)
        .bind(&ids)
        .fetch_all(self.pool())
        .await?;
        let cursor = latest.last().map_or(since, |(_, seq, _)| *seq);
        let changes = latest
            .into_iter()
            .map(|(id, seq, device_id)| {
                let node = nodes
                    .iter()
                    .position(|node| node.id == id)
                    .map(|at| nodes.swap_remove(at));
                NodeChange {
                    seq,
                    id,
                    node,
                    device_id,
                }
            })
            .collect();
        Ok(ChangeBatch {
            changes,
            cursor,
            more,
        })
    }

    /// Drops the entries of each node but its latest; returns how many were dropped.
    ///
    /// Reading the journal only ever uses a node's latest entry, so this changes no answer and
    /// clients can still catch up from any cursor.
    pub async fn compact_changes(&self) -> Result<u64, DbError> {
        let dropped = sqlx::query(
            "DELETE FROM node_changes c WHERE EXISTS ( \
                 SELECT 1 FROM node_changes later \
                 WHERE later.owner_id = c.owner_id AND later.node_id = c.node_id \
                 AND later.seq > c.seq \
             )",
        )
        .execute(self.pool())
        .await?;
        Ok(dropped.rows_affected())
    }

    /// Drops journal entries recorded before `before`; returns how many were dropped.
    ///
    /// Clients whose cursor is older than what was dropped must read the whole tree again.
    pub async fn prune_changes(&self, before: OffsetDateTime) -> Result<u64, DbError> {
        let pruned = sqlx::query_scalar::<_, i64>(
            "WITH dropped AS ( \
                 DELETE FROM node_changes WHERE changed_at < $1 RETURNING owner_id, seq \
             ), marks AS ( \
                 INSERT INTO node_changes_pruned (owner_id, through) \
                 SELECT owner_id, max(seq) FROM dropped GROUP BY owner_id \
                 ON CONFLICT (owner_id) DO UPDATE \
                 SET through = GREATEST(node_changes_pruned.through, EXCLUDED.through) \
             ) \
             SELECT count(*) FROM dropped",
        )
        .bind(before)
        .fetch_one(self.pool())
        .await?;
        Ok(u64::try_from(pruned).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use crate::{NewContent, NewNode, NodeKind, NodeUpdate};

    use super::*;

    async fn folder(db: &Db, owner: UserId, name: &str) -> Node {
        let new = NewNode {
            id: NodeId::new(),
            parent_id: None,
            kind: NodeKind::Folder,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: crate::FOLDER_MIME.to_owned(),
        };
        db.create_node(owner, new).await.expect("folder")
    }

    async fn file(db: &Db, owner: UserId, parent: Option<NodeId>, name: &str) -> Node {
        let content = NewContent {
            size: 1,
            sha256: vec![1],
            mime: "text/plain".to_owned(),
            modified_at: None,
        };
        db.store_file(owner, parent, name, content)
            .await
            .expect("file")
            .into_node()
    }

    async fn cursor(db: &Db, owner: UserId) -> i64 {
        db.changes(owner, None, 100).await.expect("cursor").cursor
    }

    fn ids(batch: &ChangeBatch) -> Vec<NodeId> {
        batch.changes.iter().map(|change| change.id).collect()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_node_changed_twice_is_listed_once_with_its_current_state(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(crate::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let start = cursor(&db, alice).await;
        let node = file(&db, alice, None, "a.txt").await;
        let update = NodeUpdate {
            name: Some("b.txt".to_owned()),
            ..NodeUpdate::default()
        };
        db.update_node(alice, node.id, update)
            .await
            .expect("rename");
        let batch = db.changes(alice, Some(start), 100).await.expect("changes");
        assert_eq!(ids(&batch), [node.id]);
        let current = batch.changes[0].node.as_ref().expect("live");
        assert_eq!(current.name, "b.txt");
        assert!(!batch.more);
        assert_eq!(batch.cursor, batch.changes[0].seq);
        assert_eq!(cursor(&db, alice).await, batch.cursor);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn binned_and_deleted_nodes_read_as_deleted(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(crate::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let dir = folder(&db, alice, "dir").await;
        let inner = file(&db, alice, Some(dir.id), "a.txt").await;
        let start = cursor(&db, alice).await;
        db.trash_node(alice, dir.id).await.expect("trash");
        let batch = db.changes(alice, Some(start), 100).await.expect("changes");
        let changed = ids(&batch);
        assert_eq!(changed.len(), 2);
        assert!(changed.contains(&dir.id) && changed.contains(&inner.id));
        assert!(batch.changes.iter().all(|change| change.node.is_none()));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn changes_are_paged_and_scoped_to_their_owner(pool: PgPool) {
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
        let start = cursor(&db, alice).await;
        let first = file(&db, alice, None, "1.txt").await;
        file(&db, bob, None, "bob.txt").await;
        let second = file(&db, alice, None, "2.txt").await;
        let page = db.changes(alice, Some(start), 1).await.expect("page");
        assert_eq!((ids(&page), page.more), (vec![first.id], true));
        let next = db.changes(alice, Some(page.cursor), 1).await.expect("next");
        assert_eq!((ids(&next), next.more), (vec![second.id], false));
        let empty = db
            .changes(alice, Some(next.cursor), 1)
            .await
            .expect("empty");
        assert_eq!((empty.changes.len(), empty.cursor), (0, next.cursor));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn compacting_keeps_the_latest_entry_of_each_node(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(crate::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let start = cursor(&db, alice).await;
        let renamed = file(&db, alice, None, "a.txt").await;
        let kept = file(&db, alice, None, "b.txt").await;
        let middle = cursor(&db, alice).await;
        let update = NodeUpdate {
            name: Some("c.txt".to_owned()),
            ..NodeUpdate::default()
        };
        db.update_node(alice, renamed.id, update)
            .await
            .expect("rename");
        let before = db.changes(alice, Some(start), 100).await.expect("before");
        let since_middle = db.changes(alice, Some(middle), 100).await.expect("middle");

        let dropped = db.compact_changes().await.expect("compact");

        assert!(dropped > 0);
        let after = db.changes(alice, Some(start), 100).await.expect("after");
        assert_eq!(ids(&after), ids(&before));
        assert_eq!(after.cursor, before.cursor);
        assert_eq!(
            ids(&db.changes(alice, Some(middle), 100).await.expect("middle")),
            ids(&since_middle)
        );
        assert!(ids(&after).contains(&kept.id));
        assert_eq!(db.compact_changes().await.expect("again"), 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reading_from_before_a_prune_is_refused(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(crate::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let start = cursor(&db, alice).await;
        file(&db, alice, None, "a.txt").await;
        let pruned = db
            .prune_changes(OffsetDateTime::now_utc() + time::Duration::seconds(1))
            .await
            .expect("prune");
        assert!(pruned > 0);
        assert!(matches!(
            db.changes(alice, Some(start), 100).await,
            Err(DbError::Pruned)
        ));
        let end = cursor(&db, alice).await;
        assert!(end > start);
        let batch = db.changes(alice, Some(end), 100).await.expect("from end");
        assert!(batch.changes.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_change_names_the_device_that_made_it_last(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = db
            .create_user(crate::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let start = cursor(&db, alice).await;
        let laptop = Uuid::now_v7();
        let first = folder(&db.on_device(Some(laptop)), alice, "a").await;
        let second = folder(&db.on_device(Some(laptop)), alice, "b").await;
        let update = NodeUpdate {
            name: Some("c".to_owned()),
            parent_id: crate::Patch::Keep,
        };
        db.update_node(alice, second.id, update)
            .await
            .expect("rename");

        let batch = db.changes(alice, Some(start), 100).await.expect("changes");

        let devices: Vec<(NodeId, Option<Uuid>)> = batch
            .changes
            .iter()
            .map(|change| (change.id, change.device_id))
            .collect();
        assert_eq!(devices, [(first.id, Some(laptop)), (second.id, None)]);
    }
}
