use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{delete, get, post},
};

use protocol::{MoveResponse, Node, NodeId, RestoreRequest, TrashItem};

use crate::extract::{ApiJson, ApiPath};
use crate::{ApiError, AppState, extract::AuthUser, support::convert::move_response};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/trash", get(list).delete(empty))
        .route("/trash/restore", post(restore))
        .route("/trash/{id}", delete(purge))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<TrashItem>>, ApiError> {
    let items = state.files.trash.trash(user.id).await?;
    Ok(Json(
        items
            .into_iter()
            .map(|item| TrashItem {
                node: Node {
                    parent_id: None,
                    ..item.node.into()
                },
                original_path: item.original_path,
                deleted_at: item.trashed_at,
            })
            .collect(),
    ))
}

async fn restore(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(request): ApiJson<RestoreRequest>,
) -> Result<Json<MoveResponse>, ApiError> {
    let batch = state
        .files
        .trash
        .restore(user.actor(), &request.ids, request.on_conflict)
        .await?;
    // What goes back into a password folder keeps its sealed name; see the recycle bin in
    // docs/todo/protected-folders.md.
    Ok(Json(move_response(batch)))
}

async fn empty(State(state): State<AppState>, user: AuthUser) -> Result<StatusCode, ApiError> {
    state.files.trash.empty_trash(user.actor()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn purge(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<StatusCode, ApiError> {
    state.files.trash.purge_trashed(user.actor(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}
