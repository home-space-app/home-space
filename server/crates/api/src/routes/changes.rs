use axum::{Json, Router, extract::State, routing::get};

use protocol::{Change, ChangesQuery, ChangesResponse, Node, NodeId};

use crate::extract::ApiQuery;
use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant},
};

const DEFAULT_LIMIT: u32 = 500;
const MAX_LIMIT: u32 = 1000;

pub fn routes() -> Router<AppState> {
    Router::new().route("/changes", get(changes))
}

/// The journal of changes after `since`. Nodes inside a password folder the session has not
/// unlocked are left out, and those inside one it unlocked carry their plain names; a client
/// lists such a folder again once it unlocks it.
async fn changes(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ChangesQuery>,
) -> Result<Json<ChangesResponse>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let batch = state
        .files
        .nodes
        .changes(user.id, query.since, i64::from(limit))
        .await?;
    let parents: Vec<NodeId> = batch
        .changes
        .iter()
        .filter_map(|change| change.node.as_ref()?.parent_id)
        .collect();
    let shown = tenant
        .vault
        .shown(user.id, user.session_id, &parents)
        .await?;
    Ok(Json(ChangesResponse {
        changes: batch
            .changes
            .into_iter()
            .filter_map(|change| match change.node {
                Some(mut node) => {
                    node.name = shown.name(node.parent_id, node.kind, &node.name)?;
                    Some(Change::Upserted {
                        seq: change.seq,
                        node: Node::from(node),
                        device_id: change.device_id,
                    })
                }
                None => Some(Change::Deleted {
                    seq: change.seq,
                    id: change.id,
                    device_id: change.device_id,
                }),
            })
            .collect(),
        cursor: batch.cursor,
        more: batch.more,
    }))
}
