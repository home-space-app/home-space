//! Administration: the server's status and the accounts on it. Administrators get no access
//! to other users' files.

use axum::{
    Json, Router,
    extract::{Path, State},
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

use super::storage::{disk_space, usage};
use crate::{ApiError, AppState, extract::AdminUser};

const NOT_YOURSELF: &str = "administrators cannot disable, delete or demote themselves";
const NO_PASSWORD: &str = "this account signs in through single sign-on and has no password";

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

fn db_error(err: DbError) -> ApiError {
    match err {
        DbError::NotFound => ApiError::NotFound,
        DbError::Conflict => ApiError::BadRequest(
            "The account is marked for deletion; cancel the deletion to enable it.".to_owned(),
        ),
        other => ApiError::Internal(other.to_string()),
    }
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn account(found: db::Account) -> Account {
    Account {
        id: found.id,
        username: found.username,
        is_admin: found.is_admin,
        external: found.external,
        created_at: found.created_at,
        disabled: found.disabled_at.is_some(),
        quota_bytes: found.quota_bytes.map(to_u64),
        delete_at: found.delete_at,
        usage: usage(found.usage),
        devices: u32::try_from(found.devices).unwrap_or(u32::MAX),
        last_seen_at: found.last_seen_at,
    }
}

fn quota(bytes: Option<u64>) -> Option<i64> {
    bytes.map(|bytes| i64::try_from(bytes).unwrap_or(i64::MAX))
}

/// Every account, by username.
async fn list_accounts(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<Account>>, ApiError> {
    let accounts = state.db.accounts().await.map_err(db_error)?;
    Ok(Json(accounts.into_iter().map(account).collect()))
}

async fn get_account(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<UserId>,
) -> Result<Json<Account>, ApiError> {
    Ok(Json(account(state.db.account(id).await.map_err(db_error)?)))
}

/// Creates an account that signs in with a password.
async fn create_account(
    State(state): State<AppState>,
    _admin: AdminUser,
    Json(request): Json<CreateAccount>,
) -> Result<(StatusCode, Json<Account>), ApiError> {
    let user = state
        .auth
        .create_user(request.username.trim(), &request.password)
        .await?;
    if request.is_admin {
        state.db.set_admin(user.id, true).await.map_err(db_error)?;
    }
    if request.quota_bytes.is_some() {
        state
            .db
            .set_quota(user.id, quota(request.quota_bytes))
            .await
            .map_err(db_error)?;
    }
    let created = state.db.account(user.id).await.map_err(db_error)?;
    Ok((StatusCode::CREATED, Json(account(created))))
}

/// Changes the role, state or quota of an account. Disabling it ends all its sessions.
async fn update_account(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<UserId>,
    Json(request): Json<UpdateAccount>,
) -> Result<Json<Account>, ApiError> {
    if id == admin.id && (request.is_admin == Some(false) || request.disabled == Some(true)) {
        return Err(ApiError::BadRequest(NOT_YOURSELF.to_owned()));
    }
    // Fails for an unknown user before anything is changed.
    state.db.account(id).await.map_err(db_error)?;
    if let Some(is_admin) = request.is_admin {
        state.db.set_admin(id, is_admin).await.map_err(db_error)?;
    }
    if let Some(bytes) = request.quota_bytes {
        state
            .db
            .set_quota(id, quota(bytes))
            .await
            .map_err(db_error)?;
    }
    if let Some(disabled) = request.disabled {
        state
            .db
            .set_disabled(id, disabled)
            .await
            .map_err(db_error)?;
        if disabled {
            state.auth.sessions_ended(id);
        }
    }
    Ok(Json(account(state.db.account(id).await.map_err(db_error)?)))
}

/// Sets a new password for an account that has one, ending all its sessions.
async fn set_password(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<UserId>,
    Json(request): Json<SetPassword>,
) -> Result<StatusCode, ApiError> {
    let user = state
        .db
        .find_user(id)
        .await
        .map_err(db_error)?
        .ok_or(ApiError::NotFound)?;
    if user.password_hash.is_none() {
        return Err(ApiError::Auth(AuthError::InvalidInput(NO_PASSWORD)));
    }
    state.auth.set_password(id, &request.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The devices of an account, the most recently seen first.
async fn account_devices(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<UserId>,
) -> Result<Json<Vec<AccountDevice>>, ApiError> {
    state.db.account(id).await.map_err(db_error)?;
    let devices = state.db.devices(id).await.map_err(db_error)?;
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
    admin: AdminUser,
    Path(id): Path<UserId>,
) -> Result<Json<Account>, ApiError> {
    if id == admin.id {
        return Err(ApiError::BadRequest(NOT_YOURSELF.to_owned()));
    }
    let minutes = i64::try_from(state.info.config.user_deletion_delay_minutes).unwrap_or(i64::MAX);
    let delete_at = OffsetDateTime::now_utc().saturating_add(Duration::minutes(minutes));
    state
        .db
        .mark_for_deletion(id, delete_at)
        .await
        .map_err(db_error)?;
    state.auth.sessions_ended(id);
    Ok(Json(account(state.db.account(id).await.map_err(db_error)?)))
}

/// Takes back a pending deletion; the user may sign in again unless disabled.
async fn cancel_deletion(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<UserId>,
) -> Result<Json<Account>, ApiError> {
    state.db.cancel_deletion(id).await.map_err(db_error)?;
    Ok(Json(account(state.db.account(id).await.map_err(db_error)?)))
}

async fn status(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<ServerStatus>, ApiError> {
    let internal = |err: db::DbError| ApiError::Internal(err.to_string());
    let users = state.db.storage_by_user().await.map_err(internal)?;
    let database = state.db.database_info().await.map_err(internal)?;
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
