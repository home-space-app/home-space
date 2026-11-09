use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, NodeId, UserId};

/// A resumable upload in progress.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct Upload {
    pub id: Uuid,
    pub owner_id: UserId,
    pub parent_id: Option<NodeId>,
    pub node_id: Option<NodeId>,
    pub name: String,
    pub size: i64,
    pub mime: Option<String>,
    pub modified_at: Option<OffsetDateTime>,
    /// The device that started it, when the client named one.
    pub device_id: Option<Uuid>,
    /// Keep a different file with the same name, rather than making this its new version.
    pub keep_both: bool,
    /// With `keep_both`: the one file with the name that does get a new version.
    pub version_of: Option<NodeId>,
    /// Record the device as the file's source.
    pub auto_upload: bool,
    /// The transfer item the upload brings the bytes of, instead of a file in the tree.
    pub transfer_item_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// What a new upload will store.
#[derive(Clone, Debug)]
pub struct NewUpload {
    pub id: Uuid,
    pub parent_id: Option<NodeId>,
    pub node_id: Option<NodeId>,
    pub name: String,
    pub size: i64,
    pub mime: Option<String>,
    pub modified_at: Option<OffsetDateTime>,
    pub device_id: Option<Uuid>,
    pub keep_both: bool,
    pub version_of: Option<NodeId>,
    pub auto_upload: bool,
    pub transfer_item_id: Option<Uuid>,
}

macro_rules! upload_columns {
    () => {
        "id, owner_id, parent_id, node_id, name, size, mime, modified_at, device_id, keep_both, \
         version_of, auto_upload, transfer_item_id, created_at, updated_at"
    };
}

impl Db {
    /// Records a new upload.
    pub async fn create_upload(&self, owner: UserId, new: NewUpload) -> Result<Upload, DbError> {
        let upload = sqlx::query_as::<_, Upload>(concat!(
            "INSERT INTO uploads \
             (id, owner_id, parent_id, node_id, name, size, mime, modified_at, device_id, \
              keep_both, version_of, auto_upload, transfer_item_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING ",
            upload_columns!()
        ))
        .bind(new.id)
        .bind(owner)
        .bind(new.parent_id)
        .bind(new.node_id)
        .bind(new.name)
        .bind(new.size)
        .bind(new.mime)
        .bind(new.modified_at)
        .bind(new.device_id)
        .bind(new.keep_both)
        .bind(new.version_of)
        .bind(new.auto_upload)
        .bind(new.transfer_item_id)
        .fetch_one(self.pool())
        .await?;
        Ok(upload)
    }

    /// Fetches an upload of `owner`.
    pub async fn get_upload(&self, owner: UserId, id: Uuid) -> Result<Upload, DbError> {
        let upload = sqlx::query_as::<_, Upload>(concat!(
            "SELECT ",
            upload_columns!(),
            " FROM uploads WHERE id = $1 AND owner_id = $2"
        ))
        .bind(id)
        .bind(owner)
        .fetch_one(self.pool())
        .await?;
        Ok(upload)
    }

    /// The uploads of `owner`, only those of `device` when given, the oldest first.
    pub async fn uploads(
        &self,
        owner: UserId,
        device: Option<Uuid>,
    ) -> Result<Vec<Upload>, DbError> {
        let uploads = sqlx::query_as::<_, Upload>(concat!(
            "SELECT ",
            upload_columns!(),
            " FROM uploads WHERE owner_id = $1 AND ($2::uuid IS NULL OR device_id = $2) \
             ORDER BY created_at, id"
        ))
        .bind(owner)
        .bind(device)
        .fetch_all(self.pool())
        .await?;
        Ok(uploads)
    }

    /// Marks an upload as active now, postponing its expiry.
    pub async fn touch_upload(&self, id: Uuid) -> Result<(), DbError> {
        sqlx::query("UPDATE uploads SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Deletes an upload record; a missing one is not an error.
    pub async fn delete_upload(&self, id: Uuid) -> Result<(), DbError> {
        sqlx::query("DELETE FROM uploads WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Lists every upload record (used to recognise orphaned upload files).
    pub async fn upload_ids(&self) -> Result<Vec<Uuid>, DbError> {
        let ids = sqlx::query_scalar::<_, Uuid>("SELECT id FROM uploads")
            .fetch_all(self.pool())
            .await?;
        Ok(ids)
    }

    /// Deletes the uploads untouched since `before` and returns their ids.
    pub async fn expire_uploads(&self, before: OffsetDateTime) -> Result<Vec<Uuid>, DbError> {
        let ids =
            sqlx::query_scalar::<_, Uuid>("DELETE FROM uploads WHERE updated_at < $1 RETURNING id")
                .bind(before)
                .fetch_all(self.pool())
                .await?;
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_upload(name: &str) -> NewUpload {
        NewUpload {
            id: Uuid::now_v7(),
            parent_id: None,
            node_id: None,
            name: name.to_owned(),
            size: 10,
            mime: None,
            modified_at: None,
            device_id: None,
            keep_both: false,
            version_of: None,
            auto_upload: false,
            transfer_item_id: None,
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn uploads_are_scoped_to_their_owner(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let bob = db.create_user("bob", "x").await.expect("bob").id;
        let upload = db
            .create_upload(alice, new_upload("a.bin"))
            .await
            .expect("create");
        assert_eq!(db.get_upload(alice, upload.id).await.expect("get"), upload);
        assert!(matches!(
            db.get_upload(bob, upload.id).await,
            Err(DbError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn uploads_can_be_listed_by_device(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let bob = db.create_user("bob", "x").await.expect("bob").id;
        let laptop = Uuid::now_v7();
        let from_laptop = db
            .create_upload(
                alice,
                NewUpload {
                    device_id: Some(laptop),
                    ..new_upload("a.bin")
                },
            )
            .await
            .expect("laptop");
        let unnamed = db
            .create_upload(alice, new_upload("b.bin"))
            .await
            .expect("unnamed");
        db.create_upload(bob, new_upload("c.bin"))
            .await
            .expect("bob");

        let ids = |uploads: Vec<Upload>| uploads.into_iter().map(|u| u.id).collect::<Vec<_>>();
        assert_eq!(
            ids(db.uploads(alice, None).await.expect("all")),
            [from_laptop.id, unnamed.id]
        );
        assert_eq!(
            ids(db.uploads(alice, Some(laptop)).await.expect("laptop")),
            [from_laptop.id]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn invalid_names_are_rejected(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        assert!(matches!(
            db.create_upload(alice, new_upload("a/b")).await,
            Err(DbError::Invalid)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn only_stale_uploads_expire(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let upload = db
            .create_upload(alice, new_upload("a.bin"))
            .await
            .expect("create");
        let past = OffsetDateTime::now_utc() - time::Duration::hours(1);
        assert!(db.expire_uploads(past).await.expect("expire").is_empty());
        let future = OffsetDateTime::now_utc() + time::Duration::hours(1);
        assert_eq!(
            db.expire_uploads(future).await.expect("expire"),
            [upload.id]
        );
        assert!(db.upload_ids().await.expect("ids").is_empty());
    }
}
