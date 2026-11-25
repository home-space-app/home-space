//! What a session remembers of its sign-in, and confirming it is the user again.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use super::*;
use crate::ReauthProof;

const VERIFIER: &str = "a-verifier-long-enough-for-pkce-0123456789abcdef";

async fn signed_in(auth: &AuthService, device: Option<&DeviceInfo>) -> Grant {
    auth.create_user("alice", "password-1").await.expect("user");
    let pair = auth
        .login("alice", "password-1", device)
        .await
        .expect("login");
    grant_of(auth, &pair).await
}

async fn authentication(auth: &AuthService, grant: &Grant) -> db::Authentication {
    auth.sessions
        .authentication(grant.session_id, grant.user_id)
        .await
        .expect("authentication")
}

/// Moves the last proof of `grant`'s user back by `minutes`.
async fn age(pool: &PgPool, grant: &Grant, minutes: i32) {
    sqlx::query(
        "UPDATE sessions SET authenticated_at = now() - make_interval(mins => $2) WHERE id = $1",
    )
    .bind(grant.session_id)
    .bind(minutes)
    .execute(pool)
    .await
    .expect("age");
}

fn methods(result: Result<(), AuthError>) -> Vec<&'static str> {
    match result {
        Err(AuthError::ReauthenticationRequired { methods }) => methods,
        other => panic!("expected reauthentication-required, got {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_sign_in_is_remembered_as_one_just_now(pool: PgPool) {
    let auth = service(pool);
    let grant = signed_in(&auth, None).await;
    let authentication = authentication(&auth, &grant).await;
    assert_eq!(authentication.auth_methods, ["pwd"]);
    let since = OffsetDateTime::now_utc() - authentication.authenticated_at;
    assert!(since < Duration::minutes(1), "{since}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_sensitive_change_needs_a_recent_proof_which_the_password_gives(pool: PgPool) {
    let auth = service(pool.clone());
    let grant = signed_in(&auth, None).await;
    let (user, session) = (grant.user_id, grant.session_id);
    auth.require_recent_sign_in(user, session)
        .await
        .expect("just signed in");

    age(&pool, &grant, 11).await;
    let refused = auth.require_recent_sign_in(user, session).await;
    assert_eq!(methods(refused), ["password"]);

    let wrong = auth
        .reauthenticate(user, session, ReauthProof::Password("wrong-password"))
        .await;
    assert!(matches!(wrong, Err(AuthError::WrongPassword)), "{wrong:?}");
    assert!(auth.require_recent_sign_in(user, session).await.is_err());

    auth.reauthenticate(user, session, ReauthProof::Password("password-1"))
        .await
        .expect("confirmed");
    auth.require_recent_sign_in(user, session)
        .await
        .expect("recent again");
    assert_eq!(authentication(&auth, &grant).await.auth_methods, ["pwd"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn guessing_the_password_to_confirm_is_throttled_like_changing_it(pool: PgPool) {
    let auth = service(pool);
    let grant = signed_in(&auth, None).await;
    for _ in 0..3 {
        let wrong = auth
            .reauthenticate(
                grant.user_id,
                grant.session_id,
                ReauthProof::Password("wrong-password"),
            )
            .await;
        assert!(matches!(wrong, Err(AuthError::WrongPassword)));
    }
    let blocked = auth
        .change_password(grant.user_id, grant.session_id, "password-1", "password-2")
        .await;
    assert!(matches!(blocked, Err(AuthError::TooManyAttempts { .. })));
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_the_password_counts_as_confirming(pool: PgPool) {
    let auth = service(pool.clone());
    let grant = signed_in(&auth, None).await;
    age(&pool, &grant, 30).await;
    auth.change_password(grant.user_id, grant.session_id, "password-1", "password-2")
        .await
        .expect("changed");
    auth.require_recent_sign_in(grant.user_id, grant.session_id)
        .await
        .expect("recent");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_user_without_a_password_is_asked_to_sign_in_again(pool: PgPool) {
    let auth = service(pool.clone());
    let grant = signed_in(&auth, None).await;
    sqlx::query("UPDATE users SET password_hash = NULL WHERE id = $1")
        .bind(grant.user_id)
        .execute(&pool)
        .await
        .expect("no password");
    age(&pool, &grant, 11).await;
    let refused = auth
        .require_recent_sign_in(grant.user_id, grant.session_id)
        .await;
    assert!(methods(refused).is_empty());
    let confirm = auth
        .reauthenticate(
            grant.user_id,
            grant.session_id,
            ReauthProof::Password("password-1"),
        )
        .await;
    assert!(matches!(confirm, Err(AuthError::NoPassword)), "{confirm:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_extension_session_signed_in_as_its_app_did(pool: PgPool) {
    let auth = service(pool.clone());
    let app = signed_in(&auth, Some(&laptop())).await;
    age(&pool, &app, 20).await;
    let pair = auth
        .start_device_session(app.user_id, app.session_id, "files")
        .await
        .expect("extension");
    let extension = grant_of(&auth, &pair).await;
    assert_ne!(extension.session_id, app.session_id);
    assert_eq!(
        authentication(&auth, &extension).await,
        authentication(&auth, &app).await
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_app_let_in_from_the_browser_signed_in_as_the_browser_did(pool: PgPool) {
    let auth = service(pool.clone());
    let web = signed_in(&auth, None).await;
    age(&pool, &web, 20).await;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let page = auth
        .browser_start("http://127.0.0.1:4711/callback", &challenge, None, None)
        .await
        .expect("start");
    let flow = page.rsplit_once("flow=").expect("flow").1;
    let redirect = auth
        .browser_approve(flow, web.user_id, web.session_id)
        .await
        .expect("approve");
    let code = redirect.rsplit_once("code=").expect("code").1;
    let pair = auth
        .browser_exchange(code, VERIFIER, None, None)
        .await
        .expect("exchange");
    let app = grant_of(&auth, &pair).await;
    assert_eq!(
        authentication(&auth, &app).await,
        authentication(&auth, &web).await,
        "a click on Continue is no new proof"
    );
    assert!(
        auth.require_recent_sign_in(app.user_id, app.session_id)
            .await
            .is_err()
    );
}
