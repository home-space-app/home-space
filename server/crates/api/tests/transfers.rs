//! Sending files and folders to a device: who can see a device, making a transfer from the
//! sender's tree or from uploads, and what the recipient can do with it.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::{Response, TestApp, setup};
use protocol::{Event, TransferState};

const TRANSFERS: &str = "/api/v1/transfers";
const TARGETS: &str = "/api/v1/devices/targets";

struct Users {
    alice: String,
    alice_laptop: Uuid,
    alice_phone: Uuid,
    bob: String,
    bob_laptop: Uuid,
    carol: String,
}

async fn users(app: &TestApp) -> Users {
    let (alice_laptop, alice_phone, bob_laptop) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    app.sign_in_from("alice", alice_phone, "Phone").await;
    let alice = app.sign_in_from("alice", alice_laptop, "Laptop").await;
    let bob = app.sign_in_from("bob", bob_laptop, "Bob's laptop").await;
    let carol = app.sign_in_from("carol", Uuid::now_v7(), "Carol's").await;
    Users {
        alice,
        alice_laptop,
        alice_phone,
        bob,
        bob_laptop,
        carol,
    }
}

async fn set_visibility(app: &TestApp, token: &str, device: Uuid, body: Value) -> StatusCode {
    app.json(
        Method::PUT,
        &format!("/api/v1/devices/{device}/visibility"),
        Some(token),
        body,
    )
    .await
    .status
}

async fn targets(app: &TestApp, token: &str) -> Vec<(String, bool, Vec<Uuid>)> {
    let response = app.get(TARGETS, token).await;
    assert_eq!(response.status, StatusCode::OK);
    response
        .json()
        .as_array()
        .expect("targets")
        .iter()
        .map(|target| {
            let devices = target["devices"]
                .as_array()
                .expect("devices")
                .iter()
                .map(|device| device["id"].as_str().expect("id").parse().expect("uuid"))
                .collect();
            (
                target["user"]["username"]
                    .as_str()
                    .expect("username")
                    .to_owned(),
                target["own"].as_bool().expect("own"),
                devices,
            )
        })
        .collect()
}

async fn send(app: &TestApp, token: &str, to: &str, device: Uuid, items: Value) -> Response {
    let recipient = app.user_id(to).await;
    app.json(
        Method::POST,
        TRANSFERS,
        Some(token),
        json!({ "recipient_id": recipient, "recipient_device": device, "items": items }),
    )
    .await
}

async fn post(app: &TestApp, uri: &str, token: &str) -> StatusCode {
    app.send(Method::POST, uri, Some(token), Body::empty(), None)
        .await
        .status
}

async fn upload_file(app: &TestApp, token: &str, name: &str, content: &'static str) -> String {
    let response = app.upload(token, &format!("name={name}"), content).await;
    assert_eq!(response.status, StatusCode::CREATED);
    response.json()["id"].as_str().expect("id").to_owned()
}

/// Uploads `content` as the bytes of a transfer item, in chunks.
async fn upload_item(app: &TestApp, token: &str, item: &str, content: &str) -> Response {
    let created = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(token),
            json!({ "transfer_item_id": item, "size": content.len() }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.json());
    let id = created.json()["id"].as_str().expect("id").to_owned();
    let mut last = created;
    for (offset, chunk) in content
        .as_bytes()
        .chunks(usize::try_from(common::UPLOAD_CHUNK).expect("chunk"))
        .scan(0, |offset, chunk| {
            let start = *offset;
            *offset += chunk.len();
            Some((start, chunk.to_vec()))
        })
    {
        last = app
            .send_with(
                Method::PATCH,
                &format!("/api/v1/uploads/{id}"),
                Some(token),
                Body::from(chunk),
                Some("application/offset+octet-stream"),
                &[("upload-offset", &offset.to_string())],
            )
            .await;
        assert_eq!(last.status, StatusCode::OK, "{:?}", last.json());
    }
    last
}

fn item_id(created: &Value, path: &str) -> String {
    created["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["path"] == path)
        .and_then(|item| item["id"].as_str())
        .expect("item")
        .to_owned()
}

async fn content(app: &TestApp, token: &str, transfer: &str, item: &str) -> Response {
    app.get(
        &format!("{TRANSFERS}/{transfer}/items/{item}/content"),
        token,
    )
    .await
}

async fn incoming(app: &TestApp, token: &str) -> Vec<Value> {
    let response = app.get(&format!("{TRANSFERS}/incoming"), token).await;
    assert_eq!(response.status, StatusCode::OK);
    response.json().as_array().expect("incoming").clone()
}

fn transfers_dir(app: &TestApp) -> std::path::PathBuf {
    app.state.files.store().root().join("transfers")
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_new_device_is_only_visible_to_its_owner(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;

    let own = targets(&app, &u.alice).await;
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].0, "alice");
    assert!(own[0].1);
    assert_eq!(own[0].2.len(), 2);
    assert!(
        targets(&app, &u.bob)
            .await
            .iter()
            .all(|(name, ..)| name == "bob")
    );

    let response = send(
        &app,
        &u.bob,
        "alice",
        u.alice_laptop,
        json!([{ "path": "a.txt", "kind": "file", "size": 1 }]),
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn visibility_decides_who_can_send(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let bob = app.user_id("bob").await;
    let item = json!([{ "path": "a.txt", "kind": "file", "size": 1 }]);

    let status = set_visibility(
        &app,
        &u.alice,
        u.alice_laptop,
        json!({ "visibility": "users", "users": [bob] }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let seen_by_bob = targets(&app, &u.bob).await;
    assert_eq!(seen_by_bob.len(), 2);
    assert_eq!(
        seen_by_bob[1],
        ("alice".to_owned(), false, vec![u.alice_laptop])
    );
    assert_eq!(targets(&app, &u.carol).await.len(), 1);
    let listed = app.get("/api/v1/devices", &u.alice).await.json();
    let laptop = listed
        .as_array()
        .expect("devices")
        .iter()
        .find(|device| device["id"] == json!(u.alice_laptop))
        .expect("laptop")
        .clone();
    assert_eq!(laptop["visibility"], "users");
    assert_eq!(laptop["visible_to"][0]["username"], "bob");
    for (token, expected) in [
        (&u.alice, StatusCode::CREATED),
        (&u.bob, StatusCode::CREATED),
        (&u.carol, StatusCode::FORBIDDEN),
    ] {
        let response = send(&app, token, "alice", u.alice_laptop, item.clone()).await;
        assert_eq!(response.status, expected);
    }

    set_visibility(
        &app,
        &u.alice,
        u.alice_laptop,
        json!({ "visibility": "everyone" }),
    )
    .await;
    let response = send(&app, &u.carol, "alice", u.alice_laptop, item.clone()).await;
    assert_eq!(response.status, StatusCode::CREATED);

    set_visibility(
        &app,
        &u.alice,
        u.alice_laptop,
        json!({ "visibility": "hidden" }),
    )
    .await;
    for token in [&u.alice, &u.bob, &u.carol] {
        let response = send(&app, token, "alice", u.alice_laptop, item.clone()).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN);
    }
    assert_eq!(targets(&app, &u.alice).await[0].2, vec![u.alice_phone]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_cannot_be_shown_to_its_owner_or_by_someone_else(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let alice = app.user_id("alice").await;

    let to_self = json!({ "visibility": "users", "users": [alice] });
    assert_eq!(
        set_visibility(&app, &u.alice, u.alice_laptop, to_self).await,
        StatusCode::BAD_REQUEST
    );
    let everyone = json!({ "visibility": "everyone" });
    assert_eq!(
        set_visibility(&app, &u.bob, u.alice_laptop, everyone).await,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_user_picker_lists_everyone_else(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;

    let listed = app.get("/api/v1/users", &u.alice).await.json();

    let names = listed
        .as_array()
        .expect("users")
        .iter()
        .map(|user| user["username"].as_str().expect("name").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["bob", "carol"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_file_from_the_tree_is_sent_as_it_was(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let mut events = app.state.hub.subscribe(app.user_id("alice").await);
    let node = upload_file(&app, &u.alice, "a.txt", "hello").await;

    let created = send(
        &app,
        &u.alice,
        "alice",
        u.alice_phone,
        json!([{ "path": "note.txt", "kind": "file", "node_id": node }]),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let created = created.json();
    assert_eq!(created["state"], "ready");
    assert_eq!(created["items"][0]["upload_needed"], false);
    let replaced = app
        .send(
            Method::PUT,
            &format!("/api/v1/files/{node}/content"),
            Some(&u.alice),
            Body::from("changed"),
            Some("text/plain"),
        )
        .await;
    assert_eq!(replaced.status, StatusCode::OK);

    let id = created["id"].as_str().expect("id");
    let item = item_id(&created, "note.txt");
    let downloaded = content(&app, &u.alice, id, &item).await;
    assert_eq!(downloaded.status, StatusCode::OK);
    assert_eq!(downloaded.body, b"hello");
    let transfer = app.get(&format!("{TRANSFERS}/{id}"), &u.alice).await.json();
    assert_eq!(
        transfer["items"][0]["sha256"],
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    let received = loop {
        if let Event::TransferReceived { transfer } = events.try_recv().expect("event") {
            break transfer;
        }
    };
    assert_eq!(received.recipient_device, u.alice_phone);
    assert_eq!(received.total_bytes, 5);
    assert_eq!(received.sender_device_name.as_deref(), Some("Laptop"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_folder_from_the_tree_brings_everything_in_it(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let folder = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&u.alice),
            json!({ "name": "Docs" }),
        )
        .await
        .json()["id"]
        .as_str()
        .expect("folder")
        .to_owned();
    app.upload(&u.alice, &format!("name=a.txt&parent_id={folder}"), "aaa")
        .await;

    let created = send(
        &app,
        &u.alice,
        "alice",
        u.alice_phone,
        json!([{ "path": "Sent", "kind": "folder", "node_id": folder }]),
    )
    .await
    .json();

    let paths = created["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["path"].as_str().expect("path").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["Sent", "Sent/a.txt"]);
    assert_eq!(created["state"], "ready");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_transfer_downloads_as_one_zip(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let folder = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&u.alice),
            json!({ "name": "Docs" }),
        )
        .await
        .json()["id"]
        .as_str()
        .expect("folder")
        .to_owned();
    app.upload(&u.alice, &format!("name=a.txt&parent_id={folder}"), "aaa")
        .await;
    app.upload(&u.alice, &format!("name=b.txt&parent_id={folder}"), "bb")
        .await;
    let created = send(
        &app,
        &u.alice,
        "alice",
        u.alice_phone,
        json!([{ "path": "Sent", "kind": "folder", "node_id": folder }]),
    )
    .await
    .json();
    let id = created["id"].as_str().expect("id");

    let listed = incoming(&app, &u.alice).await;
    assert_eq!(listed[0]["name"], "Sent");
    assert_eq!(listed[0]["file_count"], 2);
    assert_eq!(listed[0]["item_count"], 3);

    let archive = format!("{TRANSFERS}/{id}/archive");
    assert_eq!(
        app.get(&archive, &u.carol).await.status,
        StatusCode::NOT_FOUND
    );
    let response = app.get(&archive, &u.alice).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers["content-type"], "application/zip");
    assert!(
        response.headers["content-disposition"]
            .to_str()
            .expect("disposition")
            .ends_with("Sent.zip")
    );
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(response.body)).expect("zip");
    let names = zip
        .file_names()
        .map(|name| name.expect("name").into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["Sent/", "Sent/a.txt", "Sent/b.txt"]);
    let mut text = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("Sent/a.txt").expect("a"), &mut text)
        .expect("read");
    assert_eq!(text, "aaa");

    // Once the download is confirmed, there is nothing left to archive.
    assert_eq!(
        post(&app, &format!("{TRANSFERS}/{id}/received"), &u.alice).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.get(&archive, &u.alice).await.status, StatusCode::GONE);
}

#[sqlx::test(migrations = "../../migrations")]
async fn uploaded_files_make_a_transfer_ready_and_can_be_saved(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    set_visibility(
        &app,
        &u.alice,
        u.alice_laptop,
        json!({ "visibility": "everyone" }),
    )
    .await;
    let mut events = app.state.hub.subscribe(app.user_id("alice").await);

    let created = send(
        &app,
        &u.bob,
        "alice",
        u.alice_laptop,
        json!([
            { "path": "Docs", "kind": "folder" },
            { "path": "Docs/a.txt", "kind": "file", "size": 5, "mime": "text/plain" },
        ]),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let created = created.json();
    assert_eq!(created["state"], "uploading");
    let id = created["id"].as_str().expect("id").to_owned();
    let item = item_id(&created, "Docs/a.txt");
    assert!(incoming(&app, &u.alice).await.is_empty());
    assert_eq!(
        content(&app, &u.alice, &id, &item).await.status,
        StatusCode::CONFLICT
    );

    let last = upload_item(&app, &u.bob, &item, "hello").await;
    assert_eq!(last.json()["offset"], 5);

    let waiting = incoming(&app, &u.alice).await;
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0]["sender"]["username"], "bob");
    assert!(matches!(
        events.try_recv(),
        Ok(Event::TransferReceived { .. })
    ));
    let partial = app
        .send_with(
            Method::GET,
            &format!("{TRANSFERS}/{id}/items/{item}/content"),
            Some(&u.alice),
            Body::empty(),
            None,
            &[("range", "bytes=1-3")],
        )
        .await;
    assert_eq!(partial.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.body, b"ell");
    assert_eq!(partial.headers["content-range"], "bytes 1-3/5");

    let saved = app
        .json(
            Method::POST,
            &format!("{TRANSFERS}/{id}/save"),
            Some(&u.alice),
            json!({}),
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    assert_eq!(saved.json()[0]["name"], "Docs");
    let docs = saved.json()[0]["id"].as_str().expect("docs").to_owned();
    let listed = app
        .get(&format!("/api/v1/nodes?parent_id={docs}"), &u.alice)
        .await
        .json();
    assert_eq!(listed[0]["name"], "a.txt");
    assert_eq!(listed[0]["mime"], "text/plain");
    assert!(!transfers_dir(&app).join(&id).exists());
    assert_eq!(
        content(&app, &u.alice, &id, &item).await.status,
        StatusCode::GONE
    );
    let sent = app
        .get(&format!("{TRANSFERS}/outgoing"), &u.bob)
        .await
        .json();
    assert_eq!(sent[0]["state"], "saved");
}

#[sqlx::test(migrations = "../../migrations")]
async fn saving_next_to_taken_names_numbers_them(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    app.json(
        Method::POST,
        "/api/v1/folders",
        Some(&u.alice),
        json!({ "name": "Docs" }),
    )
    .await;
    upload_file(&app, &u.alice, "b.txt", "old").await;
    let created = send(
        &app,
        &u.alice,
        "alice",
        u.alice_phone,
        json!([
            { "path": "Docs", "kind": "folder" },
            { "path": "b.txt", "kind": "file", "size": 3 },
        ]),
    )
    .await
    .json();
    let id = created["id"].as_str().expect("id");
    upload_item(&app, &u.alice, &item_id(&created, "b.txt"), "new").await;

    let saved = post(&app, &format!("{TRANSFERS}/{id}/save"), &u.alice).await;

    assert_eq!(saved, StatusCode::OK);
    let mut names = app
        .get("/api/v1/nodes", &u.alice)
        .await
        .json()
        .as_array()
        .expect("root")
        .iter()
        .map(|node| node["name"].as_str().expect("name").to_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert!(names.contains(&"Docs (1)".to_owned()), "{names:?}");
    assert!(names.contains(&"b (1).txt".to_owned()), "{names:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_the_recipient_receives_and_only_the_sender_cancels(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    set_visibility(
        &app,
        &u.alice,
        u.alice_laptop,
        json!({ "visibility": "everyone" }),
    )
    .await;
    let node = upload_file(&app, &u.bob, "a.txt", "hello").await;
    let created = send(
        &app,
        &u.bob,
        "alice",
        u.alice_laptop,
        json!([{ "path": "a.txt", "kind": "file", "node_id": node }]),
    )
    .await
    .json();
    let id = created["id"].as_str().expect("id").to_owned();
    let item = item_id(&created, "a.txt");

    assert_eq!(
        app.get(&format!("{TRANSFERS}/{id}"), &u.carol).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        content(&app, &u.bob, &id, &item).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&app, &format!("{TRANSFERS}/{id}/decline"), &u.bob).await,
        StatusCode::FORBIDDEN
    );
    let cancel_by_alice = app
        .send(
            Method::DELETE,
            &format!("{TRANSFERS}/{id}"),
            Some(&u.alice),
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(cancel_by_alice.status, StatusCode::FORBIDDEN);

    assert_eq!(
        post(&app, &format!("{TRANSFERS}/{id}/decline"), &u.alice).await,
        StatusCode::NO_CONTENT
    );
    let waiting = incoming(&app, &u.alice).await;
    assert_eq!(waiting[0]["state"], "declined");
    assert_eq!(
        content(&app, &u.alice, &id, &item).await.status,
        StatusCode::GONE
    );
    assert_eq!(
        post(&app, &format!("{TRANSFERS}/{id}/received"), &u.alice).await,
        StatusCode::GONE
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_confirmed_download_removes_the_bytes(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let mut events = app.state.hub.subscribe(app.user_id("alice").await);
    let node = upload_file(&app, &u.alice, "a.txt", "hello").await;
    let created = send(
        &app,
        &u.alice,
        "alice",
        u.alice_phone,
        json!([{ "path": "a.txt", "kind": "file", "node_id": node }]),
    )
    .await
    .json();
    let id = created["id"].as_str().expect("id").to_owned();
    assert!(transfers_dir(&app).join(&id).exists());

    assert_eq!(
        post(&app, &format!("{TRANSFERS}/{id}/received"), &u.alice).await,
        StatusCode::NO_CONTENT
    );

    assert!(!transfers_dir(&app).join(&id).exists());
    assert_eq!(
        content(&app, &u.alice, &id, &item_id(&created, "a.txt"))
            .await
            .status,
        StatusCode::GONE
    );
    let changed = std::iter::from_fn(|| events.try_recv().ok()).any(|event| {
        matches!(
            event,
            Event::TransferChanged {
                state: TransferState::Downloaded,
                ..
            }
        )
    });
    assert!(changed);
}

#[sqlx::test(migrations = "../../migrations")]
async fn cancelling_ends_the_unfinished_uploads(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let created = send(
        &app,
        &u.bob,
        "bob",
        u.bob_laptop,
        json!([{ "path": "a.txt", "kind": "file", "size": 10 }]),
    )
    .await
    .json();
    let id = created["id"].as_str().expect("id").to_owned();
    let started = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(&u.bob),
            json!({ "transfer_item_id": item_id(&created, "a.txt"), "size": 10 }),
        )
        .await;
    assert_eq!(started.status, StatusCode::CREATED);
    assert_eq!(started.json()["size"], 10);

    let cancelled = app
        .send(
            Method::DELETE,
            &format!("{TRANSFERS}/{id}"),
            Some(&u.bob),
            Body::empty(),
            None,
        )
        .await;

    assert_eq!(cancelled.status, StatusCode::NO_CONTENT);
    let uploads = app.get("/api/v1/uploads", &u.bob).await.json();
    assert_eq!(uploads.as_array().map(Vec::len), Some(0));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_upload_must_match_its_item(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let created = send(
        &app,
        &u.bob,
        "bob",
        u.bob_laptop,
        json!([{ "path": "a.txt", "kind": "file", "size": 10 }]),
    )
    .await
    .json();
    let item = item_id(&created, "a.txt");

    let wrong_size = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(&u.bob),
            json!({ "transfer_item_id": item, "size": 3 }),
        )
        .await;
    let by_someone_else = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(&u.alice),
            json!({ "transfer_item_id": item, "size": 10 }),
        )
        .await;

    assert_eq!(wrong_size.status, StatusCode::BAD_REQUEST);
    assert_eq!(by_someone_else.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_manifest_with_bad_paths_is_rejected(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let file = |path: &str| json!({ "path": path, "kind": "file", "size": 1 });

    for items in [
        json!([]),
        json!([file("../a")]),
        json!([file("/a")]),
        json!([file("a\\b")]),
        json!([file("Docs/a")]),
        json!([file("a"), file("a")]),
    ] {
        let response = send(&app, &u.alice, "alice", u.alice_phone, items.clone()).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{items}");
    }
    assert!(
        std::fs::read_dir(transfers_dir(&app))
            .expect("dir")
            .next()
            .is_none()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn expired_transfers_are_removed_with_their_bytes(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let u = users(&app).await;
    let node = upload_file(&app, &u.alice, "a.txt", "hello").await;
    let created = send(
        &app,
        &u.alice,
        "alice",
        u.alice_phone,
        json!([{ "path": "a.txt", "kind": "file", "node_id": node }]),
    )
    .await
    .json();
    let id = created["id"].as_str().expect("id").to_owned();
    sqlx::query("UPDATE transfers SET expires_at = now() - interval '1 minute'")
        .execute(&pool)
        .await
        .expect("age");
    assert_eq!(
        app.get(&format!("{TRANSFERS}/{id}"), &u.alice).await.status,
        StatusCode::GONE
    );

    let expired = app.state.files.expire_transfers().await.expect("expire");

    assert_eq!(expired.len(), 1);
    assert!(!transfers_dir(&app).join(&id).exists());
    assert_eq!(
        app.get(&format!("{TRANSFERS}/{id}"), &u.alice).await.status,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn nothing_is_sent_to_a_disabled_user_or_one_marked_for_deletion(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    let (alice, bob) = (app.user_id("alice").await, app.user_id("bob").await);
    set_visibility(
        &app,
        &u.alice,
        u.alice_laptop,
        json!({ "visibility": "users", "users": [bob] }),
    )
    .await;
    let item = json!([{ "path": "a.txt", "kind": "file", "size": 3 }]);
    let created = send(&app, &u.bob, "alice", u.alice_laptop, item.clone()).await;
    assert_eq!(created.status, StatusCode::CREATED);
    let waiting = item_id(&created.json(), "a.txt");

    let later = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
    app.state
        .db
        .mark_for_deletion(alice, later)
        .await
        .expect("mark");

    // Alice's devices are no longer offered, and sending to one anyway is refused.
    assert_eq!(targets(&app, &u.bob).await.len(), 1);
    let refused = send(&app, &u.bob, "alice", u.alice_laptop, item.clone()).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    // Nor do the bytes of a transfer made before the mark still arrive.
    let upload = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(&u.bob),
            json!({ "transfer_item_id": waiting, "size": 3 }),
        )
        .await;
    assert_eq!(upload.status, StatusCode::GONE);
    assert!(
        app.get("/api/v1/uploads", &u.bob)
            .await
            .json()
            .as_array()
            .expect("uploads")
            .is_empty()
    );

    app.state.db.cancel_deletion(alice).await.expect("cancel");
    assert_eq!(targets(&app, &u.bob).await.len(), 2);
    let sent = send(&app, &u.bob, "alice", u.alice_laptop, item.clone()).await;
    assert_eq!(sent.status, StatusCode::CREATED);

    // A disabled user gets nothing either.
    app.state
        .db
        .set_disabled(alice, true)
        .await
        .expect("disable");
    assert_eq!(targets(&app, &u.bob).await.len(), 1);
    let refused = send(&app, &u.bob, "alice", u.alice_laptop, item).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn saving_a_transfer_counts_against_the_recipients_quota(pool: PgPool) {
    let app = setup(pool).await;
    let u = users(&app).await;
    set_visibility(
        &app,
        &u.alice,
        u.alice_laptop,
        json!({ "visibility": "everyone" }),
    )
    .await;
    let alice = app.user_id("alice").await;
    app.state.db.set_quota(alice, Some(3)).await.expect("quota");
    let created = send(
        &app,
        &u.bob,
        "alice",
        u.alice_laptop,
        json!([{ "path": "a.txt", "kind": "file", "size": 5 }]),
    )
    .await
    .json();
    let id = created["id"].as_str().expect("id");
    // The bytes belong to the transfer until it is saved, so the sender is not charged.
    let uploaded = upload_item(&app, &u.bob, &item_id(&created, "a.txt"), "hello").await;
    assert!(uploaded.status.is_success(), "{}", uploaded.status);

    let refused = app
        .send(
            Method::POST,
            &format!("{TRANSFERS}/{id}/save"),
            Some(&u.alice),
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(refused.status, StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(refused.json()["error"], "quota_exceeded");

    // Still ready: once there is room, it saves.
    app.state.db.set_quota(alice, Some(5)).await.expect("quota");
    let saved = post(&app, &format!("{TRANSFERS}/{id}/save"), &u.alice).await;
    assert_eq!(saved, StatusCode::OK);
}
