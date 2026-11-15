//! Read-only shares: the owner's `/shares`, for named users or as a link, and the visitors'
//! `/public/shares/{token}`.

use std::collections::{HashMap, HashSet};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, patch, post},
};
use time::OffsetDateTime;
use uuid::Uuid;

use db::{DbError, NewShareLink, NewUserShare, NodeKind, SharePermission, ShareUpdate, UserId};
use protocol::{
    CreateShare, NodeId, PublicShare, Share, ShareAccess, ShareAccessQuery, ShareArchiveQuery,
    ShareRecipient, SharedFolderQuery, SharedNode, SharesQuery, UnlockShare, UpdateShare, UserRef,
    paths::SHARE_ACCESS,
};

use crate::{
    ApiError, AppState,
    extract::{AuthUser, ClientIp},
    routes::{
        archive::zip_response,
        content::{Download, ranged},
    },
};

/// The most items one share may hold.
const MAX_SHARE_ITEMS: usize = 1000;
/// The most users one share may be for.
const MAX_RECIPIENTS: usize = 100;
/// Names the zip of a link with several items, before the owner's name.
const SHARED_BY: &str = "Shared by";
/// No token is longer; anything longer is refused before the database is asked.
const MAX_TOKEN_LEN: usize = 128;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/shares", post(create).get(list))
        .route("/shares/{id}", patch(update).delete(remove))
        .route("/public/shares/{token}", get(public_share))
        .route("/public/shares/{token}/unlock", post(unlock))
        .route("/public/shares/{token}/nodes", get(public_nodes))
        .route(
            "/public/shares/{token}/files/{id}/content",
            get(public_content),
        )
        .route("/public/shares/{token}/archive", get(public_archive))
}

pub(super) fn db_error(err: DbError) -> ApiError {
    match err {
        DbError::NotFound => ApiError::NotFound,
        other => ApiError::Internal(other.to_string()),
    }
}

pub(super) fn shared_node(node: db::Node) -> SharedNode {
    SharedNode {
        id: node.id,
        kind: node.kind,
        name: node.name,
        size: node.size,
        mime: node.mime,
        updated_at: node.updated_at,
    }
}

/// `ids` without repeats, in their first order.
fn unique<T: Copy + Eq + std::hash::Hash>(ids: &[T]) -> Vec<T> {
    let mut seen = HashSet::new();
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

/// `recipients` without repeats; refuses none, too many, or the owner.
fn check_recipients(owner: UserId, recipients: &[UserId]) -> Result<Vec<UserId>, ApiError> {
    let recipients = unique(recipients);
    if recipients.is_empty() || recipients.len() > MAX_RECIPIENTS {
        Err(ApiError::BadRequest(format!(
            "a share is for 1 to {MAX_RECIPIENTS} users"
        )))
    } else if recipients.contains(&owner) {
        Err(ApiError::BadRequest(
            "a share cannot be for its owner".to_owned(),
        ))
    } else {
        Ok(recipients)
    }
}

/// Refuses an expiry that has already passed.
fn check_expiry(expires_at: Option<OffsetDateTime>) -> Result<(), ApiError> {
    if expires_at.is_some_and(|expires_at| expires_at <= OffsetDateTime::now_utc()) {
        Err(ApiError::BadRequest(
            "the expiry must be in the future".to_owned(),
        ))
    } else {
        Ok(())
    }
}

/// The owner's view of `shares`, with their live items and recipients.
async fn owner_views(
    state: &AppState,
    shares: Vec<db::OwnedShare>,
) -> Result<Vec<Share>, ApiError> {
    let ids: Vec<Uuid> = shares.iter().map(|share| share.id).collect();
    let mut items = state.db.share_items(&ids).await.map_err(db_error)?;
    let mut recipients = state.db.share_recipients(&ids).await.map_err(db_error)?;
    Ok(shares
        .into_iter()
        .map(|share| Share {
            id: share.id,
            items: items
                .remove(&share.id)
                .unwrap_or_default()
                .into_iter()
                .map(protocol::Node::from)
                .collect(),
            has_password: share.has_password,
            permission: share.permission,
            token: share.token,
            expires_at: share.expires_at,
            created_at: share.created_at,
            recipients: recipients
                .remove(&share.id)
                .unwrap_or_default()
                .into_iter()
                .map(|recipient| ShareRecipient {
                    user: UserRef {
                        id: recipient.user_id,
                        username: recipient.username,
                    },
                    permission: recipient.permission,
                    added_at: recipient.created_at,
                })
                .collect(),
        })
        .collect())
}

async fn owner_view(state: &AppState, share: db::OwnedShare) -> Result<Share, ApiError> {
    owner_views(state, vec![share])
        .await?
        .pop()
        .ok_or_else(|| ApiError::Internal("share view missing".to_owned()))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateShare>,
) -> Result<(StatusCode, Json<Share>), ApiError> {
    let node_ids = unique(&request.node_ids);
    if node_ids.is_empty() || node_ids.len() > MAX_SHARE_ITEMS {
        return Err(ApiError::BadRequest(format!(
            "a share holds 1 to {MAX_SHARE_ITEMS} items"
        )));
    }
    check_expiry(request.expires_at)?;
    let share = if request.recipients.is_empty() {
        create_link(&state, user.id, &request, &node_ids).await?
    } else {
        if request.password.is_some() {
            return Err(ApiError::BadRequest(
                "a share with users has no password".to_owned(),
            ));
        }
        let recipients = check_recipients(user.id, &request.recipients)?;
        let new = NewUserShare {
            id: Uuid::now_v7(),
            expires_at: request.expires_at,
        };
        state
            .db
            .create_user_share(user.id, new, &node_ids, &recipients)
            .await
            .map_err(db_error)?
    };
    Ok((StatusCode::CREATED, Json(owner_view(&state, share).await?)))
}

async fn create_link(
    state: &AppState,
    owner: UserId,
    request: &CreateShare,
    node_ids: &[NodeId],
) -> Result<db::OwnedShare, ApiError> {
    let password_hash = match request.password.as_deref() {
        Some(password) => Some(state.auth.hash_share_password(password).await?),
        None => None,
    };
    let token = auth::new_share_token();
    let new = NewShareLink {
        id: Uuid::now_v7(),
        token: &token,
        password_hash: password_hash.as_deref(),
        permission: SharePermission::Read,
        expires_at: request.expires_at,
    };
    let link = state
        .db
        .create_share_link(owner, new, node_ids)
        .await
        .map_err(db_error)?;
    Ok(link.into())
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<SharesQuery>,
) -> Result<Json<Vec<Share>>, ApiError> {
    let shares = state
        .db
        .owned_shares(user.id, query.node_id)
        .await
        .map_err(db_error)?;
    Ok(Json(owner_views(&state, shares).await?))
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateShare>,
) -> Result<Json<Share>, ApiError> {
    check_expiry(request.expires_at.flatten())?;
    let current = state.db.owned_share(user.id, id).await.map_err(db_error)?;
    let is_link = current.token.is_some();
    if is_link && request.recipients.is_some() {
        return Err(ApiError::BadRequest(
            "a link is not for named users".to_owned(),
        ));
    }
    if !is_link && request.password.is_some() {
        return Err(ApiError::BadRequest(
            "a share with users has no password".to_owned(),
        ));
    }
    let recipients = request
        .recipients
        .as_deref()
        .map(|recipients| check_recipients(user.id, recipients))
        .transpose()?;
    let password_hash = match request.password {
        Some(Some(password)) => Some(Some(state.auth.hash_share_password(&password).await?)),
        Some(None) => Some(None),
        None => None,
    };
    let update = ShareUpdate {
        password_hash: password_hash.as_ref().map(Option::as_deref),
        expires_at: request.expires_at,
        recipients: recipients.as_deref(),
    };
    let share = state
        .db
        .update_share(user.id, id, update)
        .await
        .map_err(db_error)?;
    Ok(Json(owner_view(&state, share).await?))
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.db.delete_share(user.id, id).await.map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The share link of `token`: `NotFound` when there is none, `Gone` once it has expired.
async fn resolve(state: &AppState, token: &str) -> Result<db::ShareLink, ApiError> {
    if token.len() > MAX_TOKEN_LEN {
        return Err(ApiError::NotFound);
    }
    let share = state.db.share_by_token(token).await.map_err(db_error)?;
    if share.is_expired(OffsetDateTime::now_utc()) {
        Err(ApiError::Gone)
    } else {
        Ok(share)
    }
}

/// Whether the request may read `share`: it has no password, or the request carries a valid
/// access token for its current one.
fn is_unlocked(
    state: &AppState,
    share: &db::ShareLink,
    headers: &HeaderMap,
    query: &ShareAccessQuery,
) -> bool {
    let access = headers
        .get(SHARE_ACCESS)
        .and_then(|value| value.to_str().ok())
        .or(query.access.as_deref());
    share.password_hash.is_none()
        || access.is_some_and(|access| state.auth.verifies_share_access(share, access))
}

/// The share link of `token`, when the request may read it.
async fn readable(
    state: &AppState,
    token: &str,
    headers: &HeaderMap,
    query: &ShareAccessQuery,
) -> Result<db::ShareLink, ApiError> {
    let share = resolve(state, token).await?;
    if is_unlocked(state, &share, headers, query) {
        Ok(share)
    } else {
        Err(ApiError::Unauthorized)
    }
}

async fn public_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Query(query): Query<ShareAccessQuery>,
) -> Result<Json<PublicShare>, ApiError> {
    let share = resolve(&state, &token).await?;
    let unlocked = is_unlocked(&state, &share, &headers, &query);
    let items = if unlocked {
        let mut items: HashMap<Uuid, Vec<db::Node>> =
            state.db.share_items(&[share.id]).await.map_err(db_error)?;
        Some(
            items
                .remove(&share.id)
                .unwrap_or_default()
                .into_iter()
                .map(shared_node)
                .collect(),
        )
    } else {
        None
    };
    Ok(Json(PublicShare {
        owner: share.owner_name,
        item_count: u32::try_from(share.item_count).unwrap_or(u32::MAX),
        expires_at: share.expires_at,
        needs_password: !unlocked,
        items,
    }))
}

async fn unlock(
    State(state): State<AppState>,
    ClientIp(address): ClientIp,
    Path(token): Path<String>,
    Json(request): Json<UnlockShare>,
) -> Result<Json<ShareAccess>, ApiError> {
    let share = resolve(&state, &token).await?;
    let access = state
        .auth
        .unlock_share(&share, &request.password, address)
        .await?;
    Ok(Json(access))
}

async fn public_nodes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Query(access): Query<ShareAccessQuery>,
    Query(folder): Query<SharedFolderQuery>,
) -> Result<Json<Vec<SharedNode>>, ApiError> {
    let share = readable(&state, &token, &headers, &access).await?;
    let children = state
        .db
        .shared_children(share.root(), folder.parent_id)
        .await
        .map_err(db_error)?;
    Ok(Json(children.into_iter().map(shared_node).collect()))
}

async fn public_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((token, id)): Path<(String, NodeId)>,
    Query(access): Query<ShareAccessQuery>,
) -> Result<Response, ApiError> {
    let share = readable(&state, &token, &headers, &access).await?;
    let node = state
        .db
        .shared_node(share.root(), id)
        .await
        .map_err(db_error)?;
    if node.kind != NodeKind::File {
        return Err(ApiError::NotFound);
    }
    let (node, file) = state.files.download(share.owner_id, node.id).await?;
    let download = Download {
        name: &node.name,
        mime: Some(&node.mime),
        size: u64::try_from(node.size).unwrap_or_default(),
    };
    ranged(&headers, download, file).await
}

/// Everything the link shares, or one folder inside it, as one zip.
async fn public_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Query(access): Query<ShareAccessQuery>,
    Query(archive): Query<ShareArchiveQuery>,
) -> Result<Response, ApiError> {
    let share = readable(&state, &token, &headers, &access).await?;
    let entries = state
        .files
        .share_archive(share.root(), archive.folder_id)
        .await?;
    let name = archive_name(&share.owner_name, &entries);
    Ok(zip_response(&name, entries))
}

/// The name of a share's zip: its only top-level entry, or who shared it.
pub(super) fn archive_name(owner_name: &str, entries: &[files::ArchiveEntry]) -> String {
    let mut top = entries.iter().filter(|entry| !entry.path.contains('/'));
    match (top.next(), top.next()) {
        (Some(only), None) => only.path.clone(),
        _ => format!("{SHARED_BY} {owner_name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_items_are_shared_once_in_their_first_order() {
        let a = NodeId::new();
        let b = NodeId::new();
        assert_eq!(unique(&[a, b, a, b]), vec![a, b]);
    }
}
