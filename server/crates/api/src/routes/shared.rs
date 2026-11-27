//! `/shared`: shares other users made for the caller, read-only. The caller browses and
//! downloads them, and saves copies into their own files.

use axum::{
    Json, Router,
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use uuid::Uuid;

use protocol::{
    CopyShared, Node, NodeId, ReceivedShare, ShareArchiveQuery, SharedFolderQuery, SharedNode,
    TicketRequest,
};

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant, TicketQuery},
    support::{
        archive::{archive_name, zip_response},
        convert::{received_share, shared_node},
        download::{Download, ranged},
        outcome::saved_at_top,
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

async fn list(tenant: Tenant, user: AuthUser) -> Result<Json<Vec<ReceivedShare>>, ApiError> {
    let shares = tenant.shares.received_all(user.id).await?;
    Ok(Json(shares.into_iter().map(received_share).collect()))
}

async fn nodes(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(folder): ApiQuery<SharedFolderQuery>,
) -> Result<Json<Vec<SharedNode>>, ApiError> {
    let children = tenant
        .shares
        .received_children(user.id, id, folder.parent_id)
        .await?;
    Ok(Json(children.into_iter().map(shared_node).collect()))
}

/// A file of the share, answering one `Range` of bytes when asked.
async fn content(
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
    let (node, file) = tenant.shares.received_file(user, id, node_id).await?;
    ranged(&headers, Download::of(&node), file).await
}

/// Everything the share holds, or one folder inside it, as one zip.
async fn archive(
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
    let (owner, entries) = tenant
        .shares
        .received_archive(user, id, archive.folder_id)
        .await?;
    Ok(zip_response(&archive_name(&owner, &entries), entries))
}

/// Saves copies of shared items into the caller's files and answers the copies at the top.
async fn copy(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CopyShared>,
) -> Result<Json<Vec<Node>>, ApiError> {
    if let Some(parent) = request.parent_id {
        tenant
            .vault
            .refuse_protected(
                user.id,
                &[parent],
                "Shared items cannot be copied into a protected folder, where they would be \
                 unencrypted.",
            )
            .await?;
    }
    let saved = tenant
        .shares
        .copy_received(
            user.id,
            user.device_id,
            id,
            &request.node_ids,
            request.parent_id,
        )
        .await?;
    Ok(Json(saved_at_top(
        request.parent_id,
        saved.folders,
        saved.files,
    )))
}
