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
mod changes;
mod devices;
mod error;
mod login_attempts;
mod models;
mod nodes;
mod oidc;
mod refresh_tokens;
mod search;
mod sessions;
mod storage;
mod transfers;
mod trash;
mod uploads;
mod users;
mod versions;

pub use accounts::{Account, DeletedAccount};
pub use changes::{ChangeBatch, NodeChange};
pub use devices::{Device, NewDevice, TargetDevice};
pub use error::DbError;
pub use models::{FileVersion, Node, RefreshToken, User};
pub use nodes::{
    BlobOp, DeletedNodes, FOLDER_MIME, MoveBatch, MoveConflict, MoveOutcome, NewContent, NewNode,
    NodeProperties, NodeUpdate, StoreOptions, StoreOutcome,
};
pub use oidc::{OidcCode, OidcPending};
pub use protocol::{NodeId, NodeKind, OnConflict, UserId};
pub use search::{ContentState, RankedNode};
pub use sessions::LiveSession;
pub use sqlx::PgPool;
pub use storage::{DatabaseInfo, StorageUsage, UserStorage};
pub use transfers::{ExpiredTransfer, NewTransfer, SubtreeNode, Transfer, TransferItem};
pub use trash::{LOST_AND_FOUND, SpecialFolders, TrashOutcome, TrashedNode};
pub use uploads::{NewUpload, Upload};

use sqlx::{Postgres, Transaction, postgres::PgPoolOptions};
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
