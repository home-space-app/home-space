use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use protocol::{DeviceVisibility, UserRef};

use crate::{Db, DbError, UserId};

/// A device as its client describes it on sign-in.
#[derive(Clone, Copy, Debug)]
pub struct NewDevice<'a> {
    pub id: Uuid,
    pub name: &'a str,
    pub platform: &'a str,
}

/// A device of a user.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct Device {
    pub id: Uuid,
    pub name: String,
    pub platform: String,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    /// Whether it has a session that has not ended.
    pub signed_in: bool,
    /// How many uploads it has left unfinished.
    pub uploads: i64,
    /// Who can see it and send files to it.
    pub visibility: DeviceVisibility,
}

/// A device another user (or its owner) can send files to.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct TargetDevice {
    pub owner_id: UserId,
    pub owner_name: String,
    pub id: Uuid,
    pub name: String,
    pub platform: String,
    pub signed_in: bool,
}

/// Whether `$viewer` may see the device `d` and send files to it.
macro_rules! can_see_device {
    ($viewer:literal) => {
        concat!(
            "((d.user_id = ",
            $viewer,
            " AND d.visibility <> 'hidden') OR d.visibility = 'everyone' \
             OR (d.visibility = 'users' AND EXISTS (SELECT 1 FROM device_viewers v \
                 WHERE v.owner_id = d.user_id AND v.device_id = d.id AND v.viewer_id = ",
            $viewer,
            ")))"
        )
    };
}

impl Db {
    /// Records a device of `user`, or marks a known one as seen. A known device keeps its
    /// name, which the user may have changed.
    pub async fn register_device(
        &self,
        user: UserId,
        device: NewDevice<'_>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO devices (user_id, id, name, platform) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (user_id, id) DO UPDATE \
             SET platform = EXCLUDED.platform, last_seen_at = now()",
        )
        .bind(user)
        .bind(device.id)
        .bind(device.name)
        .bind(device.platform)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The devices of `user`, the most recently seen first.
    pub async fn devices(&self, user: UserId) -> Result<Vec<Device>, DbError> {
        let devices = sqlx::query_as::<_, Device>(
            "SELECT d.id, d.name, d.platform, d.created_at, d.last_seen_at, \
             EXISTS (SELECT 1 FROM sessions s WHERE s.user_id = d.user_id \
                     AND s.device_id = d.id AND s.expires_at > now()) AS signed_in, \
             (SELECT count(*) FROM uploads u WHERE u.owner_id = d.user_id \
              AND u.device_id = d.id) AS uploads, d.visibility \
             FROM devices d WHERE d.user_id = $1 ORDER BY d.last_seen_at DESC, d.id",
        )
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(devices)
    }

    /// The other users who may see the devices of `user` with visibility `users`, keyed by
    /// device.
    pub async fn device_viewers(&self, user: UserId) -> Result<Vec<(Uuid, UserRef)>, DbError> {
        let rows = sqlx::query_as::<_, (Uuid, UserId, String)>(
            "SELECT v.device_id, u.id, u.username FROM device_viewers v \
             JOIN users u ON u.id = v.viewer_id WHERE v.owner_id = $1 ORDER BY u.username",
        )
        .bind(user)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(device, id, username)| (device, UserRef { id, username }))
            .collect())
    }

    /// Sets who can see a device of `user`. `viewers` is kept only for
    /// [`DeviceVisibility::Users`]; `Invalid` when it names `user` or an unknown user.
    /// Returns the users who could see the device before or can see it now, so their lists
    /// of targets can be refreshed.
    pub async fn set_device_visibility(
        &self,
        user: UserId,
        id: Uuid,
        visibility: DeviceVisibility,
        viewers: &[UserId],
    ) -> Result<Vec<UserId>, DbError> {
        let mut tx = self.pool().begin().await?;
        let before = sqlx::query_scalar::<_, DeviceVisibility>(
            "SELECT visibility FROM devices WHERE user_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(user)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        let mut affected = sqlx::query_scalar::<_, UserId>(
            "DELETE FROM device_viewers WHERE owner_id = $1 AND device_id = $2 \
             RETURNING viewer_id",
        )
        .bind(user)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query("UPDATE devices SET visibility = $3 WHERE user_id = $1 AND id = $2")
            .bind(user)
            .bind(id)
            .bind(visibility)
            .execute(&mut *tx)
            .await?;
        if visibility == DeviceVisibility::Users {
            if viewers.contains(&user) {
                return Err(DbError::Invalid);
            }
            let ids = viewers.iter().map(|viewer| viewer.0).collect::<Vec<_>>();
            let inserted = sqlx::query(
                "INSERT INTO device_viewers (owner_id, device_id, viewer_id) \
                 SELECT $1, $2, u.id FROM users u WHERE u.id = ANY($3)",
            )
            .bind(user)
            .bind(id)
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
            let mut unique = ids.clone();
            unique.sort_unstable();
            unique.dedup();
            if usize::try_from(inserted.rows_affected()).ok() != Some(unique.len()) {
                return Err(DbError::Invalid);
            }
            affected.extend(viewers.iter().copied());
        }
        if before == DeviceVisibility::Everyone || visibility == DeviceVisibility::Everyone {
            affected.extend(
                sqlx::query_scalar::<_, UserId>("SELECT id FROM users WHERE id <> $1")
                    .bind(user)
                    .fetch_all(&mut *tx)
                    .await?,
            );
        }
        tx.commit().await?;
        affected.sort_unstable_by_key(|viewer| viewer.0);
        affected.dedup();
        Ok(affected)
    }

    /// The devices `viewer` may send files to: their own ones first, then those of other
    /// users by username.
    pub async fn transfer_targets(&self, viewer: UserId) -> Result<Vec<TargetDevice>, DbError> {
        let devices = sqlx::query_as::<_, TargetDevice>(concat!(
            "SELECT d.user_id AS owner_id, u.username AS owner_name, d.id, d.name, d.platform, \
             EXISTS (SELECT 1 FROM sessions s WHERE s.user_id = d.user_id \
                     AND s.device_id = d.id AND s.expires_at > now()) AS signed_in \
             FROM devices d JOIN users u ON u.id = d.user_id WHERE ",
            can_see_device!("$1"),
            " ORDER BY d.user_id <> $1, u.username, signed_in DESC, d.last_seen_at DESC, d.id"
        ))
        .bind(viewer)
        .fetch_all(self.pool())
        .await?;
        Ok(devices)
    }

    /// Whether `viewer` may send files to the device `id` of `owner`.
    pub async fn can_send_to(
        &self,
        viewer: UserId,
        owner: UserId,
        id: Uuid,
    ) -> Result<bool, DbError> {
        let visible = sqlx::query_scalar::<_, bool>(concat!(
            "SELECT EXISTS (SELECT 1 FROM devices d WHERE d.user_id = $2 AND d.id = $3 AND ",
            can_see_device!("$1"),
            ")"
        ))
        .bind(viewer)
        .bind(owner)
        .bind(id)
        .fetch_one(self.pool())
        .await?;
        Ok(visible)
    }

    /// Renames a device of `user`.
    pub async fn rename_device(&self, user: UserId, id: Uuid, name: &str) -> Result<(), DbError> {
        let renamed = sqlx::query("UPDATE devices SET name = $3 WHERE user_id = $1 AND id = $2")
            .bind(user)
            .bind(id)
            .bind(name)
            .execute(self.pool())
            .await?;
        if renamed.rows_affected() == 0 {
            Err(DbError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Marks a device of `user` as seen now.
    pub async fn touch_device(&self, user: UserId, id: Uuid) -> Result<(), DbError> {
        sqlx::query("UPDATE devices SET last_seen_at = now() WHERE user_id = $1 AND id = $2")
            .bind(user)
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Forgets devices that have no live session and were last seen before `seen_before`,
    /// returning how many were removed.
    pub async fn prune_devices(&self, seen_before: OffsetDateTime) -> Result<u64, DbError> {
        let pruned = sqlx::query(
            "DELETE FROM devices d WHERE d.last_seen_at < $1 \
             AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.user_id = d.user_id \
                             AND s.device_id = d.id AND s.expires_at > now())",
        )
        .bind(seen_before)
        .execute(self.pool())
        .await?;
        Ok(pruned.rows_affected())
    }

    /// Ends every session of a device of `user`. The device stays in the list until
    /// [`Db::prune_devices`] forgets it.
    pub async fn sign_out_device(&self, user: UserId, id: Uuid) -> Result<(), DbError> {
        let mut tx = self.pool().begin().await?;
        sqlx::query_scalar::<_, i32>("SELECT 1 FROM devices WHERE user_id = $1 AND id = $2")
            .bind(user)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND device_id = $2")
            .bind(user)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: &str = "Laptop";

    fn device(id: Uuid) -> NewDevice<'static> {
        NewDevice {
            id,
            name: LAPTOP,
            platform: "desktop-linux",
        }
    }

    async fn session(db: &Db, user: UserId, device: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        let expires = OffsetDateTime::now_utc() + time::Duration::days(1);
        db.start_session(id, user, device, id.as_bytes(), expires)
            .await
            .expect("session");
        id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn signing_in_again_keeps_the_name_the_user_chose(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");
        db.rename_device(alice, id, "Work").await.expect("rename");

        db.register_device(alice, device(id)).await.expect("again");

        let devices = db.devices(alice).await.expect("devices");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "Work");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn users_of_one_install_each_get_their_own_device(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let bob = db.create_user("bob", "x").await.expect("bob").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id)).await.expect("alice");
        db.register_device(bob, device(id)).await.expect("bob");

        assert!(matches!(
            db.rename_device(bob, Uuid::now_v7(), "x").await,
            Err(DbError::NotFound)
        ));
        db.rename_device(bob, id, "Bob's").await.expect("rename");
        assert_eq!(db.devices(alice).await.expect("alice")[0].name, LAPTOP);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn signing_out_a_device_ends_only_its_sessions(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let (laptop, phone) = (Uuid::now_v7(), Uuid::now_v7());
        db.register_device(alice, device(laptop))
            .await
            .expect("laptop");
        db.register_device(alice, device(phone))
            .await
            .expect("phone");
        let on_laptop = session(&db, alice, Some(laptop)).await;
        let on_phone = session(&db, alice, Some(phone)).await;
        let unnamed = session(&db, alice, None).await;

        db.sign_out_device(alice, laptop).await.expect("sign out");

        assert_eq!(
            db.live_session(on_laptop, alice).await.expect("laptop"),
            None
        );
        assert!(
            db.live_session(on_phone, alice)
                .await
                .expect("phone")
                .is_some()
        );
        assert!(
            db.live_session(unnamed, alice)
                .await
                .expect("unnamed")
                .is_some()
        );
        let signed_in = db
            .devices(alice)
            .await
            .expect("devices")
            .into_iter()
            .map(|device| (device.id, device.signed_in))
            .collect::<Vec<_>>();
        assert_eq!(signed_in.len(), 2);
        assert!(signed_in.contains(&(laptop, false)));
        assert!(signed_in.contains(&(phone, true)));
    }

    async fn last_seen(db: &Db, user: UserId, id: Uuid, days_ago: i64) {
        sqlx::query(
            "UPDATE devices SET last_seen_at = now() - make_interval(days => $3) \
             WHERE user_id = $1 AND id = $2",
        )
        .bind(user)
        .bind(id)
        .bind(i32::try_from(days_ago).expect("days"))
        .execute(db.pool())
        .await
        .expect("last seen");
    }

    fn days_ago(days: i64) -> OffsetDateTime {
        OffsetDateTime::now_utc() - time::Duration::days(days)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_device_signed_out_for_long_is_forgotten(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");
        last_seen(&db, alice, id, 31).await;

        assert_eq!(db.prune_devices(days_ago(30)).await.expect("prune"), 1);
        assert!(db.devices(alice).await.expect("devices").is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_device_signed_out_recently_is_kept(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");
        last_seen(&db, alice, id, 2).await;

        assert_eq!(db.prune_devices(days_ago(30)).await.expect("prune"), 0);
        assert_eq!(db.devices(alice).await.expect("devices").len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_signed_in_device_is_kept(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");
        session(&db, alice, Some(id)).await;
        last_seen(&db, alice, id, 31).await;

        assert_eq!(db.prune_devices(days_ago(30)).await.expect("prune"), 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn touching_a_device_marks_it_seen(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");
        last_seen(&db, alice, id, 2).await;

        db.touch_device(alice, id).await.expect("touch");

        let seen = db.devices(alice).await.expect("devices")[0].last_seen_at;
        assert!(seen > days_ago(1));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_session_without_a_device_takes_the_one_attached(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");
        let unnamed = session(&db, alice, None).await;

        db.attach_device(unnamed, alice, id).await.expect("attach");

        let live = db.live_session(unnamed, alice).await.expect("live");
        assert_eq!(live.map(|live| live.device_id), Some(Some(id)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_session_keeps_the_device_it_has(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let (laptop, other) = (Uuid::now_v7(), Uuid::now_v7());
        db.register_device(alice, device(laptop))
            .await
            .expect("laptop");
        db.register_device(alice, device(other))
            .await
            .expect("other");
        let on_laptop = session(&db, alice, Some(laptop)).await;

        db.attach_device(on_laptop, alice, other)
            .await
            .expect("attach");

        let live = db.live_session(on_laptop, alice).await.expect("live");
        assert_eq!(live.map(|live| live.device_id), Some(Some(laptop)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn another_users_device_cannot_be_signed_out(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let bob = db.create_user("bob", "x").await.expect("bob").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");
        let on_laptop = session(&db, alice, Some(id)).await;

        assert!(matches!(
            db.sign_out_device(bob, id).await,
            Err(DbError::NotFound)
        ));
        assert!(
            db.live_session(on_laptop, alice)
                .await
                .expect("live")
                .is_some()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn visibility_decides_who_can_send_to_a_device(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let bob = db.create_user("bob", "x").await.expect("bob").id;
        let carol = db.create_user("carol", "x").await.expect("carol").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");
        let can = async |viewer| db.can_send_to(viewer, alice, id).await.expect("can");

        assert_eq!(
            db.devices(alice).await.expect("devices")[0].visibility,
            DeviceVisibility::Owner
        );
        assert!(can(alice).await);
        assert!(!can(bob).await);

        db.set_device_visibility(alice, id, DeviceVisibility::Hidden, &[])
            .await
            .expect("hidden");
        assert!(!can(alice).await);
        assert!(!can(bob).await);

        let affected = db
            .set_device_visibility(alice, id, DeviceVisibility::Users, &[bob])
            .await
            .expect("users");
        assert_eq!(affected, vec![bob]);
        assert!(can(alice).await);
        assert!(can(bob).await);
        assert!(!can(carol).await);
        assert_eq!(db.device_viewers(alice).await.expect("viewers").len(), 1);

        db.set_device_visibility(alice, id, DeviceVisibility::Everyone, &[bob])
            .await
            .expect("everyone");
        assert!(can(carol).await);
        assert!(db.device_viewers(alice).await.expect("viewers").is_empty());

        let targets = db.transfer_targets(bob).await.expect("targets");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].owner_name, "alice");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_device_cannot_be_shown_to_its_owner_or_unknown_users(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let alice = db.create_user("alice", "x").await.expect("alice").id;
        let bob = db.create_user("bob", "x").await.expect("bob").id;
        let id = Uuid::now_v7();
        db.register_device(alice, device(id))
            .await
            .expect("register");

        for viewers in [vec![alice], vec![UserId::new()]] {
            assert!(matches!(
                db.set_device_visibility(alice, id, DeviceVisibility::Users, &viewers)
                    .await,
                Err(DbError::Invalid)
            ));
        }
        assert!(matches!(
            db.set_device_visibility(bob, id, DeviceVisibility::Everyone, &[])
                .await,
            Err(DbError::NotFound)
        ));
        assert_eq!(
            db.devices(alice).await.expect("devices")[0].visibility,
            DeviceVisibility::Owner
        );
    }
}
