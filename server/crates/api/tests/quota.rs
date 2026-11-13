//! Storage quotas: what counts against them and which writes they refuse.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::{Response, TestApp, UPLOAD_CHUNK, setup};

async fn set_quota(app: &TestApp, username: &str, bytes: Option<i64>) {
    let id = app.user_id(username).await;
    app.state.db.set_quota(id, bytes).await.expect("quota");
}

fn assert_quota_exceeded(response: &Response) {
    assert_eq!(response.status, StatusCode::INSUFFICIENT_STORAGE);
    let body = response.json();
    assert_eq!(body["error"], "quota_exceeded");
    assert!(
        body["message"].as_str().expect("message").contains("left"),
        "{body}"
    );
}

/// Names of the files at the root, leaving out folders such as Lost & Found.
async fn root_files(app: &TestApp, token: &str) -> Vec<String> {
    let mut names = app
        .get("/api/v1/nodes", token)
        .await
        .json()
        .as_array()
        .expect("root")
        .iter()
        .filter(|node| node["kind"] == "file")
        .map(|node| node["name"].as_str().expect("name").to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn id_of(response: &Response) -> String {
    response.json()["id"].as_str().expect("id").to_owned()
}

async fn create_upload(app: &TestApp, token: &str, request: Value) -> Response {
    app.json(Method::POST, "/api/v1/uploads", Some(token), request)
        .await
}

async fn patch(app: &TestApp, token: &str, id: &str, offset: usize, chunk: &[u8]) -> Response {
    app.send_with(
        Method::PATCH,
        &format!("/api/v1/uploads/{id}"),
        Some(token),
        Body::from(chunk.to_vec()),
        None,
        &[("upload-offset", &offset.to_string())],
    )
    .await
}

async fn delete(app: &TestApp, token: &str, uri: &str) -> StatusCode {
    app.send(Method::DELETE, uri, Some(token), Body::empty(), None)
        .await
        .status
}

#[sqlx::test(migrations = "../../migrations")]
async fn without_a_quota_uploads_are_not_limited(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let response = app.upload(&token, "name=a.txt", "hello world").await;
    assert_eq!(response.status, StatusCode::CREATED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_upload_that_fits_is_stored_and_one_that_does_not_is_refused(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    set_quota(&app, "alice", Some(10)).await;

    let fits = app.upload(&token, "name=a.txt", "123456").await;
    assert_eq!(fits.status, StatusCode::CREATED);
    let exact = app.upload(&token, "name=b.txt", "7890").await;
    assert_eq!(exact.status, StatusCode::CREATED);

    let over = app.upload(&token, "name=c.txt", "x").await;
    assert_quota_exceeded(&over);
    assert_eq!(root_files(&app, &token).await, ["a.txt", "b.txt"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_users_are_not_held_back_by_a_quota(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    set_quota(&app, "alice", Some(0)).await;

    assert_quota_exceeded(&app.upload(&alice, "name=a.txt", "a").await);
    assert_eq!(
        app.upload(&bob, "name=a.txt", "a").await.status,
        StatusCode::CREATED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_new_version_counts_and_the_old_one_stays_counted(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    set_quota(&app, "alice", Some(8)).await;
    let file = id_of(&app.upload(&token, "name=a.txt", "four").await);

    // The previous 4 bytes become a version, so 4 + 4 fills the quota exactly.
    let replaced = app
        .send(
            Method::PUT,
            &format!("/api/v1/files/{file}/content"),
            Some(&token),
            Body::from("FOUR"),
            None,
        )
        .await;
    assert!(replaced.status.is_success(), "{}", replaced.status);

    let again = app
        .send(
            Method::PUT,
            &format!("/api/v1/files/{file}/content"),
            Some(&token),
            Body::from("x"),
            None,
        )
        .await;
    assert_quota_exceeded(&again);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_recycle_bin_counts_until_it_is_emptied(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    set_quota(&app, "alice", Some(5)).await;
    let file = id_of(&app.upload(&token, "name=a.txt", "12345").await);
    assert!(
        delete(&app, &token, &format!("/api/v1/nodes/{file}"))
            .await
            .is_success()
    );

    assert_quota_exceeded(&app.upload(&token, "name=b.txt", "1").await);

    assert!(delete(&app, &token, "/api/v1/trash").await.is_success());
    let after = app.upload(&token, "name=b.txt", "12345").await;
    assert_eq!(after.status, StatusCode::CREATED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_chunked_upload_too_big_for_the_quota_is_refused_up_front(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    set_quota(&app, "alice", Some(10)).await;
    app.upload(&token, "name=a.txt", "123456").await;

    let created = create_upload(&app, &token, json!({ "name": "b.txt", "size": 5 })).await;

    assert_quota_exceeded(&created);
    assert_eq!(app.get("/api/v1/uploads", &token).await.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn unfinished_uploads_reserve_their_size(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    set_quota(&app, "alice", Some(10)).await;
    let first = create_upload(&app, &token, json!({ "name": "a.txt", "size": 8 })).await;
    assert_eq!(first.status, StatusCode::CREATED);

    let second = create_upload(&app, &token, json!({ "name": "b.txt", "size": 3 })).await;
    assert_quota_exceeded(&second);
    assert_quota_exceeded(&app.upload(&token, "name=c.txt", "123").await);

    // Cancelling gives the reserved bytes back.
    let first = id_of(&first);
    assert!(
        delete(&app, &token, &format!("/api/v1/uploads/{first}"))
            .await
            .is_success()
    );
    let retried = create_upload(&app, &token, json!({ "name": "b.txt", "size": 3 })).await;
    assert_eq!(retried.status, StatusCode::CREATED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_chunked_upload_completes_when_it_still_fits(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    set_quota(&app, "alice", Some(10)).await;
    let content = b"0123456789";
    let id = id_of(&create_upload(&app, &token, json!({ "name": "a.txt", "size": 10 })).await);

    let mut last = None;
    for (index, chunk) in content.chunks(UPLOAD_CHUNK as usize).enumerate() {
        last = Some(patch(&app, &token, &id, index * UPLOAD_CHUNK as usize, chunk).await);
    }

    assert_eq!(last.expect("chunks").status, StatusCode::CREATED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_quota_lowered_during_a_chunked_upload_refuses_its_last_chunk(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    set_quota(&app, "alice", Some(10)).await;
    let id = id_of(&create_upload(&app, &token, json!({ "name": "a.txt", "size": 6 })).await);
    let partial = patch(&app, &token, &id, 0, b"0123").await;
    assert!(partial.status.is_success(), "{}", partial.status);

    set_quota(&app, "alice", Some(5)).await;
    let last = patch(&app, &token, &id, 4, b"45").await;

    assert_quota_exceeded(&last);
    assert!(root_files(&app, &token).await.is_empty());
    // The upload is over and no longer reserves anything.
    assert_eq!(
        app.upload(&token, "name=b.txt", "12345").await.status,
        StatusCode::CREATED
    );
}
