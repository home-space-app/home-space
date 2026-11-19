use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post, put},
};

use files::NodeUpdate;
use protocol::{
    ContentQuery, CreateFolderRequest, FileVersion, ListQuery, MoveRequest, MoveResponse, Node,
    NodeId, NodeProperties, TicketRequest, UpdateRequest, UploadQuery,
};

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant, TicketQuery},
    support::{
        body::{body_stream, declared_type},
        convert::move_response,
        download::{Download, ranged},
        outcome::stored,
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/nodes", get(list))
        .route("/nodes/move", post(move_nodes))
        .route("/nodes/{id}", get(get_node).patch(update).delete(delete))
        .route("/nodes/{id}/ancestors", get(ancestors))
        .route("/nodes/{id}/properties", get(properties))
        .route("/folders", post(create_folder))
        .route("/files", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/nodes/{id}/versions", get(list_versions))
        .route(
            "/nodes/{id}/versions/{version}",
            axum::routing::delete(delete_version),
        )
        .route(
            "/files/{id}/versions/{version}/content",
            get(download_version),
        )
        .route(
            "/files/{id}/content",
            put(replace_content)
                .layer(DefaultBodyLimit::disable())
                .get(download),
        )
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let nodes = state.files.nodes.list(user.id, query.parent_id).await?;
    Ok(Json(nodes.into_iter().map(Node::from).collect()))
}

async fn get_node(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<Node>, ApiError> {
    Ok(Json(state.files.nodes.get(user.id, id).await?.into()))
}

async fn ancestors(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let nodes = state.files.nodes.ancestors(user.id, id).await?;
    Ok(Json(nodes.into_iter().map(Node::from).collect()))
}

async fn properties(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<NodeProperties>, ApiError> {
    let properties = state.files.nodes.properties(user.id, id).await?;
    Ok(Json(NodeProperties {
        source_device: properties.source_device,
    }))
}

async fn create_folder(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(request): ApiJson<CreateFolderRequest>,
) -> Result<(StatusCode, Json<Node>), ApiError> {
    let node = state
        .files
        .nodes
        .create_folder(user.actor(), request.parent_id, &request.name)
        .await?;
    Ok((StatusCode::CREATED, Json(node.into())))
}

async fn upload(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<Node>), ApiError> {
    let outcome = state
        .files
        .nodes
        .upload(
            user.actor(),
            query.parent_id,
            &query.name,
            declared_type(&headers),
            query.modified_at,
            body_stream(body),
        )
        .await?;
    Ok(stored(outcome))
}

async fn replace_content(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    ApiPath(id): ApiPath<NodeId>,
    ApiQuery(query): ApiQuery<ContentQuery>,
    body: Body,
) -> Result<Json<Node>, ApiError> {
    let outcome = state
        .files
        .nodes
        .replace_content(
            user.actor(),
            id,
            declared_type(&headers),
            query.modified_at,
            body_stream(body),
        )
        .await?;
    Ok(Json(outcome.into_node().into()))
}

async fn download(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<TicketQuery>,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Response, ApiError> {
    let owner = tenant
        .caller(
            &headers,
            query.ticket.as_deref(),
            &TicketRequest::Download { node_id: id },
        )
        .await?;
    let (node, file) = state.files.nodes.download(owner, id).await?;
    ranged(&headers, Download::of(&node), file).await
}

async fn download_version(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<TicketQuery>,
    ApiPath((id, version)): ApiPath<(NodeId, i32)>,
) -> Result<Response, ApiError> {
    let owner = tenant
        .caller(
            &headers,
            query.ticket.as_deref(),
            &TicketRequest::DownloadVersion {
                node_id: id,
                version,
            },
        )
        .await?;
    let (node, meta, file) = state
        .files
        .versions
        .download_version(owner, id, version)
        .await?;
    let download = Download {
        name: &node.name,
        mime: Some(&meta.mime),
        size: u64::try_from(meta.size).unwrap_or_default(),
    };
    ranged(&headers, download, file).await
}

async fn list_versions(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<Vec<FileVersion>>, ApiError> {
    let versions = state.files.versions.versions(user.id, id).await?;
    Ok(Json(versions.into_iter().map(FileVersion::from).collect()))
}

async fn delete_version(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((id, version)): ApiPath<(NodeId, i32)>,
) -> Result<StatusCode, ApiError> {
    state
        .files
        .versions
        .delete_version(user.id, id, version)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
    ApiJson(request): ApiJson<UpdateRequest>,
) -> Result<Json<Node>, ApiError> {
    let changes = NodeUpdate {
        name: request.name,
        parent_id: request.parent_id,
    };
    let outcome = state.files.nodes.update(user.actor(), id, changes).await?;
    Ok(Json(outcome.node.into()))
}

async fn move_nodes(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(request): ApiJson<MoveRequest>,
) -> Result<Json<MoveResponse>, ApiError> {
    let batch = state
        .files
        .nodes
        .move_nodes(
            user.actor(),
            &request.ids,
            request.parent_id,
            request.on_conflict,
        )
        .await?;
    Ok(Json(move_response(batch)))
}

async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<StatusCode, ApiError> {
    state.files.trash.delete(user.actor(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}
