//! `/shared`: shares other users made for the caller, read-only. The caller browses and
//! downloads them, and saves copies into their own files.

use axum::{
    Json, Router,
    extract::State,
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

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant, TicketQuery},
    support::{
        archive::{archive_name, zip_response},
        convert::shared_node,
        download::{Download, ranged},
        publish::publish_saved,
    },
};

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
    let share = state.db.received_share(user, id).await?;
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
        .await?;
    let ids: Vec<Uuid> = shares.iter().map(|share| share.id).collect();
    let mut items = state.db.share_items(&ids).await?;
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
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(folder): ApiQuery<SharedFolderQuery>,
) -> Result<Json<Vec<SharedNode>>, ApiError> {
    let share = readable(&state, user.id, id).await?;
    let children = state
        .db
        .shared_children(share.root(), folder.parent_id)
        .await?;
    Ok(Json(children.into_iter().map(shared_node).collect()))
}

/// A file of the share, answering one `Range` of bytes when asked.
async fn content(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<TicketQuery>,
    ApiPath((id, node_id)): ApiPath<(Uuid, NodeId)>,
) -> Result<Response, ApiError> {
    let request = TicketRequest::SharedFile {
        share_id: id,
        node_id,
    };
    let user = tenant
        .caller(&headers, query.ticket.as_deref(), &request)
        .await?;
    let share = readable(&state, user, id).await?;
    let node = state.db.shared_node(share.root(), node_id).await?;
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
    ApiQuery(query): ApiQuery<TicketQuery>,
    ApiQuery(archive): ApiQuery<ShareArchiveQuery>,
    ApiPath(id): ApiPath<Uuid>,
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
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CopyShared>,
) -> Result<Json<Vec<Node>>, ApiError> {
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
