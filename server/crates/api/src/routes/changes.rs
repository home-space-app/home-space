use axum::{Json, Router, extract::State, routing::get};

use protocol::{Change, ChangesQuery, ChangesResponse, Node};

use crate::extract::ApiQuery;
use crate::{ApiError, AppState, extract::AuthUser};

const DEFAULT_LIMIT: u32 = 500;
const MAX_LIMIT: u32 = 1000;

pub fn routes() -> Router<AppState> {
    Router::new().route("/changes", get(changes))
}

async fn changes(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ChangesQuery>,
) -> Result<Json<ChangesResponse>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let batch = state
        .files
        .nodes
        .changes(user.id, query.since, i64::from(limit))
        .await?;
    Ok(Json(ChangesResponse {
        changes: batch
            .changes
            .into_iter()
            .map(|change| match change.node {
                Some(node) => Change::Upserted {
                    seq: change.seq,
                    node: Node::from(node),
                    device_id: change.device_id,
                },
                None => Change::Deleted {
                    seq: change.seq,
                    id: change.id,
                    device_id: change.device_id,
                },
            })
            .collect(),
        cursor: batch.cursor,
        more: batch.more,
    }))
}
