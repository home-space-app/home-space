mod common;

use axum::{body::Body, http::Method, http::StatusCode};
use futures_util::StreamExt;
use serde_json::json;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use common::{SHORT_WAIT, TestApp, setup};

async fn ticket(app: &TestApp, token: &str, scope: serde_json::Value) -> String {
    let response = app
        .json(Method::POST, "/api/v1/tickets", Some(token), scope)
        .await;
    assert_eq!(response.status, StatusCode::OK);
    response.json()["ticket"]
        .as_str()
        .expect("ticket")
        .to_owned()
}

async fn uploaded_file_id(app: &TestApp, token: &str) -> String {
    let response = app.upload(token, "name=a.txt", "hello").await;
    response.json()["id"].as_str().expect("id").to_owned()
}

#[sqlx::test(migrations = "../../migrations")]
async fn download_ticket_downloads_without_a_bearer_header(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let id = uploaded_file_id(&app, &alice).await;
    let ticket = ticket(&app, &alice, json!({ "scope": "download", "node_id": id })).await;

    let response = app
        .send(
            Method::GET,
            &format!("/api/v1/files/{id}/content?ticket={ticket}"),
            None,
            Body::empty(),
            None,
        )
        .await;

    assert_eq!(
        (response.status, response.body.as_slice()),
        (StatusCode::OK, &b"hello"[..])
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn download_ticket_works_once(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let id = uploaded_file_id(&app, &alice).await;
    let ticket = ticket(&app, &alice, json!({ "scope": "download", "node_id": id })).await;
    let uri = format!("/api/v1/files/{id}/content?ticket={ticket}");
    let download = || app.send(Method::GET, &uri, None, Body::empty(), None);

    assert_eq!(download().await.status, StatusCode::OK);
    assert_eq!(download().await.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn events_ticket_opens_one_websocket_only(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let ticket = ticket(&app, &alice, json!({ "scope": "events" })).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app.router.clone()).into_future());
    let url = format!("ws://{addr}/api/v1/ws?ticket={ticket}");

    let _open = connect_async(&url).await.expect("first connection");
    assert!(connect_async(&url).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn download_ticket_is_limited_to_its_node(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let first = uploaded_file_id(&app, &alice).await;
    let second = app.upload(&alice, "name=b.txt", "other").await.json()["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let ticket = ticket(
        &app,
        &alice,
        json!({ "scope": "download", "node_id": first }),
    )
    .await;

    let response = app
        .send(
            Method::GET,
            &format!("/api/v1/files/{second}/content?ticket={ticket}"),
            None,
            Body::empty(),
            None,
        )
        .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn cannot_get_a_download_ticket_for_another_users_file(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let id = uploaded_file_id(&app, &alice).await;

    let response = app
        .json(
            Method::POST,
            "/api/v1/tickets",
            Some(&bob),
            json!({ "scope": "download", "node_id": id }),
        )
        .await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn tickets_require_a_bearer_token(pool: PgPool) {
    let app = setup(pool).await;

    let response = app
        .json(
            Method::POST,
            "/api/v1/tickets",
            None,
            json!({ "scope": "events" }),
        )
        .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn ticket_is_not_accepted_as_a_bearer_token(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let ticket = ticket(&app, &alice, json!({ "scope": "events" })).await;

    let response = app.get("/api/v1/me", &ticket).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn access_token_is_not_accepted_as_a_ticket(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let id = uploaded_file_id(&app, &alice).await;

    let response = app
        .send(
            Method::GET,
            &format!("/api/v1/files/{id}/content?ticket={alice}"),
            None,
            Body::empty(),
            None,
        )
        .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn events_ticket_opens_a_websocket_that_receives_events(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let ticket = ticket(&app, &alice, json!({ "scope": "events" })).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app.router.clone()).into_future());

    let (mut ws, _) = connect_async(format!("ws://{addr}/api/v1/ws?ticket={ticket}"))
        .await
        .expect("connect");
    app.upload(&alice, "name=a.txt", "hello").await;

    let message = tokio::time::timeout(SHORT_WAIT * 10, ws.next())
        .await
        .expect("event")
        .expect("stream open")
        .expect("message");
    let Message::Text(text) = message else {
        panic!("expected text message, got {message:?}");
    };
    let event: serde_json::Value = serde_json::from_str(text.as_str()).expect("json");
    assert_eq!(event["type"], "node_created");
}

#[sqlx::test(migrations = "../../migrations")]
async fn download_ticket_does_not_open_a_websocket(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let id = uploaded_file_id(&app, &alice).await;
    let ticket = ticket(&app, &alice, json!({ "scope": "download", "node_id": id })).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app.router.clone()).into_future());

    let result = connect_async(format!("ws://{addr}/api/v1/ws?ticket={ticket}")).await;

    assert!(result.is_err());
}
