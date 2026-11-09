//! Storage usage per user, and the administration status that lists it for everyone.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::Value;
use sqlx::PgPool;

use common::{TestApp, setup};

const STORAGE: &str = "/api/v1/storage";
const STATUS: &str = "/api/v1/admin/status";
const ME: &str = "/api/v1/me";

/// Leaves `token`'s user with 4 bytes of files, a 3-byte previous version and 5 bytes binned.
async fn store_some_files(app: &TestApp, token: &str) {
    app.upload(token, "name=a.txt", "one").await;
    app.upload(token, "name=a.txt", "two!").await;
    let binned = app.upload(token, "name=b.txt", "12345").await;
    let id = binned.json()["id"].as_str().expect("id").to_owned();
    let deleted = app
        .send(
            Method::DELETE,
            &format!("/api/v1/nodes/{id}"),
            Some(token),
            Body::empty(),
            None,
        )
        .await;
    assert!(deleted.status.is_success(), "{}", deleted.status);
}

async fn make_admin(app: &TestApp, username: &str) {
    let user = app
        .state
        .db
        .find_user_by_username(username)
        .await
        .expect("lookup")
        .expect("user");
    app.state.db.set_admin(user.id, true).await.expect("admin");
}

fn usage(value: &Value) -> (u64, u64, u64) {
    let part = |name: &str| value[name].as_u64().expect(name);
    (part("files"), part("versions"), part("trash"))
}

#[sqlx::test(migrations = "../../migrations")]
async fn storage_splits_files_versions_and_recycle_bin(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    store_some_files(&app, &token).await;

    let response = app.get(STORAGE, &token).await;

    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    assert_eq!(usage(&body["usage"]), (4, 3, 5));
    let disk = &body["disk"];
    assert!(disk["total"].as_u64().expect("total") >= disk["available"].as_u64().expect("free"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn storage_counts_only_the_callers_files(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    store_some_files(&app, &alice).await;

    let response = app.get(STORAGE, &bob).await;

    assert_eq!(usage(&response.json()["usage"]), (0, 0, 0));
}

#[sqlx::test(migrations = "../../migrations")]
async fn me_reports_the_administrator_role(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    assert_eq!(app.get(ME, &token).await.json()["is_admin"], false);

    make_admin(&app, "alice").await;

    let me = app.get(ME, &token).await.json();
    assert_eq!(me["is_admin"], true);
    assert_eq!(me["external"], false);
}

#[sqlx::test(migrations = "../../migrations")]
async fn status_lists_every_users_storage(pool: PgPool) {
    let app = setup(pool).await;
    let admin = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    make_admin(&app, "alice").await;
    store_some_files(&app, &bob).await;

    let response = app.get(STATUS, &admin).await;

    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    let users = body["users"].as_array().expect("users");
    let names: Vec<_> = users.iter().map(|user| user["username"].clone()).collect();
    assert_eq!(names, ["alice", "bob"]);
    assert_eq!(users[0]["is_admin"], true);
    assert_eq!(usage(&users[0]["usage"]), (0, 0, 0));
    assert_eq!(usage(&users[1]["usage"]), (4, 3, 5));
    assert!(body["disk"]["space"]["total"].as_u64().is_some());
    assert!(
        body["database"]["version"]
            .as_str()
            .expect("version")
            .starts_with("PostgreSQL ")
    );
    assert!(body["database"]["migrations"].as_i64().expect("migrations") > 0);
    assert_eq!(body["config"]["password_login"], true);
}

#[sqlx::test(migrations = "../../migrations")]
async fn status_is_refused_to_regular_users(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("bob").await;

    let response = app.get(STATUS, &token).await;

    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(response.json()["error"], "forbidden");
}

#[sqlx::test(migrations = "../../migrations")]
async fn status_needs_a_session(pool: PgPool) {
    let app = setup(pool).await;

    let response = app
        .send(Method::GET, STATUS, None, Body::empty(), None)
        .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}
