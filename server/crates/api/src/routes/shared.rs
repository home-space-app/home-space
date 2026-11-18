//! `/shared`: shares other users made for the caller, read-only. The caller browses and
//! downloads them, and saves copies into their own files.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use time::OffsetDateTime;
use uuid::Uuid;

use db::{NodeKind, UserId};
use protocol::{
    CopyShared, Node, NodeId, ReceivedShare, ShareArchiveQuery, SharedFolderQuery, SharedNode,
    TicketRequest, UserRef,
};

use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant, TicketQuery},
    routes::{
        archive::zip_response,
        content::{Download, ranged},
        shares::{archive_name, db_error, shared_node},
        transfers::publish_saved,
    },
};

/// The most items one copy may name.
const MAX_COPY_ITEMS: usize = 1000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/shared", get(list))
        .route("/shared/{id}/nodes", get(nodes))
        .route("/shared/{id}/files/{node}/content", get(content))
        .route("/shared/{id}/archive", get(archive))
        .route("/shared/{id}/copy", post(copy))
}

/// The share `id` made for `user`: `NotFound` when there is none, `Gone` once it has expired.
async fn readable(state: &AppState, user: UserId, id: Uuid) -> Result<db::ReceivedShare, ApiError> {
    let share = state.db.received_share(user, id).await.map_err(db_error)?;
    if share.is_expired(OffsetDateTime::now_utc()) {
        Err(ApiError::Gone)
    } else {
        Ok(share)
    }
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<ReceivedShare>>, ApiError> {
    let shares = state
        .db
        .received_shares(user.id, OffsetDateTime::now_utc())
        .await
        .map_err(db_error)?;
    let ids: Vec<Uuid> = shares.iter().map(|share| share.id).collect();
    let mut items = state.db.share_items(&ids).await.map_err(db_error)?;
    Ok(Json(
        shares
            .into_iter()
            .map(|share| ReceivedShare {
                id: share.id,
                owner: UserRef {
                    id: share.owner_id,
                    username: share.owner_name,
                },
                items: items
                    .remove(&share.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(shared_node)
                    .collect(),
                expires_at: share.expires_at,
                shared_at: share.shared_at,
            })
            .filter(|share| !share.items.is_empty())
            .collect(),
    ))
}

async fn nodes(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(folder): Query<SharedFolderQuery>,
) -> Result<Json<Vec<SharedNode>>, ApiError> {
    let share = readable(&state, user.id, id).await?;
    let children = state
        .db
        .shared_children(share.root(), folder.parent_id)
        .await
        .map_err(db_error)?;
    Ok(Json(children.into_iter().map(shared_node).collect()))
}

/// A file of the share, answering one `Range` of bytes when asked.
async fn content(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    Path((id, node_id)): Path<(Uuid, NodeId)>,
) -> Result<Response, ApiError> {
    let request = TicketRequest::SharedFile {
        share_id: id,
        node_id,
    };
    let user = tenant
        .caller(&headers, query.ticket.as_deref(), &request)
        .await?;
    let share = readable(&state, user, id).await?;
    let node = state
        .db
        .shared_node(share.root(), node_id)
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

/// Everything the share holds, or one folder inside it, as one zip.
async fn archive(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    Query(archive): Query<ShareArchiveQuery>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let request = TicketRequest::SharedArchive { share_id: id };
    let user = tenant
        .caller(&headers, query.ticket.as_deref(), &request)
        .await?;
    let share = readable(&state, user, id).await?;
    let entries = state
        .files
        .share_archive(share.root(), archive.folder_id)
        .await?;
    let name = archive_name(&share.owner_name, &entries);
    Ok(zip_response(&name, entries))
}

/// Saves copies of shared items into the caller's files and answers the copies at the top.
async fn copy(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(request): Json<CopyShared>,
) -> Result<Json<Vec<Node>>, ApiError> {
    if request.node_ids.is_empty() || request.node_ids.len() > MAX_COPY_ITEMS {
        return Err(ApiError::BadRequest(format!(
            "a copy holds 1 to {MAX_COPY_ITEMS} items"
        )));
    }
    let share = readable(&state, user.id, id).await?;
    let saved = state
        .files
        .on_device(user.device_id)
        .copy_shared(user.id, share.root(), &request.node_ids, request.parent_id)
        .await?;
    Ok(Json(
        publish_saved(
            &state,
            user.id,
            request.parent_id,
            saved.folders,
            saved.files,
        )
        .await,
    ))
}
