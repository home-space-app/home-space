use axum::{
    Json, Router,
    extract::{Query, State},
    http::{
        StatusCode,
        header::{CACHE_CONTROL, REFERRER_POLICY},
    },
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use protocol::{
    ChangePasswordRequest, LoginRequest, Me, OidcCallbackQuery, OidcExchangeRequest,
    OidcStartQuery, Providers, RefreshRequest, TicketRequest, TicketResponse, TokenPair,
};

use crate::{ApiError, AppState, extract::AuthUser};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/providers", get(providers))
        .route("/auth/oidc/start", get(oidc_start))
        .route("/auth/oidc/callback", get(oidc_callback))
        .route("/auth/oidc/exchange", post(oidc_exchange))
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/me/password", post(change_password))
        .route("/tickets", post(ticket))
}

async fn providers(State(state): State<AppState>) -> Json<Providers> {
    Json(state.auth.providers())
}

/// Sends the browser to the identity provider.
async fn oidc_start(
    State(state): State<AppState>,
    Query(query): Query<OidcStartQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let url = state
        .auth
        .oidc_start(&query.redirect_uri, &query.code_challenge)
        .await?;
    Ok(([(CACHE_CONTROL, "no-store")], Redirect::to(&url)))
}

/// Where the identity provider sends the browser back; hands the user on to the client.
async fn oidc_callback(
    State(state): State<AppState>,
    Query(query): Query<OidcCallbackQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let code = query.code.as_deref().filter(|code| !code.is_empty());
    let url = state
        .auth
        .oidc_callback(
            query.state.as_deref().filter(|state| !state.is_empty()),
            code,
        )
        .await?;
    // The URL carries the one-time code, so keep it out of caches and `Referer` headers.
    Ok((
        [
            (CACHE_CONTROL, "no-store"),
            (REFERRER_POLICY, "no-referrer"),
        ],
        Redirect::to(&url),
    ))
}

async fn oidc_exchange(
    State(state): State<AppState>,
    Json(request): Json<OidcExchangeRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    Ok(Json(
        state
            .auth
            .oidc_exchange(
                &request.code,
                &request.code_verifier,
                request.device.as_ref(),
            )
            .await?,
    ))
}

async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    Ok(Json(
        state
            .auth
            .login(
                &request.username,
                &request.password,
                request.device.as_ref(),
            )
            .await?,
    ))
}

async fn refresh(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    Ok(Json(state.auth.refresh(&request.refresh_token).await?))
}

async fn logout(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<StatusCode, ApiError> {
    state.auth.logout(&request.refresh_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(State(state): State<AppState>, user: AuthUser) -> Result<Json<Me>, ApiError> {
    let found = state
        .db
        .find_user(user.id)
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?
        .ok_or(ApiError::Unauthorized)?;
    let folders = state
        .db
        .special_folders(user.id)
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;
    Ok(Json(Me {
        id: found.id,
        username: found.username,
        lost_and_found_id: folders.lost_and_found_id,
        is_admin: found.is_admin,
        external: found.password_hash.is_none(),
    }))
}

/// Replaces the caller's password; their other sessions end, this one stays signed in.
async fn change_password(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .auth
        .change_password(
            user.id,
            user.session_id,
            &request.current_password,
            &request.new_password,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ticket(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<TicketRequest>,
) -> Result<Json<TicketResponse>, ApiError> {
    match request {
        TicketRequest::Download { node_id } | TicketRequest::DownloadVersion { node_id, .. } => {
            state.files.get(user.id, node_id).await?;
        }
        TicketRequest::TransferItem { transfer_id, .. } => {
            let (transfer, _) = state.files.transfer(user.id, transfer_id).await?;
            if transfer.recipient_id != user.id {
                return Err(ApiError::Forbidden);
            }
        }
        TicketRequest::Events => {}
    }
    Ok(Json(state.auth.issue_ticket(
        user.id,
        user.session_id,
        &request,
    )?))
}
