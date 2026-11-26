//! The vault that unlocks the caller's protected folders, and the folders themselves (feature
//! `protected_folders`).

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{delete, get, post},
};
use uuid::Uuid;

use axum::extract::State;
use db::NodeId;
use protocol::{
    AddUnlocker, FolderLock, FolderUnlocked, ProtectFolder, ProtectedFolder, ProtectionState,
    RotateVault, UnlockFolder, Vault, VaultUnlocker,
};

use crate::{
    ApiError, AppState,
    extract::{ApiJson, ApiPath, AuthUser, ClientIp, Tenant},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/vault", get(vault))
        .route("/vault/unlockers", post(add_unlocker))
        .route("/vault/unlockers/{id}", delete(remove_unlocker))
        .route("/vault/rotate", post(rotate))
        .route("/protected", get(folders).post(protect))
        .route("/protected/{id}", delete(remove))
        .route("/protected/{id}/finish", post(finish))
        .route("/protected/{id}/unprotect", post(unprotect))
        .route("/protected/{id}/unlock", post(unlock).delete(lock))
}

/// The caller's vault: its epoch and unlockers, all useless without the keys.
async fn vault(tenant: Tenant, user: AuthUser) -> Result<Json<Vault>, ApiError> {
    Ok(Json(tenant.vault.vault(user.id).await?))
}

/// Enables a security key, or sets the recovery key.
async fn add_unlocker(
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<AddUnlocker>,
) -> Result<(StatusCode, Json<VaultUnlocker>), ApiError> {
    let added = tenant
        .vault
        .add_unlocker(user.id, user.session_id, &request)
        .await?;
    Ok((StatusCode::CREATED, Json(added)))
}

async fn remove_unlocker(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    tenant
        .vault
        .remove_unlocker(user.id, user.session_id, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Replaces the vault key, such as after losing a security key.
async fn rotate(
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<RotateVault>,
) -> Result<Json<Vault>, ApiError> {
    Ok(Json(
        tenant
            .vault
            .rotate(user.id, user.session_id, &request)
            .await?,
    ))
}

async fn folders(tenant: Tenant, user: AuthUser) -> Result<Json<Vec<ProtectedFolder>>, ApiError> {
    Ok(Json(tenant.vault.folders(user.id).await?))
}

async fn protect(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<ProtectFolder>,
) -> Result<(StatusCode, Json<ProtectedFolder>), ApiError> {
    let folder = tenant
        .vault
        .protect(user.id, user.session_id, &request)
        .await?;
    if folder.lock == FolderLock::Password && folder.state == ProtectionState::Protecting {
        // The server encrypts what the folder holds; the regular job resumes it after a stop.
        let vault = tenant.vault.clone();
        let nodes = state.files.nodes.clone();
        tokio::spawn(async move {
            if let Err(err) = vault.convert_folders(&nodes).await {
                tracing::error!(error = %err, "encrypting a password folder failed");
            }
        });
    }
    Ok((StatusCode::CREATED, Json(folder)))
}

/// Unlocks a password folder for the caller's session.
async fn unlock(
    tenant: Tenant,
    user: AuthUser,
    ClientIp(address): ClientIp,
    ApiPath(id): ApiPath<NodeId>,
    ApiJson(request): ApiJson<UnlockFolder>,
) -> Result<Json<FolderUnlocked>, ApiError> {
    Ok(Json(
        tenant
            .vault
            .unlock(user.id, user.session_id, id, &request.password, address)
            .await?,
    ))
}

/// Locks a password folder for the caller's session again.
async fn lock(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<StatusCode, ApiError> {
    tenant.vault.lock(user.id, user.session_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The client has replaced the folder's content with its encrypted form.
async fn finish(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<ProtectedFolder>, ApiError> {
    Ok(Json(tenant.vault.finish(user.id, id).await?))
}

/// Starts replacing the folder's content with its decrypted form.
async fn unprotect(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<ProtectedFolder>, ApiError> {
    Ok(Json(
        tenant.vault.unprotect(user.id, user.session_id, id).await?,
    ))
}

/// Forgets the protection of a folder whose content is plain again.
async fn remove(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<StatusCode, ApiError> {
    tenant.vault.remove(user.id, user.session_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
