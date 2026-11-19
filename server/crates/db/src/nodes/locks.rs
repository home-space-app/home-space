//! Reading nodes inside a [`Tx`], locking the rows a change will write.

use uuid::Uuid;

use crate::{DbError, Node, NodeId, NodeKind, Tx};

impl Tx {
    /// Locks a live node of the owner; `NotFound` when there is none.
    pub async fn lock_node(&mut self, id: NodeId) -> Result<Node, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            node_columns!(),
            " FROM nodes WHERE id = $1 AND owner_id = $2 AND ",
            visible!("nodes"),
            " FOR UPDATE"
        ))
        .bind(id)
        .bind(self.owner)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(node)
    }

    /// Locks a live folder of the owner; `NotFound` when there is none or it is a file.
    pub async fn lock_folder(&mut self, id: NodeId) -> Result<Node, DbError> {
        let node = self.lock_node(id).await?;
        if node.kind == NodeKind::Folder {
            Ok(node)
        } else {
            Err(DbError::NotFound)
        }
    }

    /// Locks the live nodes `ids`, in the order of their ids; `NotFound` unless every one of
    /// them is a live node of the owner. `ids` must not repeat.
    pub async fn lock_nodes(&mut self, ids: &[NodeId]) -> Result<Vec<Node>, DbError> {
        let found = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            node_columns!(),
            " FROM nodes WHERE id = ANY($1) AND owner_id = $2 AND ",
            visible!("nodes"),
            " ORDER BY id FOR UPDATE"
        ))
        .bind(raw(ids))
        .bind(self.owner)
        .fetch_all(&mut *self.conn)
        .await?;
        if found.len() == ids.len() {
            Ok(found)
        } else {
            Err(DbError::NotFound)
        }
    }

    /// The ones of `ids` that sit somewhere below another one of them.
    pub async fn nested_in(&mut self, ids: &[NodeId]) -> Result<Vec<NodeId>, DbError> {
        let nested = sqlx::query_scalar::<_, NodeId>(
            "WITH RECURSIVE chain AS ( \
                 SELECT id, parent_id FROM nodes WHERE id = ANY($1) AND owner_id = $2 \
                 UNION ALL \
                 SELECT c.id, n.parent_id FROM chain c JOIN nodes n ON n.id = c.parent_id \
             ) SELECT DISTINCT id FROM chain WHERE parent_id = ANY($1)",
        )
        .bind(raw(ids))
        .bind(self.owner)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(nested)
    }

    /// Locks the children of `parent`, binned ones included, folders first, then by name.
    pub async fn lock_children(&mut self, parent: NodeId) -> Result<Vec<Node>, DbError> {
        let nodes = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            node_columns!(),
            " FROM nodes WHERE owner_id = $1 AND parent_id = $2 ORDER BY kind, name FOR UPDATE"
        ))
        .bind(self.owner)
        .bind(parent)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(nodes)
    }

    /// Locks the live node of `kind` named `name` in `parent` (the root when `None`), other
    /// than `exclude`.
    pub async fn find_sibling(
        &mut self,
        parent: Option<NodeId>,
        kind: NodeKind,
        name: &str,
        exclude: Option<NodeId>,
    ) -> Result<Option<Node>, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            node_columns!(),
            " FROM nodes \
             WHERE owner_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND kind = $3 \
             AND name = $4 AND id IS DISTINCT FROM $5 AND ",
            visible!("nodes"),
            " FOR UPDATE"
        ))
        .bind(self.owner)
        .bind(parent)
        .bind(kind)
        .bind(name)
        .bind(exclude)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(node)
    }

    /// `NotFound` unless `id` is a live folder of the owner; the root (`None`) always is.
    pub async fn require_folder(&mut self, id: Option<NodeId>) -> Result<(), DbError> {
        super::require_folder(&mut self.conn, self.owner, id).await
    }

    /// Whether `candidate` is `ancestor` or lies somewhere below it.
    pub async fn is_within(
        &mut self,
        candidate: NodeId,
        ancestor: NodeId,
    ) -> Result<bool, DbError> {
        let within = sqlx::query_scalar::<_, bool>(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
             ) SELECT EXISTS (SELECT 1 FROM subtree WHERE id = $3)",
        )
        .bind(ancestor)
        .bind(self.owner)
        .bind(candidate)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(within)
    }
}

fn raw(ids: &[NodeId]) -> Vec<Uuid> {
    ids.iter().map(|id| id.0).collect()
}
