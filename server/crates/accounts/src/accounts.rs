use time::{Duration, OffsetDateTime};

use auth::{AuthError, AuthService};
use db::{Account, DatabaseInfo, Db, DbError, Device, TenantId, User, UserId, UserStorage};
use protocol::{CreateAccount, Me, SecondFactors, UpdateAccount, UserRef};

use crate::AccountError;

/// The accounts of one tenant: the caller's own, and the administration of all of them.
#[derive(Clone, Debug)]
pub struct AccountService {
    db: Db,
    auth: AuthService,
    /// How long after an administrator deletes an account it is actually deleted.
    deletion_delay: Duration,
}

/// What the administration status shows about the tenant's storage and the database.
#[derive(Clone, Debug)]
pub struct StorageReport {
    pub users: Vec<UserStorage>,
    pub database: DatabaseInfo,
}

impl AccountService {
    /// The accounts of `auth`'s tenant, deleted `deletion_delay` after an administrator asks.
    pub fn new(db: Db, auth: AuthService, deletion_delay: Duration) -> Self {
        Self {
            db,
            auth,
            deletion_delay,
        }
    }

    fn tenant(&self) -> TenantId {
        self.auth.tenant()
    }

    /// The caller's own account of the tenant; `Unauthorized` once it is gone.
    async fn caller(&self, user: UserId) -> Result<User, AccountError> {
        self.db
            .find_user(user)
            .await?
            .filter(|found| found.tenant_id == self.tenant())
            .ok_or(AccountError::Unauthorized)
    }

    /// Refuses anyone but an administrator of the tenant. Asked on every request, so that
    /// taking the role away takes effect at once.
    pub async fn require_admin(&self, user: UserId) -> Result<(), AccountError> {
        if self.caller(user).await?.is_admin {
            Ok(())
        } else {
            Err(AccountError::Forbidden)
        }
    }

    /// Who the caller is.
    pub async fn me(&self, user: UserId) -> Result<Me, AccountError> {
        let found = self.caller(user).await?;
        let folders = self.db.special_folders(user).await?;
        let factors = self.db.second_factors(user).await?;
        Ok(Me {
            id: found.id,
            username: found.username,
            lost_and_found_id: folders.lost_and_found_id,
            is_admin: found.is_admin,
            external: found.password_hash.is_none(),
            second_factors: SecondFactors {
                totp: factors.totp,
                webauthn: u32::try_from(factors.webauthn).unwrap_or(0),
                recovery_codes_left: u32::try_from(factors.recovery_codes_left).unwrap_or(0),
            },
        })
    }

    /// Every other user of the tenant, by username, for pickers.
    pub async fn others(&self, user: UserId) -> Result<Vec<UserRef>, AccountError> {
        let users = self.db.list_users(self.tenant()).await?;
        Ok(users
            .into_iter()
            .filter(|other| other.id != user)
            .map(|other| UserRef {
                id: other.id,
                username: other.username,
            })
            .collect())
    }

    /// What the caller stores.
    pub async fn usage(&self, user: UserId) -> Result<db::StorageUsage, AccountError> {
        Ok(self.db.storage_usage(user).await?)
    }

    /// Every account of the tenant, by username.
    pub async fn accounts(&self) -> Result<Vec<Account>, AccountError> {
        Ok(self.db.accounts(self.tenant()).await?)
    }

    /// One account of the tenant.
    pub async fn account(&self, id: UserId) -> Result<Account, AccountError> {
        Ok(self.db.account(self.tenant(), id).await?)
    }

    /// Creates an account that signs in with a password.
    pub async fn create(&self, request: &CreateAccount) -> Result<Account, AccountError> {
        let user = self
            .auth
            .create_user(request.username.trim(), &request.password)
            .await?;
        if request.is_admin {
            self.db.set_admin(user.id, true).await?;
        }
        if request.quota_bytes.is_some() {
            self.db
                .set_quota(user.id, quota(request.quota_bytes))
                .await?;
        }
        self.account(user.id).await
    }

    /// Changes the role, state or quota of an account. Disabling it ends all its sessions.
    pub async fn update(
        &self,
        admin: UserId,
        id: UserId,
        request: &UpdateAccount,
    ) -> Result<Account, AccountError> {
        if id == admin && request.is_admin == Some(false) {
            return Err(AccountError::NotYourself { field: "/is_admin" });
        }
        if id == admin && request.disabled == Some(true) {
            return Err(AccountError::NotYourself { field: "/disabled" });
        }
        // Fails for an account of another tenant before anything is changed.
        self.account(id).await?;
        if let Some(is_admin) = request.is_admin {
            self.db.set_admin(id, is_admin).await?;
        }
        if !request.quota_bytes.is_keep() {
            let bytes = request.quota_bytes.value().copied();
            self.db.set_quota(id, quota(bytes)).await?;
        }
        if let Some(disabled) = request.disabled {
            self.db
                .set_disabled(id, disabled)
                .await
                .map_err(|err| match err {
                    DbError::Conflict => AccountError::MarkedForDeletion,
                    other => other.into(),
                })?;
            if disabled {
                self.auth.sessions_ended(id);
            }
        }
        self.account(id).await
    }

    /// Sets a new password for an account that has one, ending all its sessions.
    pub async fn set_password(&self, id: UserId, password: &str) -> Result<(), AccountError> {
        if self.account(id).await?.external {
            return Err(AuthError::NoPassword.into());
        }
        Ok(self.auth.set_password(id, password).await?)
    }

    /// The devices of an account, the most recently seen first.
    pub async fn devices(&self, id: UserId) -> Result<Vec<Device>, AccountError> {
        self.account(id).await?;
        Ok(self.db.devices(id).await?)
    }

    /// Marks an account for deletion after the configured delay and ends its sessions; a
    /// background job then deletes it with all its files. Until then it can be cancelled.
    pub async fn schedule_deletion(
        &self,
        admin: UserId,
        id: UserId,
    ) -> Result<Account, AccountError> {
        if id == admin {
            // The account to delete is the request's path, so the pointer is the request itself.
            return Err(AccountError::NotYourself { field: "" });
        }
        self.account(id).await?;
        let delete_at = OffsetDateTime::now_utc().saturating_add(self.deletion_delay);
        self.db.mark_for_deletion(id, delete_at).await?;
        self.auth.sessions_ended(id);
        self.account(id).await
    }

    /// Takes back a pending deletion; the user may sign in again unless disabled.
    pub async fn cancel_deletion(&self, id: UserId) -> Result<Account, AccountError> {
        self.account(id).await?;
        self.db.cancel_deletion(id).await?;
        self.account(id).await
    }

    /// What every user of the tenant stores, and what the database is.
    pub async fn storage_report(&self) -> Result<StorageReport, AccountError> {
        Ok(StorageReport {
            users: self.db.storage_by_user(self.tenant()).await?,
            database: self.db.database_info().await?,
        })
    }
}

/// A quota in bytes as the database keeps it; one too large to count is no limit at all.
fn quota(bytes: Option<u64>) -> Option<i64> {
    bytes.map(|bytes| i64::try_from(bytes).unwrap_or(i64::MAX))
}
