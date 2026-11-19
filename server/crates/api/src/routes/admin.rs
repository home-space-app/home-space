//! Administration: the server's status and the accounts on it. Administrators get no access
//! to other users' files.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{delete, get, put},
};
use time::{Duration, OffsetDateTime};

use auth::AuthError;
use db::{DbError, UserId};
use protocol::{
    Account, AccountDevice, CreateAccount, DatabaseStatus, DiskStatus, ServerStatus, SetPassword,
    UpdateAccount, UserStorage,
};

use crate::extract::{ApiJson, ApiPath};
use crate::{
    ApiError, AppState,
    extract::{AdminUser, Tenant},
    support::{
        convert::{account, quota, usage},
        disk::disk_space,
    },
};

const NOT_YOURSELF: &str = "administrators cannot disable, delete or demote themselves";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/status", get(status))
        .route("/admin/users", get(list_accounts).post(create_account))
        .route(
            "/admin/users/{id}",
            get(get_account)
                .patch(update_account)
                .delete(delete_account),
        )
        .route("/admin/users/{id}/password", put(set_password))
        .route("/admin/users/{id}/devices", get(account_devices))
        .route("/admin/users/{id}/deletion", delete(cancel_deletion))
}

/// `set_disabled` refuses to enable an account that is marked for deletion.
fn marked_for_deletion(err: DbError) -> ApiError {
    match err {
        DbError::Conflict => ApiError::MarkedForDeletion,
        other => other.into(),
    }
}

/// Every account, by username.
async fn list_accounts(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<Account>>, ApiError> {
    let accounts = state.db.accounts().await?;
    Ok(Json(accounts.into_iter().map(account).collect()))
}

async fn get_account(
    State(state): State<AppState>,
    _admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<Account>, ApiError> {
    Ok(Json(account(state.db.account(id).await?)))
}

/// Creates an account that signs in with a password.
async fn create_account(
    State(state): State<AppState>,
    tenant: Tenant,
    _admin: AdminUser,
    ApiJson(request): ApiJson<CreateAccount>,
) -> Result<(StatusCode, Json<Account>), ApiError> {
    let user = tenant
        .auth
        .create_user(request.username.trim(), &request.password)
        .await?;
    if request.is_admin {
        state.db.set_admin(user.id, true).await?;
    }
    if request.quota_bytes.is_some() {
        state
            .db
            .set_quota(user.id, quota(request.quota_bytes))
            .await?;
    }
    let created = state.db.account(user.id).await?;
    Ok((StatusCode::CREATED, Json(account(created))))
}

/// Changes the role, state or quota of an account. Disabling it ends all its sessions.
async fn update_account(
    State(state): State<AppState>,
    tenant: Tenant,
    admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
    ApiJson(request): ApiJson<UpdateAccount>,
) -> Result<Json<Account>, ApiError> {
    if id == admin.id && request.is_admin == Some(false) {
        return Err(ApiError::invalid("/is_admin", NOT_YOURSELF));
    }
    if id == admin.id && request.disabled == Some(true) {
        return Err(ApiError::invalid("/disabled", NOT_YOURSELF));
    }
    // Fails for an unknown user before anything is changed.
    state.db.account(id).await?;
    if let Some(is_admin) = request.is_admin {
        state.db.set_admin(id, is_admin).await?;
    }
    if let Some(bytes) = request.quota_bytes {
        state.db.set_quota(id, quota(bytes)).await?;
    }
    if let Some(disabled) = request.disabled {
        state
            .db
            .set_disabled(id, disabled)
            .await
            .map_err(marked_for_deletion)?;
        if disabled {
            tenant.auth.sessions_ended(id);
        }
    }
    Ok(Json(account(state.db.account(id).await?)))
}

/// Sets a new password for an account that has one, ending all its sessions.
async fn set_password(
    State(state): State<AppState>,
    tenant: Tenant,
    _admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
    ApiJson(request): ApiJson<SetPassword>,
) -> Result<StatusCode, ApiError> {
    let user = state.db.find_user(id).await?.ok_or(ApiError::NotFound)?;
    if user.password_hash.is_none() {
        return Err(ApiError::Auth(AuthError::NoPassword));
    }
    tenant.auth.set_password(id, &request.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The devices of an account, the most recently seen first.
async fn account_devices(
    State(state): State<AppState>,
    _admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<Vec<AccountDevice>>, ApiError> {
    state.db.account(id).await?;
    let devices = state.db.devices(id).await?;
    Ok(Json(
        devices
            .into_iter()
            .map(|device| AccountDevice {
                id: device.id,
                name: device.name,
                platform: device.platform,
                last_seen_at: device.last_seen_at,
                signed_in: device.signed_in,
            })
            .collect(),
    ))
}

/// Marks an account for deletion after the configured delay and ends its sessions; a
/// background job then deletes it with all its files. Until then it can be cancelled.
async fn delete_account(
    State(state): State<AppState>,
    tenant: Tenant,
    admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<Account>, ApiError> {
    if id == admin.id {
        // The account to delete is the request's path, so the pointer is the request itself.
        return Err(ApiError::invalid("", NOT_YOURSELF));
    }
    let minutes = i64::try_from(state.info.config.user_deletion_delay_minutes).unwrap_or(i64::MAX);
    let delete_at = OffsetDateTime::now_utc().saturating_add(Duration::minutes(minutes));
    state.db.mark_for_deletion(id, delete_at).await?;
    tenant.auth.sessions_ended(id);
    Ok(Json(account(state.db.account(id).await?)))
}

/// Takes back a pending deletion; the user may sign in again unless disabled.
async fn cancel_deletion(
    State(state): State<AppState>,
    _admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<Account>, ApiError> {
    state.db.cancel_deletion(id).await?;
    Ok(Json(account(state.db.account(id).await?)))
}

async fn status(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<ServerStatus>, ApiError> {
    let users = state.db.storage_by_user().await?;
    let database = state.db.database_info().await?;
    Ok(Json(ServerStatus {
        version: state.info.version.clone(),
        started_at: state.info.started_at,
        disk: DiskStatus {
            path: state.files.store().root().display().to_string(),
            space: disk_space(&state).await,
        },
        users: users
            .into_iter()
            .map(|user| UserStorage {
                id: user.id,
                username: user.username,
                is_admin: user.is_admin,
                external: user.external,
                usage: usage(user.usage),
            })
            .collect(),
        config: state.info.config.clone(),
        database: DatabaseStatus {
            version: database.version,
            migrations: database.migrations,
        },
    }))
}
