use axum::{
    Json, Router,
    extract::{Query, State},
    http::{
        HeaderMap, StatusCode,
        header::{CACHE_CONTROL, REFERRER_POLICY, SET_COOKIE},
    },
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use protocol::{
    ChangePasswordRequest, LoginRequest, Me, OidcCallbackQuery, OidcExchangeRequest,
    OidcStartQuery, Providers, RefreshRequest, TicketRequest, TicketResponse, TokenPair,
};

use crate::{
    ApiError, AppState,
    browser::{Browser, refresh_cookie},
    extract::{AuthUser, ClientIp},
};

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
    headers: HeaderMap,
    Json(request): Json<OidcExchangeRequest>,
) -> Result<Response, ApiError> {
    let browser = browser(request.cookie, &headers, &state)?;
    let pair = state
        .auth
        .oidc_exchange(
            &request.code,
            &request.code_verifier,
            request.device.as_ref(),
        )
        .await?;
    Ok(session(&state, pair, browser))
}

async fn login(
    State(state): State<AppState>,
    ClientIp(address): ClientIp,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let browser = browser(request.cookie, &headers, &state)?;
    let pair = state
        .auth
        .login_from(
            &request.username,
            &request.password,
            request.device.as_ref(),
            address,
        )
        .await?;
    Ok(session(&state, pair, browser))
}

/// An app sends its refresh token in the body and gets the next one back the same way. A
/// browser's token is in its cookie, where the next one goes too.
async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RefreshRequest>,
) -> Result<Response, ApiError> {
    let from_cookie = request.refresh_token.is_none();
    let browser = browser(request.cookie || from_cookie, &headers, &state)?;
    let token = request
        .refresh_token
        .as_deref()
        .or_else(|| refresh_cookie(&headers))
        .ok_or(ApiError::Unauthorized)?;
    let refreshed = match browser {
        Some(_) => state.auth.refresh_shared(token).await,
        None => state.auth.refresh(token).await,
    };
    match (refreshed, browser) {
        (Ok(pair), browser) => Ok(session(&state, pair, browser)),
        // The token is of no more use, so the browser need not keep it.
        (Err(err @ auth::AuthError::InvalidCredentials), Some(browser)) => {
            Ok(([(SET_COOKIE, browser.forget())], ApiError::from(err)).into_response())
        }
        (Err(err), _) => Err(err.into()),
    }
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RefreshRequest>,
) -> Result<Response, ApiError> {
    let from_cookie = request.refresh_token.is_none();
    let browser = browser(request.cookie || from_cookie, &headers, &state)?;
    let token = request
        .refresh_token
        .as_deref()
        .or_else(|| refresh_cookie(&headers));
    if let Some(token) = token {
        state.auth.logout(token).await?;
    }
    Ok(match browser {
        Some(browser) => (StatusCode::NO_CONTENT, [(SET_COOKIE, browser.forget())]).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

/// The browser behind a request that asks for the cookie or relies on it; `None` for an app.
fn browser(
    uses_cookie: bool,
    headers: &HeaderMap,
    state: &AppState,
) -> Result<Option<Browser>, ApiError> {
    uses_cookie.then(|| Browser::of(headers, state)).transpose()
}

/// The response that starts or continues a session. For a browser the refresh token goes
/// into the cookie and is left out of the body, so that no script ever holds it.
fn session(state: &AppState, mut pair: TokenPair, browser: Option<Browser>) -> Response {
    let Some(browser) = browser else {
        return Json(pair).into_response();
    };
    let token = std::mem::take(&mut pair.refresh_token);
    if token.is_empty() {
        // Another tab was handed the next token, and the cookie already holds it.
        Json(pair).into_response()
    } else {
        let cookie = browser.keep(&token, state.auth.refresh_ttl());
        ([(SET_COOKIE, cookie)], Json(pair)).into_response()
    }
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
        TicketRequest::TransferItem { transfer_id, .. }
        | TicketRequest::TransferArchive { transfer_id } => {
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
