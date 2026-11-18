use std::collections::HashMap;

use sqlx::FromRow;
use uuid::Uuid;

use crate::{Db, DbError, Node, NodeId, UserId};

macro_rules! prefixed_node_columns {
    () => {
        "n.id, n.owner_id, n.parent_id, n.kind, n.name, n.size, n.sha256, n.mime, \
         n.version, n.created_at, n.updated_at, n.edited_on"
    };
}

/// A search match with its relevance.
#[derive(Clone, Debug, FromRow)]
pub struct RankedNode {
    #[sqlx(flatten)]
    pub node: Node,
    pub rank: f32,
}

/// A file together with the content-index state recorded for it.
#[derive(Clone, Debug, FromRow)]
pub struct ContentState {
    #[sqlx(flatten)]
    pub node: Node,
    pub content_provider: Option<String>,
    pub content_sha256: Option<Vec<u8>>,
}

impl Db {
    /// Inserts or replaces the name terms of a node's index entry.
    pub async fn upsert_name_entry(
        &self,
        owner: UserId,
        id: NodeId,
        terms: &str,
    ) -> Result<(), DbError> {
        sqlx::query(concat!(
            "INSERT INTO search_entries (node_id, owner_id, name_tsv) \
             SELECT n.id, n.owner_id, to_tsvector('simple', $3) \
             FROM nodes n WHERE n.id = $1 AND n.owner_id = $2 AND ",
            visible!("n"),
            " ON CONFLICT (node_id) DO UPDATE \
             SET name_tsv = EXCLUDED.name_tsv, indexed_at = now()"
        ))
        .bind(id)
        .bind(owner)
        .bind(terms)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Records the extracted content terms of a file; `None` terms clear the content.
    ///
    /// `NotFound` when the node has no index entry.
    pub async fn set_content_entry(
        &self,
        owner: UserId,
        id: NodeId,
        terms: Option<&str>,
        provider: Option<&str>,
        sha256: Option<&[u8]>,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE search_entries \
             SET content_tsv = to_tsvector('simple', $3), content_provider = $4, \
                 content_sha256 = $5, indexed_at = now() \
             WHERE node_id = $1 AND owner_id = $2",
        )
        .bind(id)
        .bind(owner)
        .bind(terms)
        .bind(provider)
        .bind(sha256)
        .execute(self.pool())
        .await?;
        if updated.rows_affected() == 0 {
            Err(DbError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Finds the owner's nodes matching a `tsquery` expression, best matches first.
    pub async fn search_nodes(
        &self,
        owner: UserId,
        tsquery: &str,
        limit: i64,
    ) -> Result<Vec<RankedNode>, DbError> {
        let hits = sqlx::query_as::<_, RankedNode>(concat!(
            "SELECT ",
            prefixed_node_columns!(),
            ", ts_rank(e.tsv, q.query) AS rank \
             FROM search_entries e \
             JOIN nodes n ON n.id = e.node_id \
             CROSS JOIN to_tsquery('simple', $2) AS q(query) \
             WHERE e.owner_id = $1 AND n.owner_id = $1 AND e.tsv @@ q.query AND ",
            visible!("n"),
            " ORDER BY rank DESC, n.kind, n.name \
             LIMIT $3"
        ))
        .bind(owner)
        .bind(tsquery)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(hits)
    }

    /// Names of the ancestors of each node, root first; root-level nodes map to nothing.
    pub async fn list_locations(
        &self,
        owner: UserId,
        ids: &[NodeId],
    ) -> Result<HashMap<NodeId, Vec<String>>, DbError> {
        let ids = ids.iter().map(|id| id.0).collect::<Vec<Uuid>>();
        let rows = sqlx::query_as::<_, (NodeId, String)>(
            "WITH RECURSIVE chain AS ( \
                 SELECT n.id AS hit, p.parent_id, p.name, 1 AS depth \
                 FROM nodes n JOIN nodes p ON p.id = n.parent_id AND p.owner_id = $2 \
                 WHERE n.id = ANY($1) AND n.owner_id = $2 \
                 UNION ALL \
                 SELECT c.hit, p.parent_id, p.name, c.depth + 1 \
                 FROM chain c JOIN nodes p ON p.id = c.parent_id AND p.owner_id = $2 \
             ) SELECT hit, name FROM chain ORDER BY hit, depth DESC",
        )
        .bind(ids)
        .bind(owner)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .fold(HashMap::new(), |mut locations, (hit, name)| {
                locations.entry(hit).or_default().push(name);
                locations
            }))
    }

    /// Nodes of any owner that have no index entry yet.
    pub async fn nodes_missing_entries(&self, limit: i64) -> Result<Vec<Node>, DbError> {
        let nodes = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            prefixed_node_columns!(),
            " FROM nodes n \
             LEFT JOIN search_entries e ON e.node_id = n.id \
             WHERE e.node_id IS NULL AND ",
            visible!("n"),
            " LIMIT $1"
        ))
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(nodes)
    }

    /// Pages through indexed files in id order with their content-index state.
    pub async fn list_content_states(
        &self,
        after: Option<NodeId>,
        limit: i64,
    ) -> Result<Vec<ContentState>, DbError> {
        let states = sqlx::query_as::<_, ContentState>(concat!(
            "SELECT ",
            prefixed_node_columns!(),
            ", e.content_provider, e.content_sha256 \
             FROM nodes n \
             JOIN search_entries e ON e.node_id = n.id \
             WHERE n.kind = 'file' AND ($1::uuid IS NULL OR n.id > $1) AND ",
            visible!("n"),
            " ORDER BY n.id \
             LIMIT $2"
        ))
        .bind(after)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(states)
    }

    /// Removes every index entry so the index can be rebuilt.
    pub async fn clear_search_entries(&self) -> Result<(), DbError> {
        sqlx::query("TRUNCATE search_entries")
            .execute(self.pool())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use crate::{NewNode, NodeKind};

    use super::*;

    async fn user(db: &Db, name: &str) -> UserId {
        db.create_user(crate::TenantId::DEFAULT, name, "x")
            .await
            .expect("user")
            .id
    }

    async fn add(db: &Db, owner: UserId, parent: Option<NodeId>, name: &str) -> Node {
        let new = NewNode {
            id: NodeId::new(),
            parent_id: parent,
            kind: NodeKind::File,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: "text/plain".to_owned(),
        };
        db.create_node(owner, new).await.expect("node")
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn matches_prefixes_of_indexed_terms(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let node = add(&db, alice, None, "holiday.jpg").await;
        db.upsert_name_entry(alice, node.id, "holiday jpg")
            .await
            .expect("index");
        let hits = db.search_nodes(alice, "holi:*", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node.id, node.id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn other_users_nodes_are_never_returned(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let bob = user(&db, "bob").await;
        let node = add(&db, alice, None, "secret.txt").await;
        db.upsert_name_entry(alice, node.id, "secret txt")
            .await
            .expect("index");
        assert!(
            db.search_nodes(bob, "secret:*", 10)
                .await
                .expect("search")
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deleting_a_node_removes_its_entry(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let node = add(&db, alice, None, "a.txt").await;
        db.upsert_name_entry(alice, node.id, "a txt")
            .await
            .expect("index");
        db.delete_node(alice, node.id).await.expect("delete");
        assert!(
            db.search_nodes(alice, "a:*", 10)
                .await
                .expect("search")
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn name_matches_rank_above_content_matches(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let named = add(&db, alice, None, "budget.txt").await;
        let mentioning = add(&db, alice, None, "notes.txt").await;
        db.upsert_name_entry(alice, named.id, "budget txt")
            .await
            .expect("index named");
        db.upsert_name_entry(alice, mentioning.id, "notes txt")
            .await
            .expect("index mentioning");
        db.set_content_entry(alice, mentioning.id, Some("budget"), Some("test@1"), None)
            .await
            .expect("content");
        let hits = db
            .search_nodes(alice, "budget:*", 10)
            .await
            .expect("search");
        let ids = hits.iter().map(|hit| hit.node.id).collect::<Vec<_>>();
        assert_eq!(ids, [named.id, mentioning.id]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn locations_list_ancestors_root_first(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let new_folder = |parent, name: &str| NewNode {
            id: NodeId::new(),
            parent_id: parent,
            kind: NodeKind::Folder,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: "text/plain".to_owned(),
        };
        let a = db
            .create_node(alice, new_folder(None, "a"))
            .await
            .expect("a");
        let b = db
            .create_node(alice, new_folder(Some(a.id), "b"))
            .await
            .expect("b");
        let deep = add(&db, alice, Some(b.id), "f.txt").await;
        let top = add(&db, alice, None, "g.txt").await;
        let locations = db
            .list_locations(alice, &[deep.id, top.id])
            .await
            .expect("locations");
        assert_eq!(locations[&deep.id], ["a", "b"]);
        assert!(!locations.contains_key(&top.id));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn missing_entries_lists_only_unindexed_nodes(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let indexed = add(&db, alice, None, "a.txt").await;
        let pending = add(&db, alice, None, "b.txt").await;
        db.upsert_name_entry(alice, indexed.id, "a txt")
            .await
            .expect("index");
        let missing = db.nodes_missing_entries(10).await.expect("missing");
        let mut names = missing
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        // The recycle bin folder is never indexed.
        assert_eq!(names, ["Lost & Found", pending.name.as_str()]);
    }
}
