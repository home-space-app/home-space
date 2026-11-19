//! Every error is an RFC 9457 problem, including the requests the framework refuses before a
//! handler runs (docs/api-errors.md).

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::json;
use sqlx::PgPool;

use common::{Response, setup};

fn assert_problem(response: &Response, status: StatusCode, slug: &str) {
    assert_eq!(response.status, status, "{:?}", response.json());
    assert_eq!(
        response
            .headers
            .get("content-type")
            .map(|value| value.as_bytes()),
        Some(&b"application/problem+json"[..])
    );
    let body = response.json();
    assert_eq!(body["type"], format!("/api/v1/problems/{slug}"));
    assert_eq!(body["status"], status.as_u16());
    assert!(body["title"].is_string());
    assert!(body["detail"].is_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_path_is_a_not_found_problem_naming_the_path(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = app.get("/api/v1/no-such-thing", &token).await;

    assert_problem(&response, StatusCode::NOT_FOUND, "not-found");
    assert_eq!(response.json()["instance"], "/api/v1/no-such-thing");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_wrong_method_is_a_problem(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = app
        .send(Method::PUT, "/api/v1/me", Some(&token), Body::empty(), None)
        .await;

    assert_problem(
        &response,
        StatusCode::METHOD_NOT_ALLOWED,
        "method-not-allowed",
    );
    assert!(
        response.headers.contains_key("allow"),
        "405 must say what is allowed"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_json_cannot_be_read(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = app
        .send(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            Body::from("{ not json"),
            Some("application/json"),
        )
        .await;

    assert_problem(&response, StatusCode::BAD_REQUEST, "bad-request");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_missing_field_cannot_be_read(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = app
        .json(Method::POST, "/api/v1/folders", Some(&token), json!({}))
        .await;

    assert_problem(&response, StatusCode::BAD_REQUEST, "bad-request");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_body_without_its_content_type_is_unsupported(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = app
        .send(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            Body::from(r#"{"name":"a"}"#),
            Some("text/plain"),
        )
        .await;

    assert_problem(
        &response,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported-media-type",
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_id_that_is_not_a_uuid_cannot_be_read(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = app.get("/api/v1/nodes/not-a-uuid", &token).await;

    assert_problem(&response, StatusCode::BAD_REQUEST, "bad-request");
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_invalid_field_is_listed_at_once(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;

    let response = app
        .json(
            Method::POST,
            "/api/v1/shares",
            Some(&alice),
            json!({
                "node_ids": [],
                "expires_at": "2000-01-01T00:00:00Z",
                "recipients": [uuid::Uuid::nil()],
                "password": "secret",
            }),
        )
        .await;

    assert_problem(
        &response,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation-failed",
    );
    let pointers: Vec<String> = response.json()["errors"]
        .as_array()
        .expect("errors")
        .iter()
        .map(|error| error["pointer"].as_str().expect("pointer").to_owned())
        .collect();
    assert_eq!(pointers, ["/node_ids", "/expires_at", "/password"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_taken_name_is_its_own_problem_type(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let folder = json!({ "name": "Photos" });
    app.json(
        Method::POST,
        "/api/v1/folders",
        Some(&token),
        folder.clone(),
    )
    .await;

    let response = app
        .json(Method::POST, "/api/v1/folders", Some(&token), folder)
        .await;

    assert_problem(&response, StatusCode::CONFLICT, "name-taken");
    assert_eq!(response.json()["code"], "conflict");
}
