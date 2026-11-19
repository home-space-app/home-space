use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{delete, get, post},
};

use db::{MoveBatch, UserId};
use notifications::Event;
use protocol::{MoveResponse, Node, NodeId, NodeKind, RestoreRequest, TrashItem};

use crate::extract::{ApiJson, ApiPath};
use crate::{
    ApiError, AppState,
    extract::AuthUser,
    support::{convert::move_response, publish::publish_changes},
};

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
    let items = state.files.trash(user.id).await?;
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
    let mut batch = state
        .files
        .on_device(user.device_id)
        .restore(user.id, &request.ids, request.on_conflict)
        .await?;
    // Clients never see the bin folder, so binned files report no parent.
    let bin = state
        .db
        .special_folders(user.id)
        .await
        .map_err(files::FilesError::from)?
        .trash_id;
    let binned = batch
        .deleted
        .iter_mut()
        .chain(
            batch
                .conflicts
                .iter_mut()
                .map(|conflict| &mut conflict.source),
        )
        .filter(|node| node.parent_id == Some(bin));
    for node in binned {
        node.parent_id = None;
    }
    publish_changes(
        &state,
        user.id,
        &batch.updated,
        &batch.deleted,
        batch.content_changed.clone(),
    )
    .await;
    reindex_content(&state, user.id, &batch);
    if !batch.moved.is_empty() {
        state.hub.publish(user.id, Event::TrashChanged);
    }
    Ok(Json(move_response(batch)))
}

/// Restored files were not indexed while binned, so their content is extracted again.
fn reindex_content(state: &AppState, owner: UserId, batch: &MoveBatch) {
    for node in batch
        .updated
        .iter()
        .filter(|node| node.kind == NodeKind::File)
    {
        state.search.enqueue_content(owner, node.id);
    }
}

async fn empty(State(state): State<AppState>, user: AuthUser) -> Result<StatusCode, ApiError> {
    state
        .files
        .on_device(user.device_id)
        .empty_trash(user.id)
        .await?;
    state.hub.publish(user.id, Event::TrashChanged);
    Ok(StatusCode::NO_CONTENT)
}

async fn purge(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<StatusCode, ApiError> {
    state
        .files
        .on_device(user.device_id)
        .purge_trashed(user.id, id)
        .await?;
    state.hub.publish(user.id, Event::TrashChanged);
    Ok(StatusCode::NO_CONTENT)
}
