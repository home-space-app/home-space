//! Shares of a user's files and folders: through a link, or with named users.

use std::collections::HashMap;

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, Node, NodeId, NodeKind, TenantId, UserId};

mod types;
pub use types::{
    NewShareLink, NewUserShare, OwnedShare, ReceivedShare, ShareLink, ShareRecipient, ShareUpdate,
    SharedAccess, SharedRoot,
};

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

/// Adds share `id` of `owner` for `node_ids`, which must all be live nodes of `owner`.
/// Adds `users` to the recipients of the share `id` of `owner`; `NotFound` unless every one
/// is another user of the owner's tenant. Users never change tenant, so counting them first
/// holds under concurrency.
async fn add_recipients(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: UserId,
    id: Uuid,
    users: &[UserId],
) -> Result<(), DbError> {
    let mut unique = users.to_vec();
    unique.sort_unstable_by_key(|user| user.0);
    unique.dedup();
    let found: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users r JOIN users o ON o.tenant_id = r.tenant_id \
         WHERE o.id = $1 AND r.id = ANY($2) AND r.id <> $1",
    )
    .bind(owner)
    .bind(&unique)
    .fetch_one(&mut **tx)
    .await?;
    if usize::try_from(found).ok() != Some(unique.len()) {
        return Err(DbError::NotFound);
    }
    sqlx::query(
        "INSERT INTO share_users (share_id, user_id) SELECT $1, unnest($2::uuid[]) \
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(&unique)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_share(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: UserId,
    id: Uuid,
    expires_at: Option<OffsetDateTime>,
    node_ids: &[NodeId],
) -> Result<(), DbError> {
    let found: i64 = sqlx::query_scalar(concat!(
        "SELECT count(*) FROM nodes WHERE owner_id = $1 AND id = ANY($2) AND ",
        visible!("nodes"),
        " AND ",
        unprotected!("nodes"),
        " AND NOT EXISTS (SELECT 1 FROM protected_folders p WHERE p.node_id = nodes.id)"
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
    /// all be other users of the owner's tenant; `NotFound` when one is not.
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
        add_recipients(&mut tx, owner, new.id, user_ids).await?;
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
    /// recipients must be other users of the owner's tenant; `NotFound` when one is not.
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
        .bind(!update.expires_at.is_keep())
        .bind(update.expires_at.value().copied())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        if !update.password_hash.is_keep() {
            let password_hash = update.password_hash.value().copied();
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
            add_recipients(&mut tx, owner, id, recipients).await?;
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

    /// The share link with `token`, whoever of `tenant` owns it; `NotFound` for a link of
    /// another tenant.
    pub async fn share_by_token(
        &self,
        tenant: TenantId,
        token: &str,
    ) -> Result<ShareLink, DbError> {
        let share = sqlx::query_as::<_, ShareLink>(concat!(
            "SELECT ",
            share_columns!(),
            " WHERE l.token = $1 AND u.tenant_id = $2"
        ))
        .bind(token)
        .bind(tenant)
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
    /// folder, with nothing on the way up in the recycle bin. `NotFound` for anything else,
    /// protected folders and what they hold included.
    pub async fn shared_node(&self, share: SharedRoot, id: NodeId) -> Result<Node, DbError> {
        let reachable: bool = sqlx::query_scalar(concat!(
            "WITH RECURSIVE chain AS ( \
                 SELECT n.id, n.parent_id, 0 AS depth FROM nodes n \
                 WHERE n.id = $2 AND n.owner_id = $3 AND ",
            visible!("n"),
            " AND ",
            unprotected!("n"),
            " AND NOT EXISTS (SELECT 1 FROM protected_folders p WHERE p.node_id = n.id) \
                 UNION ALL \
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
            let protected: Vec<NodeId> = sqlx::query_scalar(
                "SELECT p.node_id FROM protected_folders p JOIN nodes n ON n.id = p.node_id \
                 WHERE n.parent_id = $1 AND n.owner_id = $2",
            )
            .bind(parent_id)
            .bind(share.owner_id)
            .fetch_all(self.pool())
            .await?;
            let mut children = self.list_children(share.owner_id, Some(parent_id)).await?;
            children.retain(|child| !protected.contains(&child.id));
            Ok(children)
        } else {
            Err(DbError::NotFound)
        }
    }
}

#[cfg(test)]
mod tests;
