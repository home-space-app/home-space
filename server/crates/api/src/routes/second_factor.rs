//! Two-factor sign-in: the second step of a sign-in, and the user's own authenticator app and
//! recovery codes.

use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};

use auth::SecondFactorProof;
use protocol::{LoginVerifyRequest, RecoveryCodes, TotpConfirmRequest, TotpEnrolment};

use super::auth::{browser, session};
use crate::extract::ApiJson;
use crate::{
    ApiError, AppState,
    extract::{AuthUser, ClientIp, Tenant},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login/verify", post(login_verify))
        .route("/me/totp", post(start_totp).delete(remove_totp))
        .route("/me/totp/confirm", post(confirm_totp))
        .route("/me/recovery-codes", post(new_recovery_codes))
}

/// Finishes a sign-in that answered `second-factor-required` with a code, or a passkey's or
/// security key's answer.
async fn login_verify(
    tenant: Tenant,
    ClientIp(address): ClientIp,
    headers: HeaderMap,
    ApiJson(request): ApiJson<LoginVerifyRequest>,
) -> Result<Response, ApiError> {
    let browser = browser(request.cookie, &headers, &tenant)?;
    let proof = match (&request.code, &request.recovery_code, &request.webauthn) {
        (Some(code), _, _) => SecondFactorProof::Totp(code),
        (None, Some(code), _) => SecondFactorProof::Recovery(code),
        (None, None, Some(answer)) => SecondFactorProof::WebAuthn(answer),
        // Validation lets through only requests with one of them.
        (None, None, None) => return Err(ApiError::BadRequest("a code is missing".to_owned())),
    };
    let pair = tenant
        .auth
        .login_verify(
            &request.login_token,
            proof,
            request.device.as_ref(),
            address,
        )
        .await?;
    Ok(session(&tenant, pair, browser))
}

/// A new authenticator app secret for the caller, kept once its first code is confirmed.
async fn start_totp(tenant: Tenant, user: AuthUser) -> Result<Json<TotpEnrolment>, ApiError> {
    let me = tenant.accounts.me(user.id).await?;
    let account = match site_host(&tenant) {
        Some(host) => format!("{}@{host}", me.username),
        None => me.username,
    };
    let enrolment = tenant
        .auth
        .start_totp(user.id, user.session_id, &account)
        .await?;
    Ok(Json(enrolment))
}

/// Confirms the caller's new authenticator app, answering their recovery codes.
async fn confirm_totp(
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<TotpConfirmRequest>,
) -> Result<Json<RecoveryCodes>, ApiError> {
    let codes = tenant
        .auth
        .confirm_totp(user.id, user.session_id, &request.code)
        .await?;
    Ok(Json(codes))
}

/// Removes the caller's authenticator app and recovery codes.
async fn remove_totp(tenant: Tenant, user: AuthUser) -> Result<StatusCode, ApiError> {
    tenant.auth.remove_totp(user.id, user.session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A new set of recovery codes for the caller, voiding the old ones.
async fn new_recovery_codes(
    tenant: Tenant,
    user: AuthUser,
) -> Result<Json<RecoveryCodes>, ApiError> {
    let codes = tenant
        .auth
        .new_recovery_codes(user.id, user.session_id)
        .await?;
    Ok(Json(codes))
}

/// The host the tenant is reached at, to tell its accounts apart in an authenticator app.
fn site_host(tenant: &Tenant) -> Option<String> {
    let authority = match &tenant.public_url {
        Some(url) => url.split_once("://").map_or(url.as_str(), |(_, rest)| rest),
        None => tenant.domains.first()?,
    };
    let authority = authority.split('/').next().unwrap_or_default();
    let host = protocol::encoding::authority_host(authority);
    (!host.is_empty()).then(|| host.to_owned())
}
