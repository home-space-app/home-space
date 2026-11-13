use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_bytes, body_partial_json, header, method, path},
};

use api_client::{Client, FALLBACK_UPLOAD_CHUNK_BYTES};
use protocol::{NodeId, paths};

const FILE_ID: &str = "00000000-0000-0000-0000-000000000009";
const UPLOAD_ID: &str = "00000000-0000-0000-0000-0000000000aa";

async fn signed_in(server: &MockServer) -> Client {
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 900,
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(paths::ME))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "username": "alice",
            "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
        })))
        .mount(server)
        .await;
    let client = Client::new(server.uri());
    client.login("alice", "pw").await.expect("login");
    client
}

fn file_id() -> NodeId {
    FILE_ID.parse().expect("id")
}

fn stored(size: usize) -> serde_json::Value {
    json!({
        "id": FILE_ID,
        "parent_id": null,
        "kind": "file",
        "name": "notes.txt",
        "size": size,
        "sha256": "b",
        "mime": "text/plain",
        "version": 2,
        "created_at": "2026-11-01T10:00:00Z",
        "updated_at": "2026-11-02T11:30:00Z",
    })
}

#[tokio::test]
async fn small_content_replaces_the_file_in_one_request() {
    let server = MockServer::start().await;
    let client = signed_in(&server).await;
    Mock::given(method("PUT"))
        .and(path(paths::file_content(file_id())))
        .and(header("authorization", "Bearer access"))
        .and(body_bytes(b"hello".to_vec()))
        .respond_with(ResponseTemplate::new(200).set_body_json(stored(5)))
        .expect(1)
        .mount(&server)
        .await;

    let node = client
        .replace_with_bytes(file_id(), Some("text/plain"), b"hello".to_vec())
        .await
        .expect("replaced");

    assert_eq!(node.version, 2);
}

#[tokio::test]
async fn content_over_one_chunk_replaces_the_file_in_chunks() {
    let server = MockServer::start().await;
    let client = signed_in(&server).await;
    let size = usize::try_from(FALLBACK_UPLOAD_CHUNK_BYTES).expect("size") + 1;
    Mock::given(method("POST"))
        .and(path(paths::UPLOADS))
        .and(body_partial_json(
            json!({ "node_id": FILE_ID, "size": size }),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": UPLOAD_ID, "offset": 0, "size": size, "chunk_size": size,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(paths::upload(UPLOAD_ID.parse().expect("id"))))
        .and(header(paths::UPLOAD_OFFSET, "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "offset": FALLBACK_UPLOAD_CHUNK_BYTES,
        })))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(paths::upload(UPLOAD_ID.parse().expect("id"))))
        .and(header(
            paths::UPLOAD_OFFSET,
            FALLBACK_UPLOAD_CHUNK_BYTES.to_string(),
        ))
        .and(body_bytes(vec![b'x']))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "offset": size, "node": stored(size),
        })))
        .expect(1)
        .mount(&server)
        .await;

    let node = client
        .replace_with_bytes(file_id(), Some("text/plain"), vec![b'x'; size])
        .await
        .expect("replaced");

    assert_eq!(node.version, 2);
}
