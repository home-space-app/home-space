//! Shares of a user's files and folders: through a link, or with named users.

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

macro_rules! owned_columns {
    () => {
        "s.id, l.token, l.password_hash IS NOT NULL AS has_password, \
         COALESCE(l.permission, 'read') AS permission, s.expires_at, s.created_at \
         FROM shares s LEFT JOIN share_links l ON l.share_id = s.id"
    };
}

macro_rules! received_columns {
    () => {
        "s.id, s.owner_id, u.username AS owner_name, s.expires_at, su.created_at AS shared_at \
         FROM share_users su JOIN shares s ON s.id = su.share_id \
         JOIN users u ON u.id = s.owner_id"
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

    /// What the link reaches.
    pub fn root(&self) -> SharedRoot {
        SharedRoot {
            id: self.id,
            owner_id: self.owner_id,
        }
    }
}

/// A share as far as reaching its nodes goes: which share, and whose tree it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedRoot {
    pub id: Uuid,
    pub owner_id: UserId,
}

/// A share another user made for the reader, with the name of its owner.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ReceivedShare {
    pub id: Uuid,
    pub owner_id: UserId,
    pub owner_name: String,
    pub expires_at: Option<OffsetDateTime>,
    /// When the reader was added to the share.
    pub shared_at: OffsetDateTime,
}

impl ReceivedShare {
    /// Whether the share stopped working at `now`.
    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    /// What the share reaches.
    pub fn root(&self) -> SharedRoot {
        SharedRoot {
            id: self.id,
            owner_id: self.owner_id,
        }
    }
}

/// A share as its owner sees it: through a link, or with named users.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct OwnedShare {
    pub id: Uuid,
    /// `None` for a share with named users.
    pub token: Option<String>,
    pub has_password: bool,
    pub permission: SharePermission,
    pub expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

impl From<ShareLink> for OwnedShare {
    fn from(link: ShareLink) -> Self {
        Self {
            id: link.id,
            has_password: link.password_hash.is_some(),
            token: Some(link.token),
            permission: link.permission,
            expires_at: link.expires_at,
            created_at: link.created_at,
        }
    }
}

/// A user a share is for.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ShareRecipient {
    pub share_id: Uuid,
    pub user_id: UserId,
    pub username: String,
    pub permission: SharePermission,
    /// When the user was added to the share.
    pub created_at: OffsetDateTime,
}

/// Values for a share with named users to insert.
#[derive(Clone, Copy, Debug)]
pub struct NewUserShare {
    pub id: Uuid,
    pub expires_at: Option<OffsetDateTime>,
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

/// Changes to a share; `None` leaves a value as it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShareUpdate<'a> {
    /// The password of a link; `Some(None)` removes it. Ignored for a share without a link.
    pub password_hash: Option<Option<&'a str>>,
    /// `Some(None)` makes the share never expire.
    pub expires_at: Option<Option<OffsetDateTime>>,
    /// Everyone the share is for from now on; users already in it keep when they were added.
    pub recipients: Option<&'a [UserId]>,
}

/// Adds share `id` of `owner` for `node_ids`, which must all be live nodes of `owner`.
async fn insert_share(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: UserId,
    id: Uuid,
    expires_at: Option<OffsetDateTime>,
    node_ids: &[NodeId],
) -> Result<(), DbError> {
    let found: i64 = sqlx::query_scalar(concat!(
        "SELECT count(*) FROM nodes WHERE owner_id = $1 AND id = ANY($2) AND ",
        visible!("nodes")
    ))
    .bind(owner)
    .bind(node_ids)
    .fetch_one(&mut **tx)
    .await?;
    if usize::try_from(found).ok() != Some(node_ids.len()) {
        return Err(DbError::NotFound);
    }
    sqlx::query("INSERT INTO shares (id, owner_id, expires_at) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(owner)
        .bind(expires_at)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO share_items (share_id, node_id) SELECT $1, unnest($2::uuid[])")
        .bind(id)
        .bind(node_ids)
        .execute(&mut **tx)
        .await?;
    Ok(())
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
        insert_share(&mut tx, owner, new.id, new.expires_at, node_ids).await?;
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
        tx.commit().await?;
        self.share_link(owner, new.id).await
    }

    /// Shares `node_ids`, which must all be live nodes of `owner`, with `user_ids`, who must
    /// all be other users; `NotFound` when one is not.
    pub async fn create_user_share(
        &self,
        owner: UserId,
        new: NewUserShare,
        node_ids: &[NodeId],
        user_ids: &[UserId],
    ) -> Result<OwnedShare, DbError> {
        if user_ids.contains(&owner) {
            return Err(DbError::NotFound);
        }
        let mut tx = self.pool().begin().await?;
        insert_share(&mut tx, owner, new.id, new.expires_at, node_ids).await?;
        sqlx::query("INSERT INTO share_users (share_id, user_id) SELECT $1, unnest($2::uuid[])")
            .bind(new.id)
            .bind(user_ids)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.owned_share(owner, new.id).await
    }

    /// A share of `owner`, with a link or not; `NotFound` for missing or foreign ones.
    pub async fn owned_share(&self, owner: UserId, id: Uuid) -> Result<OwnedShare, DbError> {
        let share = sqlx::query_as::<_, OwnedShare>(concat!(
            "SELECT ",
            owned_columns!(),
            " WHERE s.id = $1 AND s.owner_id = $2"
        ))
        .bind(id)
        .bind(owner)
        .fetch_one(self.pool())
        .await?;
        Ok(share)
    }

    /// The shares of `owner`, newest first, only those including `node_id` when given.
    pub async fn owned_shares(
        &self,
        owner: UserId,
        node_id: Option<NodeId>,
    ) -> Result<Vec<OwnedShare>, DbError> {
        let shares = sqlx::query_as::<_, OwnedShare>(concat!(
            "SELECT ",
            owned_columns!(),
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

    /// The users each of `share_ids` is for, by username.
    pub async fn share_recipients(
        &self,
        share_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<ShareRecipient>>, DbError> {
        let rows = sqlx::query_as::<_, ShareRecipient>(
            "SELECT su.share_id, su.user_id, u.username, su.permission, su.created_at \
             FROM share_users su JOIN users u ON u.id = su.user_id \
             WHERE su.share_id = ANY($1) ORDER BY u.username",
        )
        .bind(share_ids)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .fold(HashMap::new(), |mut recipients, row| {
                recipients
                    .entry(row.share_id)
                    .or_insert_with(Vec::new)
                    .push(row);
                recipients
            }))
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

    /// Changes a share of `owner`. A new or removed password raises its version. New
    /// recipients must be other users; `NotFound` when one is not.
    pub async fn update_share(
        &self,
        owner: UserId,
        id: Uuid,
        update: ShareUpdate<'_>,
    ) -> Result<OwnedShare, DbError> {
        if update
            .recipients
            .is_some_and(|recipients| recipients.contains(&owner))
        {
            return Err(DbError::NotFound);
        }
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
        if let Some(recipients) = update.recipients {
            sqlx::query("DELETE FROM share_users WHERE share_id = $1 AND user_id <> ALL($2)")
                .bind(id)
                .bind(recipients)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "INSERT INTO share_users (share_id, user_id) SELECT $1, unnest($2::uuid[]) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(recipients)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.owned_share(owner, id).await
    }

    /// Removes a share of `owner` with its link or recipients; it stops working at once.
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

    /// The shares other users made for `user` that have not expired at `now`, newest first.
    pub async fn received_shares(
        &self,
        user: UserId,
        now: OffsetDateTime,
    ) -> Result<Vec<ReceivedShare>, DbError> {
        let shares = sqlx::query_as::<_, ReceivedShare>(concat!(
            "SELECT ",
            received_columns!(),
            " WHERE su.user_id = $1 AND (s.expires_at IS NULL OR s.expires_at > $2) \
             ORDER BY su.created_at DESC, s.id"
        ))
        .bind(user)
        .bind(now)
        .fetch_all(self.pool())
        .await?;
        Ok(shares)
    }

    /// A share made for `user`, expired or not; `NotFound` when there is none.
    pub async fn received_share(&self, user: UserId, id: Uuid) -> Result<ReceivedShare, DbError> {
        let share = sqlx::query_as::<_, ReceivedShare>(concat!(
            "SELECT ",
            received_columns!(),
            " WHERE su.user_id = $1 AND s.id = $2"
        ))
        .bind(user)
        .bind(id)
        .fetch_one(self.pool())
        .await?;
        Ok(share)
    }

    /// A live node reachable through `share`: a shared item or something inside a shared
    /// folder, with nothing on the way up in the recycle bin. `NotFound` for anything else.
    pub async fn shared_node(&self, share: SharedRoot, id: NodeId) -> Result<Node, DbError> {
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
        share: SharedRoot,
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
        db.create_user(crate::TenantId::DEFAULT, name, "hash")
            .await
            .expect("user")
            .id
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
            db.shared_node(share.root(), file.id)
                .await
                .expect("reachable")
                .id,
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
            db.shared_node(share.root(), sibling.id).await,
            Err(DbError::NotFound)
        ));
        assert!(matches!(
            db.shared_node(share.root(), parent.id).await,
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
        db.update_share(
            alice,
            share.id,
            ShareUpdate {
                password_hash: Some(Some("phc")),
                ..ShareUpdate::default()
            },
        )
        .await
        .expect("update");
        let updated = db.share_by_token(TOKEN).await.expect("by token");
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
        db.update_share(
            alice,
            share.id,
            ShareUpdate {
                expires_at: Some(Some(expires_at)),
                ..ShareUpdate::default()
            },
        )
        .await
        .expect("update");
        let updated = db.share_by_token(TOKEN).await.expect("by token");
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

    fn new_user_share() -> NewUserShare {
        NewUserShare {
            id: Uuid::now_v7(),
            expires_at: None,
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_share_with_users_only_reads_and_has_no_link(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let bob = user(&db, "bob").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        let share = db
            .create_user_share(alice, new_user_share(), &[file.id], &[bob])
            .await
            .expect("share");
        assert_eq!(share.token, None);
        let recipients = db.share_recipients(&[share.id]).await.expect("recipients");
        let only = &recipients[&share.id];
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].user_id, bob);
        assert_eq!(only[0].username, "bob");
        assert_eq!(only[0].permission, SharePermission::Read);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_share_cannot_be_for_its_owner(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        assert!(matches!(
            db.create_user_share(alice, new_user_share(), &[file.id], &[alice])
                .await,
            Err(DbError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_share_cannot_be_for_an_unknown_user(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        assert!(matches!(
            db.create_user_share(alice, new_user_share(), &[file.id], &[UserId::new()])
                .await,
            Err(DbError::NotFound)
        ));
        assert!(db.owned_shares(alice, None).await.expect("list").is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn changing_recipients_keeps_when_the_others_were_added(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let bob = user(&db, "bob").await;
        let carol = user(&db, "carol").await;
        let dave = user(&db, "dave").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        let share = db
            .create_user_share(alice, new_user_share(), &[file.id], &[bob, carol])
            .await
            .expect("share");
        let before = db.share_recipients(&[share.id]).await.expect("before");
        db.update_share(
            alice,
            share.id,
            ShareUpdate {
                recipients: Some(&[carol, dave]),
                ..ShareUpdate::default()
            },
        )
        .await
        .expect("update");
        let after = db.share_recipients(&[share.id]).await.expect("after");
        let names: Vec<&str> = after[&share.id]
            .iter()
            .map(|recipient| recipient.username.as_str())
            .collect();
        assert_eq!(names, ["carol", "dave"]);
        assert_eq!(
            after[&share.id][0].created_at,
            before[&share.id][1].created_at
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_owner_lists_links_and_shares_with_users(pool: PgPool) {
        let db = Db::from_pool(pool);
        let alice = user(&db, "alice").await;
        let bob = user(&db, "bob").await;
        let file = node(&db, alice, None, NodeKind::File).await;
        db.create_share_link(alice, new_share(TOKEN), &[file.id])
            .await
            .expect("link");
        db.create_user_share(alice, new_user_share(), &[file.id], &[bob])
            .await
            .expect("users");
        let shares = db.owned_shares(alice, Some(file.id)).await.expect("list");
        assert_eq!(shares.len(), 2);
        assert_eq!(
            shares.iter().filter(|share| share.token.is_some()).count(),
            1
        );
    }
}
