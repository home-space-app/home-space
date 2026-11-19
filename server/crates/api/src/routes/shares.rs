//! Read-only shares: the owner's `/shares`, for named users or as a link, and the visitors'
//! `/public/shares/{token}`.

use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, patch, post},
};
use uuid::Uuid;

use protocol::{
    CreateShare, NodeId, PublicShare, Share, ShareAccess, ShareAccessQuery, ShareArchiveQuery,
    SharedFolderQuery, SharedNode, SharesQuery, UnlockShare, UpdateShare, paths::SHARE_ACCESS,
};

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{
    ApiError, AppState,
    extract::{AuthUser, ClientIp, Tenant},
    support::{
        archive::{archive_name, zip_response},
        convert::{owner_share, public_share, shared_node},
        download::{Download, ranged},
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/shares", post(create).get(list))
        .route("/shares/{id}", patch(update).delete(remove))
        .route("/public/shares/{token}", get(public))
        .route("/public/shares/{token}/unlock", post(unlock))
        .route("/public/shares/{token}/nodes", get(public_nodes))
        .route(
            "/public/shares/{token}/files/{id}/content",
            get(public_content),
        )
        .route("/public/shares/{token}/archive", get(public_archive))
}

/// The proof of a link's password the visitor sent: the header, or else the query, which is
/// all a plain download link can carry.
fn access<'a>(headers: &'a HeaderMap, query: &'a ShareAccessQuery) -> Option<&'a str> {
    headers
        .get(SHARE_ACCESS)
        .and_then(|value| value.to_str().ok())
        .or(query.access.as_deref())
}

async fn create(
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<CreateShare>,
) -> Result<(StatusCode, Json<Share>), ApiError> {
    let share = tenant.shares.create(user.id, &request).await?;
    Ok((StatusCode::CREATED, Json(owner_share(share))))
}

async fn list(
    tenant: Tenant,
    user: AuthUser,
    ApiQuery(query): ApiQuery<SharesQuery>,
) -> Result<Json<Vec<Share>>, ApiError> {
    let shares = tenant.shares.list(user.id, query.node_id).await?;
    Ok(Json(shares.into_iter().map(owner_share).collect()))
}

async fn update(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateShare>,
) -> Result<Json<Share>, ApiError> {
    let share = tenant.shares.update(user.id, id, &request).await?;
    Ok(Json(owner_share(share)))
}

async fn remove(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    tenant.shares.delete(user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn public(
    tenant: Tenant,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
    ApiQuery(query): ApiQuery<ShareAccessQuery>,
) -> Result<Json<PublicShare>, ApiError> {
    let view = tenant.shares.link(&token, access(&headers, &query)).await?;
    Ok(Json(public_share(view)))
}

async fn unlock(
    tenant: Tenant,
    ClientIp(address): ClientIp,
    ApiPath(token): ApiPath<String>,
    ApiJson(request): ApiJson<UnlockShare>,
) -> Result<Json<ShareAccess>, ApiError> {
    let access = tenant
        .shares
        .unlock(&token, &request.password, address)
        .await?;
    Ok(Json(access))
}

async fn public_nodes(
    tenant: Tenant,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
    ApiQuery(query): ApiQuery<ShareAccessQuery>,
    ApiQuery(folder): ApiQuery<SharedFolderQuery>,
) -> Result<Json<Vec<SharedNode>>, ApiError> {
    let children = tenant
        .shares
        .link_children(&token, access(&headers, &query), folder.parent_id)
        .await?;
    Ok(Json(children.into_iter().map(shared_node).collect()))
}

async fn public_content(
    tenant: Tenant,
    headers: HeaderMap,
    ApiPath((token, id)): ApiPath<(String, NodeId)>,
    ApiQuery(query): ApiQuery<ShareAccessQuery>,
) -> Result<Response, ApiError> {
    let (node, file) = tenant
        .shares
        .link_file(&token, access(&headers, &query), id)
        .await?;
    ranged(&headers, Download::of(&node), file).await
}

/// Everything the link shares, or one folder inside it, as one zip.
async fn public_archive(
    tenant: Tenant,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
    ApiQuery(query): ApiQuery<ShareAccessQuery>,
    ApiQuery(archive): ApiQuery<ShareArchiveQuery>,
) -> Result<Response, ApiError> {
    let (owner, entries) = tenant
        .shares
        .link_archive(&token, access(&headers, &query), archive.folder_id)
        .await?;
    Ok(zip_response(&archive_name(&owner, &entries), entries))
}
