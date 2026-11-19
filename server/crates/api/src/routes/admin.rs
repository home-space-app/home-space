//! Administration: the server's status and the accounts of the tenant. Administrators get no
//! access to other users' files.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{delete, get, put},
};

use db::UserId;
use protocol::{
    Account, AccountDevice, CreateAccount, DatabaseStatus, DiskStatus, ServerStatus, SetPassword,
    UpdateAccount, UserStorage,
};

use crate::extract::{ApiJson, ApiPath};
use crate::{
    ApiError, AppState,
    extract::{AdminUser, Tenant},
    support::{
        convert::{account, usage},
        disk::disk_space,
    },
};

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

/// Every account, by username.
async fn list_accounts(tenant: Tenant, _admin: AdminUser) -> Result<Json<Vec<Account>>, ApiError> {
    let accounts = tenant.accounts.accounts().await?;
    Ok(Json(accounts.into_iter().map(account).collect()))
}

async fn get_account(
    tenant: Tenant,
    _admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<Account>, ApiError> {
    Ok(Json(account(tenant.accounts.account(id).await?)))
}

/// Creates an account that signs in with a password.
async fn create_account(
    tenant: Tenant,
    _admin: AdminUser,
    ApiJson(request): ApiJson<CreateAccount>,
) -> Result<(StatusCode, Json<Account>), ApiError> {
    let created = tenant.accounts.create(&request).await?;
    Ok((StatusCode::CREATED, Json(account(created))))
}

/// Changes the role, state or quota of an account. Disabling it ends all its sessions.
async fn update_account(
    tenant: Tenant,
    admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
    ApiJson(request): ApiJson<UpdateAccount>,
) -> Result<Json<Account>, ApiError> {
    let updated = tenant.accounts.update(admin.id, id, &request).await?;
    Ok(Json(account(updated)))
}

/// Sets a new password for an account that has one, ending all its sessions.
async fn set_password(
    tenant: Tenant,
    _admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
    ApiJson(request): ApiJson<SetPassword>,
) -> Result<StatusCode, ApiError> {
    tenant.accounts.set_password(id, &request.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The devices of an account, the most recently seen first.
async fn account_devices(
    tenant: Tenant,
    _admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<Vec<AccountDevice>>, ApiError> {
    let devices = tenant.accounts.devices(id).await?;
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
    tenant: Tenant,
    admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<Account>, ApiError> {
    let marked = tenant.accounts.schedule_deletion(admin.id, id).await?;
    Ok(Json(account(marked)))
}

/// Takes back a pending deletion; the user may sign in again unless disabled.
async fn cancel_deletion(
    tenant: Tenant,
    _admin: AdminUser,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<Account>, ApiError> {
    Ok(Json(account(tenant.accounts.cancel_deletion(id).await?)))
}

async fn status(
    State(state): State<AppState>,
    tenant: Tenant,
    _admin: AdminUser,
) -> Result<Json<ServerStatus>, ApiError> {
    let report = tenant.accounts.storage_report().await?;
    Ok(Json(ServerStatus {
        version: state.info.version.clone(),
        started_at: state.info.started_at,
        disk: DiskStatus {
            path: state.files.store().root().display().to_string(),
            space: disk_space(&state).await,
        },
        users: report
            .users
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
            version: report.database.version,
            migrations: report.database.migrations,
        },
    }))
}
