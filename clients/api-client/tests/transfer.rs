use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_bytes, header, method, path, query_param},
};

use api_client::{ApiError, Client};
use protocol::{ErrorCode, NodeId};
use time::macros::datetime;

const USER_ID: &str = "00000000-0000-0000-0000-000000000001";
const NODE_ID: &str = "00000000-0000-0000-0000-000000000002";
const PARENT_ID: &str = "00000000-0000-0000-0000-000000000003";

fn pair(access: &str, refresh: &str) -> Value {
    json!({ "access_token": access, "refresh_token": refresh, "expires_in": 900 })
}

fn node_json() -> Value {
    json!({
        "id": NODE_ID,
        "parent_id": PARENT_ID,
        "name": "a.txt",
        "kind": "file",
        "size": 5,
        "sha256": null,
        "mime": "text/plain",
        "version": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
    })
}

fn node_id(raw: &str) -> NodeId {
    NodeId(raw.parse().expect("uuid"))
}

async fn logged_in(server: &MockServer) -> Client {
    let client = Client::new(server.uri());
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("old", "refresh-1")))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": USER_ID, "username": "alice", "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0" })),
        )
        .mount(server)
        .await;
    client.login("alice", "pw").await.expect("login");
    client
}

#[tokio::test]
async fn upload_sends_name_parent_and_body() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/files"))
        .and(query_param("name", "a.txt"))
        .and(query_param("parent_id", PARENT_ID))
        .and(header("authorization", "Bearer old"))
        .and(body_bytes(b"hello".to_vec()))
        .respond_with(ResponseTemplate::new(201).set_body_json(node_json()))
        .expect(1)
        .mount(&server)
        .await;

    let node = client
        .upload(Some(node_id(PARENT_ID)), "a.txt", || {
            reqwest::Body::from("hello")
        })
        .await
        .expect("upload");

    assert_eq!(node.name, "a.txt");
}

#[tokio::test]
async fn upload_modified_sends_the_modification_time() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/files"))
        .and(query_param("name", "a.txt"))
        .and(query_param("modified_at", "2026-01-02T03:04:05Z"))
        .respond_with(ResponseTemplate::new(201).set_body_json(node_json()))
        .expect(1)
        .mount(&server)
        .await;

    client
        .upload_modified(
            None,
            "a.txt",
            Some(datetime!(2026-01-02 03:04:05 UTC)),
            || reqwest::Body::from("hello"),
        )
        .await
        .expect("upload");
}

#[tokio::test]
async fn replace_content_puts_the_body_with_the_modification_time() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/files/{NODE_ID}/content")))
        .and(query_param("modified_at", "2026-01-02T03:04:05Z"))
        .and(body_bytes(b"hello".to_vec()))
        .respond_with(ResponseTemplate::new(200).set_body_json(node_json()))
        .expect(1)
        .mount(&server)
        .await;

    let node = client
        .replace_content(
            node_id(NODE_ID),
            Some(datetime!(2026-01-02 03:04:05 UTC)),
            || reqwest::Body::from("hello"),
        )
        .await
        .expect("replace");

    assert_eq!(node.name, "a.txt");
}

#[tokio::test]
async fn upload_is_retried_with_a_fresh_body_after_a_refresh() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/files"))
        .and(header("authorization", "Bearer old"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("new", "refresh-2")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/files"))
        .and(header("authorization", "Bearer new"))
        .and(body_bytes(b"hello".to_vec()))
        .respond_with(ResponseTemplate::new(201).set_body_json(node_json()))
        .expect(1)
        .mount(&server)
        .await;

    client
        .upload(None, "a.txt", || reqwest::Body::from("hello"))
        .await
        .expect("upload");
}

#[tokio::test]
async fn download_returns_the_file_bytes() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/files/{NODE_ID}/content")))
        .and(header("authorization", "Bearer old"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
        .mount(&server)
        .await;

    let response = client.download(node_id(NODE_ID)).await.expect("download");

    assert_eq!(response.bytes().await.expect("bytes").as_ref(), b"hello");
}

#[tokio::test]
async fn download_of_a_missing_file_is_an_api_error() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/files/{NODE_ID}/content")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": "not_found",
            "message": "node not found",
        })))
        .mount(&server)
        .await;

    let err = client
        .download(node_id(NODE_ID))
        .await
        .expect_err("missing");

    assert!(matches!(
        err,
        ApiError::Api {
            code: ErrorCode::NotFound,
            ..
        }
    ));
}

#[tokio::test]
async fn changes_sends_the_cursor_and_decodes_the_page() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/changes"))
        .and(query_param("since", "41"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "changes": [
                { "type": "upserted", "seq": 42, "node": node_json() },
                { "type": "deleted", "seq": 43, "id": PARENT_ID },
            ],
            "cursor": 43,
            "more": false,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let page = client.changes(Some(41)).await.expect("changes");

    assert_eq!((page.changes.len(), page.cursor, page.more), (2, 43, false));
}

#[tokio::test]
async fn changes_after_a_pruned_cursor_are_gone() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/changes"))
        .respond_with(
            ResponseTemplate::new(410)
                .set_body_json(json!({ "error": "gone", "message": "no longer available" })),
        )
        .mount(&server)
        .await;

    let result = client.changes(Some(1)).await;

    assert!(matches!(
        result,
        Err(ApiError::Api {
            code: ErrorCode::Gone,
            ..
        })
    ));
}

const TRANSFER_ID: &str = "0193a4a0-0000-7000-8000-0000000000aa";
const ITEM_ID: &str = "0193a4a0-0000-7000-8000-0000000000bb";

/// `GET /transfers/{id}` for a transfer of one file of `size` bytes.
async fn one_file_transfer(server: &MockServer, size: u64) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transfers/{TRANSFER_ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": TRANSFER_ID,
            "sender": { "id": USER_ID, "username": "alice" },
            "recipient": { "id": USER_ID, "username": "alice" },
            "recipient_device": "00000000-0000-0000-0000-0000000000d1",
            "recipient_device_name": "Laptop",
            "state": "ready",
            "total_bytes": size,
            "item_count": 1,
            "file_count": 1,
            "name": "link.uris",
            "mime": "text/uri-list",
            "created_at": "2026-01-01T00:00:00Z",
            "expires_at": "2026-01-02T00:00:00Z",
            "items": [{
                "id": ITEM_ID,
                "path": "link.uris",
                "kind": "file",
                "size": size,
                "mime": "text/uri-list",
            }],
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_one_file_transfer_is_read_into_memory() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    one_file_transfer(&server, 21).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/transfers/{TRANSFER_ID}/items/{ITEM_ID}/content"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(b"https://example.com\r\n".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let body = client
        .transfer_file_bytes(TRANSFER_ID.parse().expect("uuid"), 100)
        .await
        .expect("bytes");

    assert_eq!(body, b"https://example.com\r\n");
}

#[tokio::test]
async fn a_file_over_the_limit_is_not_downloaded() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    one_file_transfer(&server, 101).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/transfers/{TRANSFER_ID}/items/{ITEM_ID}/content"
        )))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let err = client
        .transfer_file_bytes(TRANSFER_ID.parse().expect("uuid"), 100)
        .await
        .expect_err("too large");

    assert!(matches!(
        err,
        ApiError::Api {
            code: ErrorCode::PayloadTooLarge,
            ..
        }
    ));
}

#[tokio::test]
async fn a_body_longer_than_announced_is_cut_off() {
    let server = MockServer::start().await;
    let client = logged_in(&server).await;
    one_file_transfer(&server, 10).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/transfers/{TRANSFER_ID}/items/{ITEM_ID}/content"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 200]))
        .mount(&server)
        .await;

    let err = client
        .transfer_file_bytes(TRANSFER_ID.parse().expect("uuid"), 100)
        .await
        .expect_err("too large");

    assert!(matches!(
        err,
        ApiError::Api {
            code: ErrorCode::PayloadTooLarge,
            ..
        }
    ));
}
