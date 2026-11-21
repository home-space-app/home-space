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

/// Whether `$viewer` may see the device `d` and send files to it. Only devices of users of
/// the viewer's own tenant are seen, even with visibility `everyone`; devices of a disabled
/// user, which includes one marked for deletion, are seen by nobody.
macro_rules! can_see_device {
    ($viewer:literal) => {
        concat!(
            "EXISTS (SELECT 1 FROM users o JOIN users vw ON vw.tenant_id = o.tenant_id \
                 WHERE o.id = d.user_id AND vw.id = ",
            $viewer,
            " AND o.disabled_at IS NULL) \
             AND ((d.user_id = ",
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
    /// [`DeviceVisibility::Users`]; `Invalid` when it names `user`, an unknown user or a user
    /// of another tenant.
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
                 SELECT $1, $2, u.id FROM users u JOIN users o ON o.tenant_id = u.tenant_id \
                 WHERE o.id = $1 AND u.id = ANY($3)",
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
                sqlx::query_scalar::<_, UserId>(
                    "SELECT u.id FROM users u JOIN users o ON o.tenant_id = u.tenant_id \
                     WHERE o.id = $1 AND u.id <> $1",
                )
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

    /// Forgets a device of `user` now: its sessions, who could see it and the transfers sent
    /// to it go with it. It comes back if it signs in again.
    /// Returns the other users who could see it, so their lists of targets can be refreshed.
    pub async fn remove_device(&self, user: UserId, id: Uuid) -> Result<Vec<UserId>, DbError> {
        let mut tx = self.pool().begin().await?;
        let visibility = sqlx::query_scalar::<_, DeviceVisibility>(
            "SELECT visibility FROM devices WHERE user_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(user)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        let affected = if visibility == DeviceVisibility::Everyone {
            sqlx::query_scalar::<_, UserId>(
                "SELECT u.id FROM users u JOIN users o ON o.tenant_id = u.tenant_id \
                 WHERE o.id = $1 AND u.id <> $1",
            )
            .bind(user)
            .fetch_all(&mut *tx)
            .await?
        } else {
            sqlx::query_scalar::<_, UserId>(
                "SELECT viewer_id FROM device_viewers WHERE owner_id = $1 AND device_id = $2",
            )
            .bind(user)
            .bind(id)
            .fetch_all(&mut *tx)
            .await?
        };
        sqlx::query("DELETE FROM devices WHERE user_id = $1 AND id = $2")
            .bind(user)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(affected)
    }
}

#[cfg(test)]
mod tests;
