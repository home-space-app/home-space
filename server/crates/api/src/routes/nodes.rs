use std::io;

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::Response,
    routing::{get, post, put},
};
use futures_util::TryStreamExt;
use tokio_util::io::ReaderStream;

use db::{MoveOutcome, NodeUpdate, StoreOutcome};
use notifications::Event;
use protocol::{
    ContentQuery, CreateFolderRequest, FileVersion, ListQuery, MoveRequest, MoveResponse, Node,
    NodeId, NodeProperties, TicketRequest, UpdateRequest, UploadQuery,
};

use crate::{
    ApiError, AppState,
    extract::{AuthUser, TicketQuery},
    routes::trash::move_response,
};

const FALLBACK_MIME: &str = "application/octet-stream";

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

pub(crate) fn body_stream(
    body: Body,
) -> impl futures_util::Stream<Item = io::Result<axum::body::Bytes>> {
    body.into_data_stream().map_err(io::Error::other)
}

pub(crate) fn declared_type(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
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

pub(crate) async fn publish_stored(
    state: &AppState,
    owner: db::UserId,
    outcome: StoreOutcome,
) -> (StatusCode, Json<Node>) {
    match outcome {
        StoreOutcome::Created(node) => {
            state.index_name(&node).await;
            state.search.enqueue_content(owner, node.id);
            let node = Node::from(node);
            state
                .hub
                .publish(owner, Event::NodeCreated { node: node.clone() });
            (StatusCode::CREATED, Json(node))
        }
        StoreOutcome::Archived { node, .. } => {
            let node = Node::from(node);
            state
                .hub
                .publish(owner, Event::NodeUpdated { node: node.clone() });
            (StatusCode::OK, Json(node))
        }
        StoreOutcome::Versioned { node, .. } => {
            state.search.enqueue_content(owner, node.id);
            let node = Node::from(node);
            state
                .hub
                .publish(owner, Event::NodeUpdated { node: node.clone() });
            (StatusCode::OK, Json(node))
        }
        StoreOutcome::Unchanged(node) => (StatusCode::OK, Json(node.into())),
    }
}

async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    Path(id): Path<NodeId>,
) -> Result<Response, ApiError> {
    let owner = match query.ticket {
        Some(ticket) => {
            state
                .auth
                .verify_ticket(&ticket, &TicketRequest::Download { node_id: id })
                .await?
                .user_id
        }
        None => AuthUser::from_headers(&headers, &state).await?.id,
    };
    let (node, file) = state.files.download(owner, id).await?;
    Ok(attachment(&node.name, &node.mime, node.size, file))
}

async fn download_version(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    Path((id, version)): Path<(NodeId, i32)>,
) -> Result<Response, ApiError> {
    let owner = match query.ticket {
        Some(ticket) => {
            state
                .auth
                .verify_ticket(
                    &ticket,
                    &TicketRequest::DownloadVersion {
                        node_id: id,
                        version,
                    },
                )
                .await?
                .user_id
        }
        None => AuthUser::from_headers(&headers, &state).await?.id,
    };
    let (node, meta, file) = state.files.download_version(owner, id, version).await?;
    Ok(attachment(&node.name, &meta.mime, meta.size, file))
}

fn attachment(name: &str, mime: &str, size: i64, file: tokio::fs::File) -> Response {
    let content_type = if mime.is_empty() { FALLBACK_MIME } else { mime };
    let disposition = format!("attachment; filename*=UTF-8''{}", percent_encode(name));
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    let headers = response.headers_mut();
    headers.insert(CONTENT_LENGTH, HeaderValue::from(size.unsigned_abs()));
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    response
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

/// Indexes and announces what a move or restore changed.
pub(super) async fn publish_changes(
    state: &AppState,
    owner: db::UserId,
    updated: &[db::Node],
    deleted: &[db::Node],
    content_changed: Vec<NodeId>,
) {
    for node in updated {
        state.index_name(node).await;
        let event = Event::NodeUpdated {
            node: node.clone().into(),
        };
        state.hub.publish(owner, event);
    }
    for node in deleted {
        state.hub.publish(
            owner,
            Event::NodeDeleted {
                id: node.id,
                parent_id: node.parent_id,
            },
        );
    }
    for id in content_changed {
        state.search.enqueue_content(owner, id);
    }
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

pub(crate) fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
