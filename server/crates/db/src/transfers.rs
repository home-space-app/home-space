//! Files and folders sent to a device.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use protocol::TransferState;

use crate::{Db, DbError, Node, NodeId, NodeKind, UserId, accounts::lock_active_users};

/// A transfer with the names of the users and devices it is between.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct Transfer {
    pub id: Uuid,
    pub sender_id: UserId,
    pub sender_name: String,
    pub sender_device: Option<Uuid>,
    pub sender_device_name: Option<String>,
    pub recipient_id: UserId,
    pub recipient_name: String,
    pub recipient_device: Uuid,
    pub recipient_device_name: String,
    pub state: TransferState,
    pub total_bytes: i64,
    pub item_count: i32,
    pub file_count: i64,
    /// The path of the only top-level item, when there is one.
    pub name: Option<String>,
    /// The media type of the only item, when the transfer is a single file.
    pub mime: Option<String>,
    pub message: Option<String>,
    pub created_at: OffsetDateTime,
    pub ready_at: Option<OffsetDateTime>,
    pub expires_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
}

/// A file or folder of a transfer.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct TransferItem {
    pub id: Uuid,
    pub transfer_id: Uuid,
    pub path: String,
    pub kind: NodeKind,
    pub size: i64,
    /// Set once a file's bytes are in.
    pub sha256: Option<Vec<u8>>,
    pub mime: Option<String>,
    pub modified_at: Option<OffsetDateTime>,
    pub node_id: Option<NodeId>,
}

/// A transfer to record.
#[derive(Clone, Debug)]
pub struct NewTransfer {
    pub id: Uuid,
    pub sender: UserId,
    pub sender_device: Option<Uuid>,
    pub recipient: UserId,
    pub recipient_device: Uuid,
    pub message: Option<String>,
    pub expires_at: OffsetDateTime,
    /// Files with `sha256` set already have their bytes in place.
    pub items: Vec<TransferItem>,
}

/// A transfer removed because its time ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpiredTransfer {
    pub id: Uuid,
    pub sender_id: UserId,
    pub recipient_id: UserId,
}

/// A node of a subtree with its path below the subtree's root (`""` for the root itself,
/// `"/a/b"` below it).
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct SubtreeNode {
    #[sqlx(flatten)]
    pub node: Node,
    pub rel: String,
}

macro_rules! transfer_query {
    ($where:literal) => {
        concat!(
            "SELECT t.id, t.sender_id, s.username AS sender_name, t.sender_device, \
             sd.name AS sender_device_name, t.recipient_id, r.username AS recipient_name, \
             t.recipient_device, rd.name AS recipient_device_name, t.state, t.total_bytes, \
             t.item_count, t.message, t.created_at, t.ready_at, t.expires_at, t.finished_at, \
             (SELECT count(*) FROM transfer_items i \
              WHERE i.transfer_id = t.id AND i.kind = 'file') AS file_count, \
             (SELECT CASE WHEN count(*) = 1 THEN min(i.path) END FROM transfer_items i \
              WHERE i.transfer_id = t.id AND strpos(i.path, '/') = 0) AS name, \
             (SELECT i.mime FROM transfer_items i \
              WHERE i.transfer_id = t.id AND i.kind = 'file' AND t.item_count = 1) AS mime \
             FROM transfers t \
             JOIN users s ON s.id = t.sender_id \
             JOIN users r ON r.id = t.recipient_id \
             JOIN devices rd ON rd.user_id = t.recipient_id AND rd.id = t.recipient_device \
             LEFT JOIN devices sd ON sd.user_id = t.sender_id AND sd.id = t.sender_device \
             WHERE ",
            $where
        )
    };
}

impl Db {
    /// Records a transfer and its items. It is ready at once when every file has its bytes.
    /// `NotFound` while the sender or the recipient is disabled or marked for
    /// deletion.
    pub async fn create_transfer(&self, new: NewTransfer) -> Result<Transfer, DbError> {
        let total_bytes = new
            .items
            .iter()
            .filter(|item| item.kind == NodeKind::File)
            .map(|item| item.size)
            .sum::<i64>();
        let ready = new
            .items
            .iter()
            .all(|item| item.kind == NodeKind::Folder || item.sha256.is_some());
        let state = if ready {
            TransferState::Ready
        } else {
            TransferState::Uploading
        };
        let mut tx = self.pool().begin().await?;
        lock_active_users(&mut tx, &[new.sender, new.recipient]).await?;
        sqlx::query(
            "INSERT INTO transfers (id, sender_id, sender_device, recipient_id, \
             recipient_device, state, total_bytes, item_count, message, expires_at, ready_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, CASE WHEN $11 THEN now() END)",
        )
        .bind(new.id)
        .bind(new.sender)
        .bind(new.sender_device)
        .bind(new.recipient)
        .bind(new.recipient_device)
        .bind(state)
        .bind(total_bytes)
        .bind(i32::try_from(new.items.len()).map_err(|_| DbError::Invalid)?)
        .bind(new.message)
        .bind(new.expires_at)
        .bind(ready)
        .execute(&mut *tx)
        .await?;
        for item in &new.items {
            sqlx::query(
                "INSERT INTO transfer_items \
                 (id, transfer_id, path, kind, size, sha256, mime, modified_at, node_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(item.id)
            .bind(new.id)
            .bind(&item.path)
            .bind(item.kind)
            .bind(item.size)
            .bind(&item.sha256)
            .bind(&item.mime)
            .bind(item.modified_at)
            .bind(item.node_id)
            .execute(&mut *tx)
            .await?;
        }
        let transfer = sqlx::query_as::<_, Transfer>(transfer_query!("t.id = $1"))
            .bind(new.id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(transfer)
    }

    /// A transfer `caller` sent or receives; `NotFound` for any other.
    pub async fn transfer(&self, caller: UserId, id: Uuid) -> Result<Transfer, DbError> {
        let transfer = sqlx::query_as::<_, Transfer>(transfer_query!(
            "t.id = $1 AND (t.sender_id = $2 OR t.recipient_id = $2)"
        ))
        .bind(id)
        .bind(caller)
        .fetch_one(self.pool())
        .await?;
        Ok(transfer)
    }

    /// The items of a transfer, parents before what is in them.
    pub async fn transfer_items(&self, id: Uuid) -> Result<Vec<TransferItem>, DbError> {
        let items = sqlx::query_as::<_, TransferItem>(
            "SELECT id, transfer_id, path, kind, size, sha256, mime, modified_at, node_id \
             FROM transfer_items WHERE transfer_id = $1 ORDER BY path",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        Ok(items)
    }

    /// An item of a transfer `sender` sent, with the transfer.
    pub async fn sent_transfer_item(
        &self,
        sender: UserId,
        item: Uuid,
    ) -> Result<(Transfer, TransferItem), DbError> {
        let item = sqlx::query_as::<_, TransferItem>(
            "SELECT i.id, i.transfer_id, i.path, i.kind, i.size, i.sha256, i.mime, \
             i.modified_at, i.node_id FROM transfer_items i \
             JOIN transfers t ON t.id = i.transfer_id WHERE i.id = $1 AND t.sender_id = $2",
        )
        .bind(item)
        .bind(sender)
        .fetch_one(self.pool())
        .await?;
        let transfer = self.transfer(sender, item.transfer_id).await?;
        Ok((transfer, item))
    }

    /// The transfers sent to `recipient` (only to `device` when given) that wait for them:
    /// ready or declined, the newest first.
    pub async fn incoming_transfers(
        &self,
        recipient: UserId,
        device: Option<Uuid>,
    ) -> Result<Vec<Transfer>, DbError> {
        let transfers = sqlx::query_as::<_, Transfer>(transfer_query!(
            "t.recipient_id = $1 AND t.state IN ('ready', 'declined') \
             AND ($2::uuid IS NULL OR t.recipient_device = $2) \
             ORDER BY t.created_at DESC, t.id"
        ))
        .bind(recipient)
        .bind(device)
        .fetch_all(self.pool())
        .await?;
        Ok(transfers)
    }

    /// The transfers `sender` sent, the newest first.
    pub async fn outgoing_transfers(&self, sender: UserId) -> Result<Vec<Transfer>, DbError> {
        let transfers = sqlx::query_as::<_, Transfer>(transfer_query!(
            "t.sender_id = $1 ORDER BY t.created_at DESC, t.id"
        ))
        .bind(sender)
        .fetch_all(self.pool())
        .await?;
        Ok(transfers)
    }

    /// Records that a file's bytes are in. Returns the transfer when this made it ready.
    pub async fn store_transfer_item(
        &self,
        item: Uuid,
        sha256: &[u8],
    ) -> Result<Option<Transfer>, DbError> {
        let mut tx = self.pool().begin().await?;
        let transfer_id = sqlx::query_scalar::<_, Uuid>(
            "UPDATE transfer_items i SET sha256 = $2 FROM transfers t \
             WHERE i.id = $1 AND t.id = i.transfer_id AND t.state = 'uploading' \
             RETURNING i.transfer_id",
        )
        .bind(item)
        .bind(sha256)
        .fetch_one(&mut *tx)
        .await?;
        let ready = sqlx::query(
            "UPDATE transfers SET state = 'ready', ready_at = now() \
             WHERE id = $1 AND state = 'uploading' AND NOT EXISTS ( \
                 SELECT 1 FROM transfer_items WHERE transfer_id = $1 \
                 AND kind = 'file' AND sha256 IS NULL)",
        )
        .bind(transfer_id)
        .execute(&mut *tx)
        .await?;
        let transfer = if ready.rows_affected() == 1 {
            Some(
                sqlx::query_as::<_, Transfer>(transfer_query!("t.id = $1"))
                    .bind(transfer_id)
                    .fetch_one(&mut *tx)
                    .await?,
            )
        } else {
            None
        };
        tx.commit().await?;
        Ok(transfer)
    }

    /// Moves a transfer from one of the states in `from` to `to`, a finished state. Returns
    /// whether it was in one of them.
    pub async fn finish_transfer(
        &self,
        id: Uuid,
        from: &[TransferState],
        to: TransferState,
    ) -> Result<bool, DbError> {
        let finished = sqlx::query(
            "UPDATE transfers SET state = $3, finished_at = now() \
             WHERE id = $1 AND state = ANY($2)",
        )
        .bind(id)
        .bind(from)
        .bind(to)
        .execute(self.pool())
        .await?;
        Ok(finished.rows_affected() == 1)
    }

    /// Makes a saved transfer ready again, after saving it failed before anything was saved.
    pub async fn reopen_transfer(&self, id: Uuid) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE transfers SET state = 'ready', finished_at = NULL \
             WHERE id = $1 AND state = 'saved'",
        )
        .bind(id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The unfinished uploads of a transfer's items.
    pub async fn transfer_uploads(&self, id: Uuid) -> Result<Vec<Uuid>, DbError> {
        let uploads = sqlx::query_scalar::<_, Uuid>(
            "SELECT u.id FROM uploads u JOIN transfer_items i ON i.id = u.transfer_item_id \
             WHERE i.transfer_id = $1",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        Ok(uploads)
    }

    /// Removes the transfers whose time ran out before `now`, with their uploads. Returns the
    /// transfers and the ids of the uploads that went with them.
    pub async fn expire_transfers(
        &self,
        now: OffsetDateTime,
    ) -> Result<(Vec<ExpiredTransfer>, Vec<Uuid>), DbError> {
        let mut tx = self.pool().begin().await?;
        let uploads = sqlx::query_scalar::<_, Uuid>(
            "DELETE FROM uploads u USING transfer_items i, transfers t \
             WHERE i.id = u.transfer_item_id AND t.id = i.transfer_id AND t.expires_at < $1 \
             RETURNING u.id",
        )
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;
        let expired = sqlx::query_as::<_, (Uuid, UserId, UserId)>(
            "DELETE FROM transfers WHERE expires_at < $1 \
             RETURNING id, sender_id, recipient_id",
        )
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let expired = expired
            .into_iter()
            .map(|(id, sender_id, recipient_id)| ExpiredTransfer {
                id,
                sender_id,
                recipient_id,
            })
            .collect();
        Ok((expired, uploads))
    }

    /// The ids of every transfer, for sweeping leftovers on disk.
    pub async fn transfer_ids(&self) -> Result<Vec<Uuid>, DbError> {
        let ids = sqlx::query_scalar::<_, Uuid>("SELECT id FROM transfers")
            .fetch_all(self.pool())
            .await?;
        Ok(ids)
    }

    /// A live node of `owner` and everything live below it, the root first.
    pub async fn subtree(&self, owner: UserId, id: NodeId) -> Result<Vec<SubtreeNode>, DbError> {
        let nodes = sqlx::query_as::<_, SubtreeNode>(concat!(
            "WITH RECURSIVE tree AS ( \
                 SELECT id, owner_id, parent_id, kind, name, size, sha256, mime, version, \
                 created_at, updated_at, edited_on, ''::text AS rel FROM nodes \
                 WHERE id = $1 AND owner_id = $2 AND ",
            visible!("nodes"),
            " UNION ALL \
                 SELECT n.id, n.owner_id, n.parent_id, n.kind, n.name, n.size, n.sha256, \
                 n.mime, n.version, n.created_at, n.updated_at, n.edited_on, \
                 t.rel || '/' || n.name FROM nodes n \
                 JOIN tree t ON n.parent_id = t.id AND n.owner_id = $2 \
                 WHERE n.trashed_at IS NULL \
             ) SELECT * FROM tree ORDER BY rel"
        ))
        .bind(id)
        .bind(owner)
        .fetch_all(self.pool())
        .await?;
        if nodes.is_empty() {
            return Err(DbError::NotFound);
        }
        Ok(nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewDevice;

    struct Fixture {
        db: Db,
        alice: UserId,
        bob: UserId,
        phone: Uuid,
    }

    async fn fixture(pool: sqlx::PgPool) -> Fixture {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let bob = db.create_user("bob", "x").await.expect("bob").id;
        let phone = Uuid::now_v7();
        db.register_device(
            bob,
            NewDevice {
                id: phone,
                name: "Phone",
                platform: "android",
            },
        )
        .await
        .expect("phone");
        Fixture {
            db,
            alice,
            bob,
            phone,
        }
    }

    fn item(path: &str, kind: NodeKind, sha256: Option<Vec<u8>>) -> TransferItem {
        TransferItem {
            id: Uuid::now_v7(),
            transfer_id: Uuid::nil(),
            path: path.to_owned(),
            kind,
            size: 3,
            sha256,
            mime: None,
            modified_at: None,
            node_id: None,
        }
    }

    fn transfer(f: &Fixture, items: Vec<TransferItem>, expires_in: time::Duration) -> NewTransfer {
        NewTransfer {
            id: Uuid::now_v7(),
            sender: f.alice,
            sender_device: None,
            recipient: f.bob,
            recipient_device: f.phone,
            message: None,
            expires_at: OffsetDateTime::now_utc() + expires_in,
            items,
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_transfer_is_ready_once_every_file_is_in(pool: sqlx::PgPool) {
        let f = fixture(pool).await;
        let a = item("a.txt", NodeKind::File, None);
        let b = item("b.txt", NodeKind::File, None);
        let new = transfer(
            &f,
            vec![item("dir", NodeKind::Folder, None), a.clone(), b.clone()],
            time::Duration::hours(1),
        );
        let created = f.db.create_transfer(new).await.expect("create");
        assert_eq!(created.state, TransferState::Uploading);
        assert_eq!(created.total_bytes, 6);
        assert_eq!(
            created.mime, None,
            "only a single file has a type of its own"
        );
        assert_eq!(created.recipient_device_name, "Phone");
        assert!(
            f.db.incoming_transfers(f.bob, None)
                .await
                .expect("incoming")
                .is_empty()
        );

        assert_eq!(
            f.db.store_transfer_item(a.id, b"aaa").await.expect("a"),
            None
        );
        let ready = f.db.store_transfer_item(b.id, b"bbb").await.expect("b");
        assert_eq!(ready.map(|t| t.state), Some(TransferState::Ready));
        assert_eq!(
            f.db.incoming_transfers(f.bob, Some(f.phone))
                .await
                .expect("incoming")
                .len(),
            1
        );
        assert!(
            f.db.incoming_transfers(f.bob, Some(Uuid::now_v7()))
                .await
                .expect("other device")
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_single_file_carries_its_type(pool: sqlx::PgPool) {
        let f = fixture(pool).await;
        let link = TransferItem {
            mime: Some("text/uri-list".to_owned()),
            ..item("link.uris", NodeKind::File, None)
        };
        let created =
            f.db.create_transfer(transfer(&f, vec![link], time::Duration::hours(1)))
                .await
                .expect("create");
        assert_eq!(created.mime.as_deref(), Some("text/uri-list"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn only_the_sender_and_the_recipient_see_a_transfer(pool: sqlx::PgPool) {
        let f = fixture(pool).await;
        let carol = f.db.create_user("carol", "x").await.expect("carol").id;
        let new = transfer(
            &f,
            vec![item("a.txt", NodeKind::File, Some(vec![1]))],
            time::Duration::hours(1),
        );
        let id = f.db.create_transfer(new).await.expect("create").id;

        assert!(f.db.transfer(f.alice, id).await.is_ok());
        assert!(f.db.transfer(f.bob, id).await.is_ok());
        assert!(matches!(
            f.db.transfer(carol, id).await,
            Err(DbError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_finished_transfer_cannot_finish_again(pool: sqlx::PgPool) {
        let f = fixture(pool).await;
        let new = transfer(
            &f,
            vec![item("a.txt", NodeKind::File, Some(vec![1]))],
            time::Duration::hours(1),
        );
        let id = f.db.create_transfer(new).await.expect("create").id;
        let ready = [TransferState::Ready];

        assert!(
            f.db.finish_transfer(id, &ready, TransferState::Declined)
                .await
                .expect("decline")
        );
        assert!(
            !f.db
                .finish_transfer(id, &ready, TransferState::Downloaded)
                .await
                .expect("download")
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn expired_transfers_are_removed(pool: sqlx::PgPool) {
        let f = fixture(pool).await;
        let old = transfer(
            &f,
            vec![item("a.txt", NodeKind::File, None)],
            -time::Duration::minutes(1),
        );
        let fresh = transfer(
            &f,
            vec![item("a.txt", NodeKind::File, None)],
            time::Duration::hours(1),
        );
        let old = f.db.create_transfer(old).await.expect("old").id;
        let fresh = f.db.create_transfer(fresh).await.expect("fresh").id;

        let (expired, _) =
            f.db.expire_transfers(OffsetDateTime::now_utc())
                .await
                .expect("expire");

        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, old);
        assert_eq!(f.db.transfer_ids().await.expect("ids"), vec![fresh]);
    }
}
