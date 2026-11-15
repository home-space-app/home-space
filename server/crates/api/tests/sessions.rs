//! Access tokens and tickets are only good while their session exists, so a leaked signing
//! secret does not let anyone forge access.

mod common;

use axum::{body::Body, http::Method, http::StatusCode};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use common::{PASSWORD, TestApp, setup};

const SECRET: &[u8] = b"integration-test-secret-integration";

/// Signs claims with the server's real secret, as an attacker who stole it could.
fn forge(claims: Value) -> String {
    let header = Header {
        kid: Some(URL_SAFE_NO_PAD.encode(&Sha256::digest(SECRET)[..6])),
        ..Header::default()
    };
    encode(
        &header,
        &claims,
        &EncodingKey::from_secret(SECRET).expect("key"),
    )
    .expect("forge")
}

fn access_claims(user: &str, session: Option<Uuid>) -> Value {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut claims = json!({ "sub": user, "iat": now, "exp": now + 600 });
    if let Some(session) = session {
        claims["sid"] = json!(session);
    }
    claims
}

fn ticket_claims(user: &str, session: Uuid, scope: &str) -> Value {
    let mut claims = access_claims(user, Some(session));
    claims["tkt"] = json!(scope);
    claims
}

struct Session {
    access: String,
    refresh: String,
    user_id: String,
}

async fn sign_in(app: &TestApp, username: &str) -> Session {
    app.state.auth.create_user(username, PASSWORD).await.ok();
    let tokens = app
        .json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({ "username": username, "password": PASSWORD }),
        )
        .await
        .json();
    let access = tokens["access_token"].as_str().expect("access").to_owned();
    let user_id = app.get("/api/v1/me", &access).await.json()["id"]
        .as_str()
        .expect("id")
        .to_owned();
    Session {
        access,
        refresh: tokens["refresh_token"]
            .as_str()
            .expect("refresh")
            .to_owned(),
        user_id,
    }
}

async fn status_of(app: &TestApp, token: &str) -> StatusCode {
    app.get("/api/v1/me", token).await.status
}

#[sqlx::test(migrations = "../../migrations")]
async fn token_signed_with_the_real_secret_but_no_session_is_rejected(pool: PgPool) {
    let app = setup(pool).await;
    let alice = sign_in(&app, "alice").await;

    let forged = forge(access_claims(&alice.user_id, Some(Uuid::now_v7())));

    assert_eq!(status_of(&app, &forged).await, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn token_without_a_session_claim_is_rejected(pool: PgPool) {
    let app = setup(pool).await;
    let alice = sign_in(&app, "alice").await;

    let forged = forge(access_claims(&alice.user_id, None));

    assert_eq!(status_of(&app, &forged).await, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_of_another_user_cannot_be_borrowed(pool: PgPool) {
    let app = setup(pool).await;
    let alice = sign_in(&app, "alice").await;
    let bob = sign_in(&app, "bob").await;
    // Alice's real session id, taken from her own token.
    let session = decode_session(&alice.access);

    let forged = forge(access_claims(&bob.user_id, Some(session)));

    assert_eq!(status_of(&app, &forged).await, StatusCode::UNAUTHORIZED);
    assert_eq!(status_of(&app, &alice.access).await, StatusCode::OK);
}

fn decode_session(token: &str) -> Uuid {
    let payload = token.split('.').nth(1).expect("payload");
    let bytes = URL_SAFE_NO_PAD.decode(payload).expect("base64");
    let claims: Value = serde_json::from_slice(&bytes).expect("json");
    claims["sid"].as_str().expect("sid").parse().expect("uuid")
}

#[sqlx::test(migrations = "../../migrations")]
async fn logout_ends_the_access_token_at_once(pool: PgPool) {
    let app = setup(pool).await;
    let alice = sign_in(&app, "alice").await;
    assert_eq!(status_of(&app, &alice.access).await, StatusCode::OK);

    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/logout",
            None,
            json!({ "refresh_token": alice.refresh }),
        )
        .await;
    assert!(response.status.is_success());

    assert_eq!(
        status_of(&app, &alice.access).await,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn refreshing_keeps_the_session(pool: PgPool) {
    let app = setup(pool).await;
    let alice = sign_in(&app, "alice").await;

    let refreshed = app
        .json(
            Method::POST,
            "/api/v1/auth/refresh",
            None,
            json!({ "refresh_token": alice.refresh }),
        )
        .await
        .json();
    let access = refreshed["access_token"].as_str().expect("access");

    assert_eq!(decode_session(access), decode_session(&alice.access));
    assert_eq!(status_of(&app, access).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn reusing_a_refresh_token_ends_the_session(pool: PgPool) {
    let app = setup(pool).await;
    let alice = sign_in(&app, "alice").await;
    let refresh = |token: String| {
        let app = &app;
        async move {
            app.json(
                Method::POST,
                "/api/v1/auth/refresh",
                None,
                json!({ "refresh_token": token }),
            )
            .await
        }
    };
    let second = refresh(alice.refresh.clone()).await.json();
    assert_eq!(
        refresh(alice.refresh.clone()).await.status,
        StatusCode::UNAUTHORIZED
    );

    let access = second["access_token"].as_str().expect("access");
    assert_eq!(status_of(&app, access).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        status_of(&app, &alice.access).await,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn sessions_are_independent_and_a_password_change_ends_them_all(pool: PgPool) {
    let app = setup(pool).await;
    let laptop = sign_in(&app, "alice").await;
    let phone = sign_in(&app, "alice").await;
    app.state
        .auth
        .create_user("bob", PASSWORD)
        .await
        .expect("bob");
    let bob = sign_in(&app, "bob").await;
    assert_ne!(
        decode_session(&laptop.access),
        decode_session(&phone.access)
    );

    app.json(
        Method::POST,
        "/api/v1/auth/logout",
        None,
        json!({ "refresh_token": laptop.refresh }),
    )
    .await;
    assert_eq!(
        status_of(&app, &laptop.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(status_of(&app, &phone.access).await, StatusCode::OK);

    let user_id = phone.user_id.parse().expect("user id");
    app.state
        .auth
        .set_password(user_id, "another-password")
        .await
        .expect("set password");
    assert_eq!(
        status_of(&app, &phone.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(status_of(&app, &bob.access).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn expired_session_is_rejected(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let alice = sign_in(&app, "alice").await;

    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .expect("expire");

    assert_eq!(
        status_of(&app, &alice.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(app.state.db.prune_sessions().await.expect("prune"), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn forged_ticket_for_an_unknown_session_is_rejected(pool: PgPool) {
    let app = setup(pool).await;
    let alice = sign_in(&app, "alice").await;
    let id = app
        .upload(&alice.access, "name=a.txt", "hello")
        .await
        .json()["id"]
        .as_str()
        .expect("id")
        .to_owned();

    let forged = forge(ticket_claims(
        &alice.user_id,
        Uuid::now_v7(),
        &format!("download:{id}"),
    ));
    let response = app
        .send(
            Method::GET,
            &format!("/api/v1/files/{id}/content?ticket={forged}"),
            None,
            Body::empty(),
            None,
        )
        .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

async fn download(app: &TestApp, id: &str, ticket: &str) -> StatusCode {
    app.send(
        Method::GET,
        &format!("/api/v1/files/{id}/content?ticket={ticket}"),
        None,
        Body::empty(),
        None,
    )
    .await
    .status
}

#[sqlx::test(migrations = "../../migrations")]
async fn ticket_stops_working_when_its_session_ends(pool: PgPool) {
    let app = setup(pool).await;
    let alice = sign_in(&app, "alice").await;
    let id = app
        .upload(&alice.access, "name=a.txt", "hello")
        .await
        .json()["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let ticket = app
        .json(
            Method::POST,
            "/api/v1/tickets",
            Some(&alice.access),
            json!({ "scope": "download", "node_id": id }),
        )
        .await
        .json()["ticket"]
        .as_str()
        .expect("ticket")
        .to_owned();
    assert_eq!(download(&app, &id, &ticket).await, StatusCode::OK);

    app.json(
        Method::POST,
        "/api/v1/auth/logout",
        None,
        json!({ "refresh_token": alice.refresh }),
    )
    .await;

    assert_eq!(download(&app, &id, &ticket).await, StatusCode::UNAUTHORIZED);
}
