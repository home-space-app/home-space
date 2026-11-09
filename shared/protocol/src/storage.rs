use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::UserId;

/// Bytes a user stores, split the way they can free it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageUsage {
    /// Current content of live files.
    pub files: u64,
    /// Previous versions of files.
    pub versions: u64,
    /// Files in the recycle bin.
    pub trash: u64,
}

impl StorageUsage {
    /// Everything counted against the user.
    pub fn total(&self) -> u64 {
        self.files + self.versions + self.trash
    }
}

/// Space on the filesystem that holds the server's storage root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskSpace {
    /// Size of the filesystem.
    pub total: u64,
    /// Bytes still available for new files.
    pub available: u64,
}

/// `GET /storage`: what the signed-in user stores and the room left on the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storage {
    pub usage: StorageUsage,
    /// `None` when the server cannot tell.
    pub disk: Option<DiskSpace>,
}

/// One user in the administration status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserStorage {
    pub id: UserId,
    pub username: String,
    pub is_admin: bool,
    /// Signs in through the identity provider rather than with a password.
    pub external: bool,
    pub usage: StorageUsage,
}

/// The disk of the storage root, for administrators.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskStatus {
    /// The storage root as configured.
    pub path: String,
    /// `None` when the server cannot tell.
    pub space: Option<DiskSpace>,
}

/// The configured identity provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcStatus {
    pub name: String,
    pub issuer: String,
    pub required_group: Option<String>,
}

/// Read-only settings, taken from the environment at start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub max_upload_bytes: u64,
    pub upload_chunk_bytes: u64,
    pub upload_expiry_hours: u64,
    /// `None` keeps the change journal for good.
    pub changes_retention_days: Option<i64>,
    pub password_login: bool,
    pub oidc: Option<OidcStatus>,
    pub access_ttl_secs: i64,
    pub refresh_ttl_secs: i64,
    /// A previous token signing secret is still accepted.
    pub previous_jwt_secret: bool,
    pub public_url: Option<String>,
    pub bind: String,
    /// The host port a container is published on (`HOMESPACE_PORT`); the server cannot see it.
    #[serde(default)]
    pub published_port: Option<u16>,
    /// Where file content is kept (`HOMESPACE_STORAGE`).
    #[serde(default)]
    pub storage: StorageBackend,
}

/// Where the server keeps file content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// Files on the server's own disk, under `HOMESPACE_STORAGE_ROOT`.
    #[default]
    Local,
}

/// The database server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseStatus {
    /// e.g. `PostgreSQL 17.2`.
    pub version: String,
    pub migrations: i64,
}

/// `GET /admin/status`: the server at a glance, for administrators.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStatus {
    /// The server's version.
    pub version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    pub disk: DiskStatus,
    /// Every user's storage, ordered by username.
    pub users: Vec<UserStorage>,
    pub config: ServerConfig,
    pub database: DatabaseStatus,
}

impl ServerStatus {
    /// What all users store together.
    pub fn total_usage(&self) -> StorageUsage {
        self.users
            .iter()
            .fold(StorageUsage::default(), |sum, user| StorageUsage {
                files: sum.files + user.usage.files,
                versions: sum.versions + user.usage.versions,
                trash: sum.trash + user.usage.trash,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(files: u64, versions: u64, trash: u64) -> UserStorage {
        UserStorage {
            id: UserId(uuid::Uuid::nil()),
            username: "u".to_owned(),
            is_admin: false,
            external: false,
            usage: StorageUsage {
                files,
                versions,
                trash,
            },
        }
    }

    #[test]
    fn usage_total_adds_every_part() {
        assert_eq!(user(1, 2, 3).usage.total(), 6);
    }

    #[test]
    fn total_usage_sums_users_by_part() {
        let status = ServerStatus {
            version: "0.1.0".to_owned(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            disk: DiskStatus {
                path: "/data".to_owned(),
                space: None,
            },
            users: vec![user(1, 2, 3), user(10, 20, 30)],
            config: ServerConfig {
                max_upload_bytes: 0,
                upload_chunk_bytes: 0,
                upload_expiry_hours: 0,
                changes_retention_days: None,
                password_login: true,
                oidc: None,
                access_ttl_secs: 0,
                refresh_ttl_secs: 0,
                previous_jwt_secret: false,
                public_url: None,
                bind: String::new(),
                published_port: None,
                storage: StorageBackend::Local,
            },
            database: DatabaseStatus {
                version: String::new(),
                migrations: 0,
            },
        };
        assert_eq!(
            status.total_usage(),
            StorageUsage {
                files: 11,
                versions: 22,
                trash: 33
            }
        );
    }
}
