use axum::{Json, Router, extract::State, routing::get};

use protocol::{SearchHit, SearchQuery};

use crate::extract::ApiQuery;
use crate::{ApiError, AppState, extract::AuthUser};

pub fn routes() -> Router<AppState> {
    Router::new().route("/search", get(search))
}

async fn search(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    Ok(Json(
        state.search.search(user.id, &query.q, query.limit).await?,
    ))
}
