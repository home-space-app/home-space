//! Passkeys and security keys: the user's own, the second step of a sign-in with one, a
//! passkey sign-in, and confirming it is the user with one.
//!
//! Every challenge is for the page that asks, named by `Origin`, which must be one of the
//! tenant's sites (see [`WebAuthnSites`] and [`RelyingParty`]).

use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, patch, post},
};
use uuid::Uuid;

use auth::{AuthError, RelyingParty};
use protocol::{
    AssertionChallenge, LoginWebAuthnRequest, PasskeyFinish, RegistrationChallenge,
    RenameWebAuthnKey, WebAuthnAdded, WebAuthnFinish, WebAuthnKey, WebAuthnStart,
};

use super::auth::{browser, session};
use crate::{
    ApiError, AppState,
    browser::WebAuthnSites,
    extract::{ApiJson, ApiPath, AuthUser, ClientIp, Tenant},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/webauthn", get(keys))
        .route("/me/webauthn/start", post(start))
        .route("/me/webauthn/finish", post(finish))
        .route("/me/webauthn/{id}", patch(rename).delete(remove))
        .route("/me/reauthenticate/webauthn", post(reauth_options))
        .route("/auth/login/webauthn", post(login_options))
        .route("/auth/passkey/start", post(passkey_start))
        .route("/auth/passkey/finish", post(passkey_finish))
}

/// The relying party of the page the request comes from.
fn relying_party(headers: &HeaderMap, tenant: &Tenant) -> Result<RelyingParty, ApiError> {
    let origin =
        WebAuthnSites::of(headers, tenant)
            .origin()
            .ok_or(AuthError::WebAuthnUnavailable(
                "Passkeys work only on the server's own address.",
            ))?;
    Ok(RelyingParty::new(origin)?)
}

/// The caller's passkeys and security keys.
async fn keys(tenant: Tenant, user: AuthUser) -> Result<Json<Vec<WebAuthnKey>>, ApiError> {
    Ok(Json(tenant.auth.webauthn_keys(user.id).await?))
}

/// The challenge for a new passkey or security key of the caller.
async fn start(
    tenant: Tenant,
    user: AuthUser,
    headers: HeaderMap,
    ApiJson(request): ApiJson<WebAuthnStart>,
) -> Result<Json<RegistrationChallenge>, ApiError> {
    let party = relying_party(&headers, &tenant)?;
    let challenge = tenant
        .auth
        .start_webauthn(user.id, user.session_id, request.kind, &party)
        .await?;
    Ok(Json(challenge))
}

/// Keeps the caller's new passkey or security key.
async fn finish(
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<WebAuthnFinish>,
) -> Result<Json<WebAuthnAdded>, ApiError> {
    let added = tenant
        .auth
        .finish_webauthn(
            user.id,
            user.session_id,
            request.challenge_id,
            &request.credential,
            request.name.as_deref(),
        )
        .await?;
    Ok(Json(added))
}

async fn rename(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RenameWebAuthnKey>,
) -> Result<Json<WebAuthnKey>, ApiError> {
    let key = tenant
        .auth
        .rename_webauthn(user.id, user.session_id, id, &request.name)
        .await?;
    Ok(Json(key))
}

async fn remove(
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    tenant
        .auth
        .remove_webauthn(user.id, user.session_id, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The challenge for confirming it is the caller with a passkey or security key.
async fn reauth_options(
    tenant: Tenant,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Json<AssertionChallenge>, ApiError> {
    let party = relying_party(&headers, &tenant)?;
    let challenge = tenant
        .auth
        .reauth_webauthn_options(user.id, user.session_id, &party)
        .await?;
    Ok(Json(challenge))
}

/// The challenge for a passkey or security key as the second step of a sign-in.
async fn login_options(
    tenant: Tenant,
    headers: HeaderMap,
    ApiJson(request): ApiJson<LoginWebAuthnRequest>,
) -> Result<Json<AssertionChallenge>, ApiError> {
    let party = relying_party(&headers, &tenant)?;
    let challenge = tenant
        .auth
        .login_webauthn_options(&request.login_token, &party)
        .await?;
    Ok(Json(challenge))
}

/// The challenge for signing in with a passkey.
async fn passkey_start(
    tenant: Tenant,
    ClientIp(address): ClientIp,
    headers: HeaderMap,
) -> Result<Json<AssertionChallenge>, ApiError> {
    let party = relying_party(&headers, &tenant)?;
    Ok(Json(tenant.auth.passkey_start(&party, address).await?))
}

/// Signs in with a passkey's answer.
async fn passkey_finish(
    tenant: Tenant,
    ClientIp(address): ClientIp,
    headers: HeaderMap,
    ApiJson(request): ApiJson<PasskeyFinish>,
) -> Result<Response, ApiError> {
    let browser = browser(request.cookie, &headers, &tenant)?;
    let pair = tenant
        .auth
        .passkey_finish(
            request.challenge_id,
            &request.credential,
            request.device.as_ref(),
            address,
        )
        .await?;
    Ok(session(&tenant, pair, browser))
}
