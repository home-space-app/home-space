//! Which protected folder holds a node.

use uuid::Uuid;

use crate::{Db, DbError, NodeId, Tx, UserId};

impl Db {
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
}

impl Tx {
    /// The protected folder that is `node` or holds it; `None` for the root and outside every
    /// protected folder.
    pub async fn protected_folder_of(
        &mut self,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, DbError> {
        let Some(node) = node else {
            return Ok(None);
        };
        let found = sqlx::query_scalar(
            "WITH RECURSIVE up AS ( \
                 SELECT id, parent_id FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id, n.parent_id FROM nodes n JOIN up ON n.id = up.parent_id \
             ) SELECT p.node_id FROM protected_folders p JOIN up ON p.node_id = up.id LIMIT 1",
        )
        .bind(node)
        .bind(self.owner)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(found)
    }
}
