use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post, put},
};

use db::{MoveOutcome, NodeUpdate};
use notifications::Event;
use protocol::{
    ContentQuery, CreateFolderRequest, FileVersion, ListQuery, MoveRequest, MoveResponse, Node,
    NodeId, NodeProperties, TicketRequest, UpdateRequest, UploadQuery,
};

use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant, TicketQuery},
    support::{
        body::{body_stream, declared_type},
        convert::move_response,
        download::attachment,
        publish::{publish_changes, publish_stored},
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
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let nodes = state.files.list(user.id, query.parent_id).await?;
    Ok(Json(nodes.into_iter().map(Node::from).collect()))
}

async fn get_node(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<NodeId>,
) -> Result<Json<Node>, ApiError> {
    Ok(Json(state.files.get(user.id, id).await?.into()))
}

async fn ancestors(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<NodeId>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let nodes = state.files.ancestors(user.id, id).await?;
    Ok(Json(nodes.into_iter().map(Node::from).collect()))
}

async fn properties(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<NodeId>,
) -> Result<Json<NodeProperties>, ApiError> {
    let properties = state.files.properties(user.id, id).await?;
    Ok(Json(NodeProperties {
        source_device: properties.source_device,
    }))
}

async fn create_folder(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateFolderRequest>,
) -> Result<(StatusCode, Json<Node>), ApiError> {
    let node = state
        .files
        .on_device(user.device_id)
        .create_folder(user.id, request.parent_id, &request.name)
        .await?;
    state.index_name(&node).await;
    let node = Node::from(node);
    state
        .hub
        .publish(user.id, Event::NodeCreated { node: node.clone() });
    Ok((StatusCode::CREATED, Json(node)))
}

async fn upload(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<Node>), ApiError> {
    let outcome = state
        .files
        .on_device(user.device_id)
        .upload(
            user.id,
            query.parent_id,
            &query.name,
            declared_type(&headers),
            query.modified_at,
            body_stream(body),
        )
        .await?;
    Ok(publish_stored(&state, user.id, outcome).await)
}

async fn replace_content(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<NodeId>,
    Query(query): Query<ContentQuery>,
    body: Body,
) -> Result<Json<Node>, ApiError> {
    let outcome = state
        .files
        .on_device(user.device_id)
        .replace_content(
            user.id,
            id,
            declared_type(&headers),
            query.modified_at,
            body_stream(body),
        )
        .await?;
    let (_, Json(node)) = publish_stored(&state, user.id, outcome).await;
    Ok(Json(node))
}

async fn download(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    Path(id): Path<NodeId>,
) -> Result<Response, ApiError> {
    let owner = tenant
        .caller(
            &headers,
            query.ticket.as_deref(),
            &TicketRequest::Download { node_id: id },
        )
        .await?;
    let (node, file) = state.files.download(owner, id).await?;
    Ok(attachment(&node.name, &node.mime, node.size, file))
}

async fn download_version(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    Path((id, version)): Path<(NodeId, i32)>,
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
    let (node, meta, file) = state.files.download_version(owner, id, version).await?;
    Ok(attachment(&node.name, &meta.mime, meta.size, file))
}

async fn list_versions(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<NodeId>,
) -> Result<Json<Vec<FileVersion>>, ApiError> {
    let versions = state.files.versions(user.id, id).await?;
    Ok(Json(versions.into_iter().map(FileVersion::from).collect()))
}

async fn delete_version(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, version)): Path<(NodeId, i32)>,
) -> Result<StatusCode, ApiError> {
    state.files.delete_version(user.id, id, version).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<NodeId>,
    Json(request): Json<UpdateRequest>,
) -> Result<Json<Node>, ApiError> {
    let changes = NodeUpdate {
        name: request.name,
        parent_id: request.parent_id,
    };
    let outcome = state
        .files
        .on_device(user.device_id)
        .update(user.id, id, changes)
        .await?;
    let node = publish_moved(&state, user.id, outcome).await;
    Ok(Json(node))
}

async fn move_nodes(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<MoveRequest>,
) -> Result<Json<MoveResponse>, ApiError> {
    let batch = state
        .files
        .on_device(user.device_id)
        .move_nodes(
            user.id,
            &request.ids,
            request.parent_id,
            request.on_conflict,
        )
        .await?;
    publish_changes(
        &state,
        user.id,
        &batch.updated,
        &batch.deleted,
        batch.content_changed.clone(),
    )
    .await;
    if batch.binned {
        state.hub.publish(user.id, Event::TrashChanged);
    }
    Ok(Json(move_response(batch)))
}

async fn publish_moved(state: &AppState, owner: db::UserId, outcome: MoveOutcome) -> Node {
    publish_changes(
        state,
        owner,
        &outcome.updated,
        &outcome.deleted,
        outcome.content_changed,
    )
    .await;
    outcome.node.into()
}

async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<NodeId>,
) -> Result<StatusCode, ApiError> {
    let node = state
        .files
        .on_device(user.device_id)
        .delete(user.id, id)
        .await?
        .node;
    state.hub.publish(
        user.id,
        Event::NodeDeleted {
            id: node.id,
            parent_id: node.parent_id,
        },
    );
    state.hub.publish(user.id, Event::TrashChanged);
    Ok(StatusCode::NO_CONTENT)
}
