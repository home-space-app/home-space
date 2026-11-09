//! A signed-in user changing their own password.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::{PASSWORD, TestApp, setup};

const CHANGE: &str = "/api/v1/me/password";
const LOGIN: &str = "/api/v1/auth/login";
const ME: &str = "/api/v1/me";
const NEW_PASSWORD: &str = "a-much-better-one";

async fn change(app: &TestApp, token: &str, current: &str, new: &str) -> common::Response {
    app.json(
        Method::POST,
        CHANGE,
        Some(token),
        json!({ "current_password": current, "new_password": new }),
    )
    .await
}

async fn login(app: &TestApp, username: &str, password: &str) -> common::Response {
    app.json(
        Method::POST,
        LOGIN,
        None,
        json!({ "username": username, "password": password }),
    )
    .await
}

fn access(response: &common::Response) -> String {
    let body: Value = response.json();
    body["access_token"].as_str().expect("token").to_owned()
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_keeps_this_session_and_ends_the_others(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let other = access(&login(&app, "alice", PASSWORD).await);

    let response = change(&app, &token, PASSWORD, NEW_PASSWORD).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);

    assert_eq!(app.get(ME, &token).await.status, StatusCode::OK);
    assert_eq!(app.get(ME, &other).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        login(&app, "alice", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, "alice", NEW_PASSWORD).await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_wrong_current_password_changes_nothing(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = change(&app, &token, "not-the-password", NEW_PASSWORD).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.json()["message"], "the current password is wrong");
    assert_eq!(app.get(ME, &token).await.status, StatusCode::OK);
    assert_eq!(login(&app, "alice", PASSWORD).await.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_short_new_password_is_refused(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = change(&app, &token, PASSWORD, "short").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(login(&app, "alice", PASSWORD).await.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn guessing_the_current_password_is_throttled(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(
            change(&app, &token, "guess-guess", NEW_PASSWORD)
                .await
                .status,
        );
    }
    assert_eq!(statuses.last(), Some(&StatusCode::TOO_MANY_REQUESTS));
    // The block is shared with sign-in.
    assert_eq!(
        login(&app, "alice", PASSWORD).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_needs_a_session(pool: PgPool) {
    let app = setup(pool).await;
    let response = app
        .json(
            Method::POST,
            CHANGE,
            None,
            json!({ "current_password": PASSWORD, "new_password": NEW_PASSWORD }),
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}
