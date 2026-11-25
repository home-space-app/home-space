//! Postgres access for home-space: pool setup, migrations, models and repositories.

/// Restricts a query on `nodes` (under `$alias`) to what the node API exposes: live nodes, so
/// neither binned files nor a user's recycle bin folder.
macro_rules! visible {
    ($alias:literal) => {
        concat!(
            $alias,
            ".trashed_at IS NULL AND NOT EXISTS \
             (SELECT 1 FROM user_folders uf WHERE uf.trash_id = ",
            $alias,
            ".id)"
        )
    };
}

mod accounts;
mod browser_logins;
mod changes;
mod devices;
mod duplicates;
mod error;
mod login_attempts;
mod login_codes;
mod models;
mod nodes;
mod oidc;
mod refresh_tokens;
mod search;
mod sessions;
mod shares;
mod storage;
mod tenants;
mod transfers;
mod trash;
mod tx;
mod uploads;
mod users;
mod versions;

pub use accounts::{Account, DeletedAccount};
pub use browser_logins::BrowserLogin;
pub use changes::{ChangeBatch, ChangesSince, NodeChange};
pub use devices::{Device, NewDevice, TargetDevice};
pub use duplicates::OutdatedRow;
pub use error::DbError;
pub use login_codes::LoginCode;
pub use models::{FileVersion, Node, RefreshToken, User};
pub use nodes::{DeletedNodes, FOLDER_MIME, FileState, NewContent, NewNode, NodeProperties};
pub use oidc::OidcPending;
pub use protocol::{NodeId, NodeKind, OnConflict, Patch, SharePermission, UserId};
pub use search::{ContentState, RankedNode};
pub use sessions::{Authentication, LiveSession};
pub use shares::{
    NewShareLink, NewUserShare, OwnedShare, ReceivedShare, ShareLink, ShareRecipient, ShareUpdate,
    SharedAccess, SharedRoot,
};
pub use sqlx::PgPool;
pub use storage::{DatabaseInfo, QuotaUsage, StorageUsage, UserStorage};
pub use tenants::{Tenant, TenantDomain, TenantId};
pub use transfers::{
    ExpiredTransfer, IncomingScope, NewTransfer, SubtreeNode, Transfer, TransferItem,
};
pub use trash::{LOST_AND_FOUND, SpecialFolders, TrashedNode};
pub use tx::Tx;
pub use uploads::{NewUpload, Upload};

use sqlx::{
    Postgres, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use uuid::Uuid;

const MAX_CONNECTIONS: u32 = 10;
/// The transaction setting the change journal's trigger reads the writing device from.
pub(crate) const DEVICE_SETTING: &str = "homespace.device_id";

/// Handle to the database; cheap to clone.
#[derive(Clone, Debug)]
pub struct Db {
    pool: PgPool,
    /// The device the writes are made from, recorded in the change journal.
    device: Option<Uuid>,
}

impl Db {
    /// Connects to Postgres at `database_url`.
    pub async fn connect(database_url: &str) -> Result<Self, DbError> {
        let pool = PgPoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect(database_url)
            .await?;
        Ok(Self { pool, device: None })
    }

    /// Connects to Postgres at `database_url`, signing in with `password` instead of the one in
    /// the URL when it is given, so that the password can be kept out of the URL (in a secret
    /// file, say) and needs no percent-encoding.
    pub async fn connect_with_password(
        database_url: &str,
        password: Option<&str>,
    ) -> Result<Self, DbError> {
        let Some(password) = password else {
            return Self::connect(database_url).await;
        };
        let options: PgConnectOptions = database_url.parse()?;
        let pool = PgPoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect_with(options.password(password))
            .await?;
        Ok(Self { pool, device: None })
    }

    /// Wraps an existing pool (used by tests).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool, device: None }
    }

    /// The same database, recording `device` as the maker of the node changes written
    /// through it; `None` records none.
    #[must_use]
    pub fn on_device(&self, device: Option<Uuid>) -> Self {
        Self {
            pool: self.pool.clone(),
            device,
        }
    }

    /// Starts a transaction whose node changes are recorded as made by this handle's device.
    pub(crate) async fn begin(&self) -> Result<Transaction<'static, Postgres>, DbError> {
        let mut tx = self.pool.begin().await?;
        if let Some(device) = self.device {
            sqlx::query("SELECT set_config($1, $2, true)")
                .bind(DEVICE_SETTING)
                .bind(device.to_string())
                .execute(&mut *tx)
                .await?;
        }
        Ok(tx)
    }

    /// Applies all pending migrations.
    pub async fn migrate(&self) -> Result<(), DbError> {
        sqlx::migrate!("../../migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Verifies the database answers queries.
    pub async fn ping(&self) -> Result<(), DbError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }
}
