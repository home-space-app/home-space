mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use common::{PASSWORD, Response, TestApp, UPLOAD_CHUNK, setup};

async fn create(app: &TestApp, token: &str, request: Value) -> Response {
    app.json(Method::POST, "/api/v1/uploads", Some(token), request)
        .await
}

async fn start(app: &TestApp, token: &str, name: &str, size: usize) -> String {
    let created = create(app, token, json!({ "name": name, "size": size })).await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.json()["chunk_size"], UPLOAD_CHUNK);
    created.json()["id"].as_str().expect("id").to_owned()
}

async fn patch(app: &TestApp, token: &str, id: &str, offset: &str, chunk: Body) -> Response {
    app.send_with(
        Method::PATCH,
        &format!("/api/v1/uploads/{id}"),
        Some(token),
        chunk,
        None,
        &[("upload-offset", offset)],
    )
    .await
}

/// Sends `content` in chunks of the server's chunk size and returns the last response.
async fn upload_chunked(app: &TestApp, token: &str, id: &str, content: &[u8]) -> Response {
    let mut last = None;
    for (index, chunk) in content.chunks(UPLOAD_CHUNK as usize).enumerate() {
        let offset = (index * UPLOAD_CHUNK as usize).to_string();
        let response = patch(app, token, id, &offset, Body::from(chunk.to_vec())).await;
        assert!(
            response.status.is_success(),
            "chunk at {offset}: {}",
            response.status
        );
        last = Some(response);
    }
    last.expect("at least one chunk")
}

fn offset_header(response: &Response) -> String {
    response.headers["upload-offset"]
        .to_str()
        .expect("header")
        .to_owned()
}

fn sha_hex(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_chunked_upload_stores_the_same_file_as_a_single_post(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let content = b"hello chunked world";
    let single = app
        .upload(&token, "name=single.txt", "hello chunked world")
        .await;
    let id = start(&app, &token, "chunked.txt", content.len()).await;

    let done = upload_chunked(&app, &token, &id, content).await;

    assert_eq!(done.status, StatusCode::CREATED);
    let node = &done.json()["node"];
    assert_eq!(done.json()["offset"], content.len());
    assert_eq!(node["name"], "chunked.txt");
    assert_eq!(node["sha256"], single.json()["sha256"]);
    if let Some(hex) = node["sha256"].as_str() {
        assert_eq!(hex, sha_hex(content));
    }
    let download = app
        .get(
            &format!("/api/v1/files/{}/content", node["id"].as_str().expect("id")),
            &token,
        )
        .await;
    assert_eq!(download.body, content);
    let gone = app.get(&format!("/api/v1/uploads/{id}"), &token).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_wrong_offset_returns_409_with_the_current_offset(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let id = start(&app, &token, "a.bin", 8).await;
    patch(&app, &token, &id, "0", Body::from("abcd")).await;

    let conflict = patch(&app, &token, &id, "2", Body::from("xy")).await;

    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(offset_header(&conflict), "4");
    assert_eq!(conflict.json()["error"], "conflict");
}

#[sqlx::test(migrations = "../../migrations")]
async fn status_reports_the_offset_to_resume_from(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let id = start(&app, &token, "a.txt", 7).await;
    patch(&app, &token, &id, "0", Body::from("abc")).await;

    let status = app.get(&format!("/api/v1/uploads/{id}"), &token).await;
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(status.json()["offset"], 3);
    assert_eq!(offset_header(&status), "3");
    let head = app
        .send(
            Method::HEAD,
            &format!("/api/v1/uploads/{id}"),
            Some(&token),
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(offset_header(&head), "3");

    let done = patch(&app, &token, &id, "3", Body::from("defg")).await;
    assert_eq!(done.status, StatusCode::CREATED);
    assert_eq!(done.json()["node"]["size"], 7);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_interrupted_chunk_resumes_from_what_arrived(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let id = start(&app, &token, "a.txt", 6).await;
    let broken = Body::from_stream(futures_util::stream::iter([
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"abc")),
        Err(std::io::Error::other("connection reset")),
    ]));

    let failed = patch(&app, &token, &id, "0", broken).await;
    assert!(!failed.status.is_success());

    let status = app.get(&format!("/api/v1/uploads/{id}"), &token).await;
    let offset = status.json()["offset"].as_u64().expect("offset");
    assert_eq!(offset, 3);
    let done = patch(&app, &token, &id, "3", Body::from("def")).await;
    assert_eq!(done.status, StatusCode::CREATED);
    let node_id = done.json()["node"]["id"].as_str().expect("id").to_owned();
    let download = app
        .get(&format!("/api/v1/files/{node_id}/content"), &token)
        .await;
    assert_eq!(download.body, b"abcdef");
}

#[sqlx::test(migrations = "../../migrations")]
async fn oversized_uploads_and_chunks_return_413(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let huge = create(
        &app,
        &token,
        json!({ "name": "a.bin", "size": 1024 * 1024 + 1 }),
    )
    .await;
    assert_eq!(huge.status, StatusCode::PAYLOAD_TOO_LARGE);

    let id = start(&app, &token, "a.bin", 100).await;
    let big_chunk = patch(&app, &token, &id, "0", Body::from("x".repeat(5))).await;
    assert_eq!(big_chunk.status, StatusCode::PAYLOAD_TOO_LARGE);

    let small = start(&app, &token, "b.bin", 2).await;
    let past_end = patch(&app, &token, &small, "0", Body::from("abc")).await;
    assert_eq!(past_end.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_missing_offset_header_is_a_bad_request(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let id = start(&app, &token, "a.bin", 4).await;
    let response = app
        .send(
            Method::PATCH,
            &format!("/api/v1/uploads/{id}"),
            Some(&token),
            Body::from("abcd"),
            None,
        )
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_users_cannot_touch_an_upload(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let id = start(&app, &alice, "a.bin", 4).await;
    let uri = format!("/api/v1/uploads/{id}");

    assert_eq!(app.get(&uri, &bob).await.status, StatusCode::NOT_FOUND);
    let chunk = patch(&app, &bob, &id, "0", Body::from("abcd")).await;
    assert_eq!(chunk.status, StatusCode::NOT_FOUND);
    let cancel = app
        .send(Method::DELETE, &uri, Some(&bob), Body::empty(), None)
        .await;
    assert_eq!(cancel.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get(&uri, &alice).await.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replace_upload_creates_a_version(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let file = app.upload(&token, "name=a.txt", "old").await;
    let file_id = file.json()["id"].as_str().expect("id").to_owned();
    let created = create(&app, &token, json!({ "node_id": file_id, "size": 11 })).await;
    let id = created.json()["id"].as_str().expect("id").to_owned();

    let done = upload_chunked(&app, &token, &id, b"new content").await;

    assert_eq!(done.status, StatusCode::OK);
    assert_eq!(done.json()["node"]["id"], file_id.as_str());
    assert_eq!(done.json()["node"]["version"], 2);
    let uri = format!("/api/v1/files/{file_id}/content");
    assert_eq!(app.get(&uri, &token).await.body, b"new content");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_cancelled_upload_is_gone(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let id = start(&app, &token, "a.bin", 8).await;
    let uri = format!("/api/v1/uploads/{id}");
    let cancel = app
        .send(Method::DELETE, &uri, Some(&token), Body::empty(), None)
        .await;
    assert_eq!(cancel.status, StatusCode::NO_CONTENT);
    assert_eq!(app.get(&uri, &token).await.status, StatusCode::NOT_FOUND);
}

const PHONE: Uuid = Uuid::from_u128(0x0193_a4a0_0000_7000_8000_0000_0000_0001);
const TABLET: Uuid = Uuid::from_u128(0x0193_a4a0_0000_7000_8000_0000_0000_0002);

async fn sign_in_from(app: &TestApp, device: Uuid, name: &str) -> String {
    app.auth.create_user("alice", PASSWORD).await.ok();
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({
                "username": "alice",
                "password": PASSWORD,
                "device": { "id": device, "name": name, "platform": "android" },
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    response.json()["access_token"]
        .as_str()
        .expect("access")
        .to_owned()
}

/// Uploads `content` as `name` with the given fields added to the request.
async fn upload_with(
    app: &TestApp,
    token: &str,
    name: &str,
    content: &[u8],
    options: Value,
) -> Response {
    let mut request = json!({ "name": name, "size": content.len() });
    request
        .as_object_mut()
        .expect("object")
        .extend(options.as_object().expect("options").clone());
    let created = create(app, token, request).await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.json()["id"].as_str().expect("id").to_owned();
    upload_chunked(app, token, &id, content).await
}

async fn source_device(app: &TestApp, token: &str, node: &Value) -> Value {
    let id = node["id"].as_str().expect("id");
    let response = app
        .get(&format!("/api/v1/nodes/{id}/properties"), token)
        .await;
    assert_eq!(response.status, StatusCode::OK);
    response.json()["source_device"].clone()
}

#[sqlx::test(migrations = "../../migrations")]
async fn keep_both_gives_different_content_a_free_name(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let both = json!({ "keep_both": true });

    let first = upload_with(&app, &token, "a.txt", b"first", both.clone()).await;
    let second = upload_with(&app, &token, "a.txt", b"second", both.clone()).await;
    let third = upload_with(&app, &token, "a.txt", b"third", both).await;

    assert_eq!(first.json()["node"]["name"], "a.txt");
    assert_eq!(second.status, StatusCode::CREATED);
    assert_eq!(second.json()["node"]["name"], "a (1).txt");
    assert_eq!(third.json()["node"]["name"], "a (2).txt");
    assert_eq!(first.json()["node"]["version"], 1);
    let listed = app.get("/api/v1/nodes", &token).await.json();
    let names: Vec<&str> = listed
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node["name"].as_str())
        .filter(|name| name.starts_with("a"))
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn keep_both_recognises_the_same_file_arriving_again(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let both = json!({ "keep_both": true });
    upload_with(&app, &token, "a.txt", b"first", both.clone()).await;
    let second = upload_with(&app, &token, "a.txt", b"second", both.clone()).await;

    let retried = upload_with(&app, &token, "a.txt", b"second", both).await;

    assert_eq!(retried.status, StatusCode::OK);
    assert_eq!(retried.json()["node"]["id"], second.json()["node"]["id"]);
    assert_eq!(retried.json()["node"]["name"], "a (1).txt");
    assert_eq!(retried.json()["node"]["version"], 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn version_of_makes_a_new_version_of_that_file(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let first = upload_with(
        &app,
        &token,
        "a.txt",
        b"first",
        json!({ "keep_both": true }),
    )
    .await;
    let id = first.json()["node"]["id"].clone();

    let edit = upload_with(
        &app,
        &token,
        "a.txt",
        b"edited",
        json!({ "keep_both": true, "version_of": id }),
    )
    .await;

    assert_eq!(edit.status, StatusCode::OK);
    assert_eq!(edit.json()["node"]["id"], id);
    assert_eq!(edit.json()["node"]["name"], "a.txt");
    assert_eq!(edit.json()["node"]["version"], 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn version_of_keeps_a_different_file_that_took_the_name(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let first = upload_with(
        &app,
        &token,
        "a.txt",
        b"first",
        json!({ "keep_both": true }),
    )
    .await;
    let id = first.json()["node"]["id"].as_str().expect("id").to_owned();
    let renamed = app
        .json(
            Method::PATCH,
            &format!("/api/v1/nodes/{id}"),
            Some(&token),
            json!({ "name": "moved.txt" }),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK);
    let other = upload_with(&app, &token, "a.txt", b"other", json!({})).await;

    let edit = upload_with(
        &app,
        &token,
        "a.txt",
        b"edited",
        json!({ "keep_both": true, "version_of": id }),
    )
    .await;

    assert_eq!(edit.status, StatusCode::CREATED);
    assert_eq!(edit.json()["node"]["name"], "a (1).txt");
    assert_ne!(edit.json()["node"]["id"], other.json()["node"]["id"]);
    assert_ne!(edit.json()["node"]["id"], id.as_str());
    let listed = app.get("/api/v1/nodes", &token).await.json();
    let at = |name: &str| {
        listed
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["name"] == name)
            .cloned()
            .expect(name)
    };
    assert_eq!(at("a.txt")["version"], 1);
    assert_eq!(at("moved.txt")["version"], 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn version_of_makes_a_new_file_when_the_name_is_free(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let first = upload_with(
        &app,
        &token,
        "a.txt",
        b"first",
        json!({ "keep_both": true }),
    )
    .await;
    let id = first.json()["node"]["id"].as_str().expect("id").to_owned();
    let deleted = app
        .send(
            Method::DELETE,
            &format!("/api/v1/nodes/{id}"),
            Some(&token),
            Body::empty(),
            None,
        )
        .await;
    assert!(deleted.status.is_success());

    let edit = upload_with(
        &app,
        &token,
        "a.txt",
        b"edited",
        json!({ "keep_both": true, "version_of": id }),
    )
    .await;

    assert_eq!(edit.status, StatusCode::CREATED);
    assert_eq!(edit.json()["node"]["name"], "a.txt");
    assert_ne!(edit.json()["node"]["id"], id.as_str());
}

#[sqlx::test(migrations = "../../migrations")]
async fn two_phones_with_the_same_name_keep_their_own_files_through_edits(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let both = json!({ "keep_both": true });
    let one = upload_with(&app, &token, "a.txt", b"one", both.clone()).await;
    let two = upload_with(&app, &token, "a.txt", b"two", both).await;
    assert_eq!(two.json()["node"]["name"], "a (1).txt");

    let edit_one = upload_with(
        &app,
        &token,
        "a.txt",
        b"one edited",
        json!({ "keep_both": true, "version_of": one.json()["node"]["id"] }),
    )
    .await;
    let edit_two = upload_with(
        &app,
        &token,
        "a (1).txt",
        b"two edited",
        json!({ "keep_both": true, "version_of": two.json()["node"]["id"] }),
    )
    .await;

    assert_eq!(edit_one.json()["node"]["id"], one.json()["node"]["id"]);
    assert_eq!(edit_one.json()["node"]["version"], 2);
    assert_eq!(edit_two.json()["node"]["id"], two.json()["node"]["id"]);
    assert_eq!(edit_two.json()["node"]["version"], 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn without_keep_both_a_taken_name_still_becomes_a_version(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let first = upload_with(&app, &token, "a.txt", b"first", json!({})).await;

    let second = upload_with(&app, &token, "a.txt", b"second", json!({})).await;

    assert_eq!(second.json()["node"]["id"], first.json()["node"]["id"]);
    assert_eq!(second.json()["node"]["name"], "a.txt");
    assert_eq!(second.json()["node"]["version"], 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_auto_upload_records_the_name_of_the_device(pool: PgPool) {
    let app = setup(pool).await;
    let token = sign_in_from(&app, PHONE, "Phone").await;

    let done = upload_with(
        &app,
        &token,
        "a.txt",
        b"first",
        json!({ "auto_upload": true }),
    )
    .await;
    let node = done.json()["node"].clone();
    assert_eq!(source_device(&app, &token, &node).await, "Phone");

    // A later rename of the device leaves what was recorded.
    let renamed = app
        .json(
            Method::PATCH,
            &format!("/api/v1/devices/{PHONE}"),
            Some(&token),
            json!({ "name": "Old phone" }),
        )
        .await;
    assert!(renamed.status.is_success());
    assert_eq!(source_device(&app, &token, &node).await, "Phone");

    // New content from another device replaces it.
    let tablet = sign_in_from(&app, TABLET, "Tablet").await;
    upload_with(
        &app,
        &tablet,
        "a.txt",
        b"second",
        json!({ "auto_upload": true }),
    )
    .await;
    assert_eq!(source_device(&app, &tablet, &node).await, "Tablet");

    // The same content again stores nothing, so it records nothing.
    upload_with(
        &app,
        &token,
        "a.txt",
        b"second",
        json!({ "auto_upload": true }),
    )
    .await;
    assert_eq!(source_device(&app, &token, &node).await, "Tablet");
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_an_auto_upload_from_a_device_records_a_source(pool: PgPool) {
    let app = setup(pool).await;
    let on_phone = sign_in_from(&app, PHONE, "Phone").await;
    let manual = upload_with(&app, &on_phone, "manual.txt", b"data", json!({})).await;
    assert_eq!(
        source_device(&app, &on_phone, &manual.json()["node"]).await,
        Value::Null
    );

    let token = app.user_token("bob").await;
    let no_device = upload_with(
        &app,
        &token,
        "a.txt",
        b"data",
        json!({ "auto_upload": true }),
    )
    .await;
    assert_eq!(
        source_device(&app, &token, &no_device.json()["node"]).await,
        Value::Null
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_properties_of_a_node_are_private_and_go_with_it(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = sign_in_from(&app, PHONE, "Phone").await;
    let done = upload_with(
        &app,
        &token,
        "a.txt",
        b"data",
        json!({ "auto_upload": true }),
    )
    .await;
    let id = done.json()["node"]["id"].as_str().expect("id").to_owned();
    let uri = format!("/api/v1/nodes/{id}/properties");

    let bob = app.user_token("bob").await;
    assert_eq!(app.get(&uri, &bob).await.status, StatusCode::NOT_FOUND);

    let binned = app
        .send(
            Method::DELETE,
            &format!("/api/v1/nodes/{id}"),
            Some(&token),
            Body::empty(),
            None,
        )
        .await;
    assert!(binned.status.is_success());
    assert_eq!(app.get(&uri, &token).await.status, StatusCode::NOT_FOUND);
    let purged = app
        .send(
            Method::DELETE,
            &format!("/api/v1/trash/{id}"),
            Some(&token),
            Body::empty(),
            None,
        )
        .await;
    assert!(purged.status.is_success(), "{}", purged.status);

    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM node_properties")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0);
}
