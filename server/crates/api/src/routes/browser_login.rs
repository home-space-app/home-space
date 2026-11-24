//! Browser sign-in for apps: the app opens `start` in the system browser, the web client asks
//! the signed-in user about it and answers with `approve` or `deny`, and the app trades the
//! code it is sent for a session at `exchange`.

use axum::{
    Json, Router,
    extract::State,
    http::header::{CACHE_CONTROL, REFERRER_POLICY},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use protocol::{
    BrowserLoginDecision, BrowserLoginExchangeRequest, BrowserLoginFlow, BrowserLoginRedirect,
    BrowserLoginStartQuery, TokenPair,
};

use crate::{
    ApiError, AppState,
    extract::{ApiJson, ApiPath, ApiQuery, AuthUser, ClientIp, Tenant},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/browser/start", get(start))
        .route("/auth/browser/flows/{flow}", get(flow))
        .route("/auth/browser/approve", post(approve))
        .route("/auth/browser/deny", post(deny))
        .route("/auth/browser/exchange", post(exchange))
}

/// Sends the browser to the web client page that asks the user to let the app in.
async fn start(
    State(state): State<AppState>,
    tenant: Tenant,
    ClientIp(address): ClientIp,
    ApiQuery(query): ApiQuery<BrowserLoginStartQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if !state.serves_web {
        return Err(ApiError::BrowserLoginUnavailable);
    }
    let page = tenant
        .auth
        .browser_start(
            &query.redirect_uri,
            &query.code_challenge,
            query.device_name.as_deref(),
            address,
        )
        .await?;
    // The page's URL names the flow, so keep it out of caches and `Referer` headers.
    Ok((
        [
            (CACHE_CONTROL, "no-store"),
            (REFERRER_POLICY, "no-referrer"),
        ],
        Redirect::to(&page),
    ))
}

async fn flow(
    tenant: Tenant,
    _user: AuthUser,
    ApiPath(flow): ApiPath<String>,
) -> Result<Json<BrowserLoginFlow>, ApiError> {
    Ok(Json(tenant.auth.browser_flow(&flow).await?))
}

/// The signed-in caller lets the app in. Their token comes in the `Authorization` header,
/// which no other site can make the browser send.
async fn approve(
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<BrowserLoginDecision>,
) -> Result<Json<BrowserLoginRedirect>, ApiError> {
    let redirect = tenant.auth.browser_approve(&request.flow, user.id).await?;
    Ok(Json(BrowserLoginRedirect { redirect }))
}

async fn deny(
    tenant: Tenant,
    _user: AuthUser,
    ApiJson(request): ApiJson<BrowserLoginDecision>,
) -> Result<Json<BrowserLoginRedirect>, ApiError> {
    let redirect = tenant.auth.browser_deny(&request.flow).await?;
    Ok(Json(BrowserLoginRedirect { redirect }))
}

/// Only apps exchange codes here, so the refresh token is always in the body.
async fn exchange(
    tenant: Tenant,
    ClientIp(address): ClientIp,
    ApiJson(request): ApiJson<BrowserLoginExchangeRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    let pair = tenant
        .auth
        .browser_exchange(
            &request.code,
            &request.code_verifier,
            request.device.as_ref(),
            address,
        )
        .await?;
    Ok(Json(pair))
}
