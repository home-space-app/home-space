//! Shares of a user's files and folders, and the links that reach them.

use std::collections::HashMap;

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, Node, NodeId, NodeKind, SharePermission, UserId};

macro_rules! share_columns {
    () => {
        concat!(
            "s.id, s.owner_id, u.username AS owner_name, l.token, l.password_hash, \
             l.password_version, l.permission, s.expires_at, s.created_at, \
             (SELECT count(*) FROM share_items si JOIN nodes n ON n.id = si.node_id \
              WHERE si.share_id = s.id AND ",
            visible!("n"),
            ") AS item_count FROM shares s JOIN share_links l ON l.share_id = s.id \
             JOIN users u ON u.id = s.owner_id"
        )
    };
}

macro_rules! item_columns {
    () => {
        "n.id, n.owner_id, n.parent_id, n.kind, n.name, n.size, n.sha256, n.mime, n.version, \
         n.created_at, n.updated_at, n.edited_on"
    };
}

/// How deep below a shared folder a node may be found. No real tree comes near it; it bounds
/// the walk up the tree.
const MAX_SHARE_DEPTH: i32 = 1024;

/// A share reached through a link, with the name of its owner.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ShareLink {
    pub id: Uuid,
    pub owner_id: UserId,
    pub owner_name: String,
    pub token: String,
    pub password_hash: Option<String>,
    pub password_version: i32,
    pub permission: SharePermission,
    pub expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    /// Shared items that are not in the recycle bin.
    pub item_count: i64,
}

impl ShareLink {
    /// Whether the link stopped working at `now`.
    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }
}

/// Values for a share link to insert.
#[derive(Clone, Copy, Debug)]
pub struct NewShareLink<'a> {
    pub id: Uuid,
    pub token: &'a str,
    pub password_hash: Option<&'a str>,
    pub permission: SharePermission,
    pub expires_at: Option<OffsetDateTime>,
}

/// Changes to a share link; `None` leaves a value as it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShareLinkUpdate<'a> {
    /// `Some(None)` removes the password.
    pub password_hash: Option<Option<&'a str>>,
    /// `Some(None)` makes the link never expire.
    pub expires_at: Option<Option<OffsetDateTime>>,
}

#[derive(FromRow)]
struct ItemRow {
    share_id: Uuid,
    #[sqlx(flatten)]
    node: Node,
}

impl Db {
    /// Makes a share link to `node_ids`, which must all be live nodes of `owner`; `NotFound`
    /// when one is not.
    pub async fn create_share_link(
        &self,
        owner: UserId,
        new: NewShareLink<'_>,
        node_ids: &[NodeId],
    ) -> Result<ShareLink, DbError> {
        let mut tx = self.pool().begin().await?;
        let found: i64 = sqlx::query_scalar(concat!(
            "SELECT count(*) FROM nodes WHERE owner_id = $1 AND id = ANY($2) AND ",
            visible!("nodes")
        ))
        .bind(owner)
        .bind(node_ids)
        .fetch_one(&mut *tx)
        .await?;
        if usize::try_from(found).ok() != Some(node_ids.len()) {
            return Err(DbError::NotFound);
        }
        sqlx::query("INSERT INTO shares (id, owner_id, expires_at) VALUES ($1, $2, $3)")
            .bind(new.id)
            .bind(owner)
            .bind(new.expires_at)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO share_links (share_id, token, password_hash, permission) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(new.id)
        .bind(new.token)
        .bind(new.password_hash)
        .bind(new.permission)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO share_items (share_id, node_id) SELECT $1, unnest($2::uuid[])")
            .bind(new.id)
            .bind(node_ids)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.share_link(owner, new.id).await
    }

    /// A share link of `owner`; `NotFound` for missing or foreign ones.
    pub async fn share_link(&self, owner: UserId, id: Uuid) -> Result<ShareLink, DbError> {
        let share = sqlx::query_as::<_, ShareLink>(concat!(
            "SELECT ",
            share_columns!(),
            " WHERE s.id = $1 AND s.owner_id = $2"
        ))
        .bind(id)
        .bind(owner)
        .fetch_one(self.pool())
        .await?;
        Ok(share)
    }

    /// The share links of `owner`, newest first, only those including `node_id` when given.
    pub async fn share_links(
        &self,
        owner: UserId,
        node_id: Option<NodeId>,
    ) -> Result<Vec<ShareLink>, DbError> {
        let shares = sqlx::query_as::<_, ShareLink>(concat!(
            "SELECT ",
            share_columns!(),
            " WHERE s.owner_id = $1 AND ($2::uuid IS NULL OR EXISTS \
               (SELECT 1 FROM share_items si WHERE si.share_id = s.id AND si.node_id = $2)) \
             ORDER BY s.created_at DESC, s.id"
        ))
        .bind(owner)
        .bind(node_id)
        .fetch_all(self.pool())
        .await?;
        Ok(shares)
    }

    /// The live shared items of each of `share_ids`, folders first, then by name.
    pub async fn share_items(
        &self,
        share_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<Node>>, DbError> {
        let rows = sqlx::query_as::<_, ItemRow>(concat!(
            "SELECT si.share_id, ",
            item_columns!(),
            " FROM share_items si JOIN nodes n ON n.id = si.node_id \
             WHERE si.share_id = ANY($1) AND ",
            visible!("n"),
            " ORDER BY n.kind = 'file', n.name"
        ))
        .bind(share_ids)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().fold(HashMap::new(), |mut items, row| {
            items
                .entry(row.share_id)
                .or_insert_with(Vec::new)
                .push(row.node);
            items
        }))
    }

    /// Changes a share link of `owner`. A new or removed password raises its version.
    pub async fn update_share_link(
        &self,
        owner: UserId,
        id: Uuid,
        update: ShareLinkUpdate<'_>,
    ) -> Result<ShareLink, DbError> {
        let mut tx = self.pool().begin().await?;
        let result = sqlx::query(
            "UPDATE shares SET \
               expires_at = CASE WHEN $3 THEN $4 ELSE expires_at END, \
               updated_at = now() \
             WHERE id = $1 AND owner_id = $2",
        )
        .bind(id)
        .bind(owner)
        .bind(update.expires_at.is_some())
        .bind(update.expires_at.flatten())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        if let Some(password_hash) = update.password_hash {
            sqlx::query(
                "UPDATE share_links SET password_hash = $2, \
                   password_version = password_version + 1 \
                 WHERE share_id = $1",
            )
            .bind(id)
            .bind(password_hash)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.share_link(owner, id).await
    }

    /// Removes a share of `owner` with its link; the link stops working at once.
    pub async fn delete_share(&self, owner: UserId, id: Uuid) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM shares WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(owner)
            .execute(self.pool())
            .await?;
        if result.rows_affected() == 0 {
            Err(DbError::NotFound)
        } else {
            Ok(())
        }
    }

    /// The share link with `token`, whoever owns it.
    pub async fn share_by_token(&self, token: &str) -> Result<ShareLink, DbError> {
        let share = sqlx::query_as::<_, ShareLink>(concat!(
            "SELECT ",
            share_columns!(),
            " WHERE l.token = $1"
        ))
        .bind(token)
        .fetch_one(self.pool())
        .await?;
        Ok(share)
    }

    /// A live node reachable through `share`: a shared item or something inside a shared
    /// folder, with nothing on the way up in the recycle bin. `NotFound` for anything else.
    pub async fn shared_node(&self, share: &ShareLink, id: NodeId) -> Result<Node, DbError> {
        let reachable: bool = sqlx::query_scalar(concat!(
            "WITH RECURSIVE chain AS ( \
                 SELECT n.id, n.parent_id, 0 AS depth FROM nodes n \
                 WHERE n.id = $2 AND n.owner_id = $3 AND ",
            visible!("n"),
            " UNION ALL \
                 SELECT p.id, p.parent_id, c.depth + 1 FROM nodes p \
                 JOIN chain c ON p.id = c.parent_id \
                 WHERE p.owner_id = $3 AND c.depth < $4 AND ",
            visible!("p"),
            " AND NOT EXISTS (SELECT 1 FROM share_items si \
                               WHERE si.share_id = $1 AND si.node_id = c.id) \
             ) \
             SELECT EXISTS (SELECT 1 FROM chain c \
                            JOIN share_items si ON si.node_id = c.id AND si.share_id = $1)"
        ))
        .bind(share.id)
        .bind(id)
        .bind(share.owner_id)
        .bind(MAX_SHARE_DEPTH)
        .fetch_one(self.pool())
        .await?;
        if reachable {
            self.get_node(share.owner_id, id).await
        } else {
            Err(DbError::NotFound)
        }
    }

    /// The children of a folder reachable through `share`; `NotFound` when `parent_id` is not
    /// such a folder.
    pub async fn shared_children(
        &self,
        share: &ShareLink,
        parent_id: NodeId,
    ) -> Result<Vec<Node>, DbError> {
        let parent = self.shared_node(share, parent_id).await?;
        if parent.kind == NodeKind::Folder {
            self.list_children(share.owner_id, Some(parent_id)).await
        } else {
            Err(DbError::NotFound)
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::NewNode;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789a";

    async fn user(db: &Db, name: &str) -> UserId {
        db.create_user(name, "hash").await.expect("user").id
    }

    async fn node(db: &Db, owner: UserId, parent: Option<NodeId>, kind: NodeKind) -> Node {
        db.create_node(
            owner,
            NewNode {
                id: NodeId::new(),
                parent_id: parent,
                kind,
                name: Uuid::now_v7().to_string(),
                size: 0,
                sha256: None,
                mime: String::new(),
            },
        )
        .await
        .expect("node")
    }

    fn new_share(token: &str) -> NewShareLink<'_> {
        NewShareLink {
            id: Uuid::now_v7(),
            token,
            password_hash: None,
            permission: SharePermission::Read,
            expires_at: None,
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_node_inside_a_shared_folder_is_reachable(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let folder = node(&db, alice, None, NodeKind::Folder).await;
        let inner = node(&db, alice, Some(folder.id), NodeKind::Folder).await;
        let file = node(&db, alice, Some(inner.id), NodeKind::File).await;
        let share = db
            .create_share_link(alice, new_share(TOKEN), &[folder.id])
            .await
            .expect("share");
        assert_eq!(
            db.shared_node(&share, file.id).await.expect("reachable").id,
            file.id
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_sibling_of_a_shared_folder_is_not_reachable(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let parent = node(&db, alice, None, NodeKind::Folder).await;
        let shared = node(&db, alice, Some(parent.id), NodeKind::Folder).await;
        let sibling = node(&db, alice, Some(parent.id), NodeKind::File).await;
        let share = db
            .create_share_link(alice, new_share(TOKEN), &[shared.id])
            .await
            .expect("share");
        assert!(matches!(
            db.shared_node(&share, sibling.id).await,
            Err(DbError::NotFound)
        ));
        assert!(matches!(
            db.shared_node(&share, parent.id).await,
            Err(DbError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn another_users_node_cannot_be_shared(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let bob = user(&db, "bob").await;
        let theirs = node(&db, bob, None, NodeKind::File).await;
        assert!(matches!(
            db.create_share_link(alice, new_share(TOKEN), &[theirs.id])
                .await,
            Err(DbError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn changing_the_password_raises_its_version(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        let share = db
            .create_share_link(alice, new_share(TOKEN), &[file.id])
            .await
            .expect("share");
        let updated = db
            .update_share_link(
                alice,
                share.id,
                ShareLinkUpdate {
                    password_hash: Some(Some("phc")),
                    expires_at: None,
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.password_hash.as_deref(), Some("phc"));
        assert_eq!(updated.password_version, share.password_version + 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_new_link_only_reads(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        let share = db
            .create_share_link(alice, new_share(TOKEN), &[file.id])
            .await
            .expect("share");
        assert_eq!(share.permission, SharePermission::Read);
        assert_eq!(
            db.share_by_token(TOKEN).await.expect("by token").id,
            share.id
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn changing_the_expiry_keeps_the_password(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        let new = NewShareLink {
            password_hash: Some("phc"),
            ..new_share(TOKEN)
        };
        let share = db
            .create_share_link(alice, new, &[file.id])
            .await
            .expect("share");
        let expires_at = OffsetDateTime::now_utc() + time::Duration::days(1);
        let updated = db
            .update_share_link(
                alice,
                share.id,
                ShareLinkUpdate {
                    password_hash: None,
                    expires_at: Some(Some(expires_at)),
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.password_hash.as_deref(), Some("phc"));
        assert_eq!(updated.password_version, share.password_version);
        assert!(updated.expires_at.is_some());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deleting_a_share_removes_its_link(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        let share = db
            .create_share_link(alice, new_share(TOKEN), &[file.id])
            .await
            .expect("share");
        db.delete_share(alice, share.id).await.expect("delete");
        assert!(matches!(
            db.share_by_token(TOKEN).await,
            Err(DbError::NotFound)
        ));
    }
}
