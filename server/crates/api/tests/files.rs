mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::setup;

fn id_of(response: &common::Response) -> String {
    response.json()["id"].as_str().expect("id").to_owned()
}

#[sqlx::test(migrations = "../../migrations")]
async fn uploaded_file_downloads_with_same_content(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let created = app.upload(&token, "name=hello.txt", "hello world").await;
    assert_eq!(created.status, StatusCode::CREATED);

    let download = app
        .get(
            &format!("/api/v1/files/{}/content", id_of(&created)),
            &token,
        )
        .await;

    assert_eq!(download.body, b"hello world");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_interrupted_download_of_ones_own_file_resumes(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let created = app.upload(&token, "name=hello.txt", "hello world").await;
    let uri = format!("/api/v1/files/{}/content", id_of(&created));

    let rest = app
        .send_with(
            Method::GET,
            &uri,
            Some(&token),
            Body::empty(),
            None,
            &[("range", "bytes=6-")],
        )
        .await;

    assert_eq!(rest.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(rest.body, b"world");
    assert_eq!(rest.headers["content-range"], "bytes 6-10/11");
}

#[sqlx::test(migrations = "../../migrations")]
async fn declared_content_type_is_stored_as_mime(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let image = app
        .upload_as(&token, "name=photo.bin", "x", "image/png")
        .await;
    let text = app.upload(&token, "name=notes.txt", "x").await;

    assert_eq!(image.json()["mime"], "image/png");
    assert_eq!(text.json()["mime"], "text/plain");
}

#[sqlx::test(migrations = "../../migrations")]
async fn listing_returns_children_of_a_folder(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let folder = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            json!({ "name": "docs" }),
        )
        .await;
    let folder_id = id_of(&folder);
    app.upload(&token, &format!("name=a.txt&parent_id={folder_id}"), "a")
        .await;

    let listing = app
        .get(&format!("/api/v1/nodes?parent_id={folder_id}"), &token)
        .await;
    let root = app.get("/api/v1/nodes", &token).await;

    assert_eq!(listing.json().as_array().map(Vec::len), Some(1));
    // The folder and Lost & Found.
    assert_eq!(root.json().as_array().map(Vec::len), Some(2));
}

async fn create_folder(
    app: &common::TestApp,
    token: &str,
    name: &str,
    parent: Option<&str>,
) -> common::Response {
    app.json(
        Method::POST,
        "/api/v1/folders",
        Some(token),
        json!({ "name": name, "parent_id": parent }),
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_folder_names_conflict(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    create_folder(&app, &token, "docs", None).await;
    let second = create_folder(&app, &token, "docs", None).await;
    assert_eq!(second.status, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn folders_have_a_directory_mime(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let folder = create_folder(&app, &token, "docs", None).await;
    assert_eq!(folder.json()["mime"], "inode/directory");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_file_and_a_folder_may_share_a_name(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    app.upload(&token, "name=same", "1").await;
    let folder = create_folder(&app, &token, "same", None).await;
    assert_eq!(folder.status, StatusCode::CREATED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn uploading_identical_content_keeps_the_file_unchanged(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    app.upload(&token, "name=a.txt", "same").await;
    let second = app.upload(&token, "name=a.txt", "same").await;
    let versions = app
        .get(
            &format!("/api/v1/nodes/{}/versions", id_of(&second)),
            &token,
        )
        .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(second.json()["version"], 1);
    assert_eq!(versions.json().as_array().map(Vec::len), Some(0));
}

#[sqlx::test(migrations = "../../migrations")]
async fn uploading_different_content_creates_a_downloadable_version(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let first = app.upload(&token, "name=a.txt", "one").await;
    let second = app.upload(&token, "name=a.txt", "two!").await;
    let id = id_of(&first);
    let old = app
        .get(&format!("/api/v1/files/{id}/versions/1/content"), &token)
        .await;
    let current = app
        .get(&format!("/api/v1/files/{id}/content"), &token)
        .await;
    let versions = app
        .get(&format!("/api/v1/nodes/{id}/versions"), &token)
        .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(id_of(&second), id);
    assert_eq!(second.json()["version"], 2);
    assert_eq!(old.body, b"one");
    assert_eq!(current.body, b"two!");
    assert_eq!(versions.json()[0]["version"], 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_version_removes_it(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let first = app.upload(&token, "name=a.txt", "one").await;
    app.upload(&token, "name=a.txt", "two").await;
    let id = id_of(&first);
    let deleted = app
        .send(
            Method::DELETE,
            &format!("/api/v1/nodes/{id}/versions/1"),
            Some(&token),
            Body::empty(),
            None,
        )
        .await;
    let gone = app
        .get(&format!("/api/v1/files/{id}/versions/1/content"), &token)
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_users_cannot_see_versions(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let file = app.upload(&alice, "name=a.txt", "one").await;
    app.upload(&alice, "name=a.txt", "two").await;
    let id = id_of(&file);
    let list = app.get(&format!("/api/v1/nodes/{id}/versions"), &bob).await;
    let content = app
        .get(&format!("/api/v1/files/{id}/versions/1/content"), &bob)
        .await;
    assert_eq!(list.status, StatusCode::NOT_FOUND);
    assert_eq!(content.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn moving_a_folder_onto_a_same_named_folder_merges_it(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let target = id_of(&create_folder(&app, &token, "target", None).await);
    let kept = id_of(&create_folder(&app, &token, "docs", Some(&target)).await);
    let source = id_of(&create_folder(&app, &token, "docs", None).await);
    app.upload(&token, &format!("name=a.txt&parent_id={source}"), "1")
        .await;
    app.upload(&token, &format!("name=a.txt&parent_id={kept}"), "2")
        .await;

    let moved = app
        .json(
            Method::PATCH,
            &format!("/api/v1/nodes/{source}"),
            Some(&token),
            json!({ "parent_id": target }),
        )
        .await;
    let listing = app
        .get(&format!("/api/v1/nodes?parent_id={kept}"), &token)
        .await;
    let gone = app.get(&format!("/api/v1/nodes/{source}"), &token).await;

    assert_eq!(id_of(&moved), kept);
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    assert_eq!(listing.json().as_array().map(Vec::len), Some(1));
    assert_eq!(listing.json()[0]["version"], 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn renaming_onto_a_taken_name_conflicts(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    app.upload(&token, "name=a.txt", "1").await;
    let other = app.upload(&token, "name=b.txt", "2").await;
    let response = app
        .json(
            Method::PATCH,
            &format!("/api/v1/nodes/{}", id_of(&other)),
            Some(&token),
            json!({ "name": "a.txt" }),
        )
        .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_names_are_rejected(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let response = app.upload(&token, "name=..", "1").await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../../migrations")]
async fn moving_a_folder_into_its_child_is_rejected(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let outer = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            json!({ "name": "outer" }),
        )
        .await;
    let inner = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            json!({ "name": "inner", "parent_id": id_of(&outer) }),
        )
        .await;

    let response = app
        .json(
            Method::PATCH,
            &format!("/api/v1/nodes/{}", id_of(&outer)),
            Some(&token),
            json!({ "parent_id": id_of(&inner) }),
        )
        .await;

    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rename_and_move_to_root_work(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let folder = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            json!({ "name": "docs" }),
        )
        .await;
    let file = app
        .upload(
            &token,
            &format!("name=a.txt&parent_id={}", id_of(&folder)),
            "a",
        )
        .await;

    let moved = app
        .json(
            Method::PATCH,
            &format!("/api/v1/nodes/{}", id_of(&file)),
            Some(&token),
            json!({ "name": "b.txt", "parent_id": null }),
        )
        .await;

    assert_eq!(moved.json()["name"], "b.txt");
    assert_eq!(moved.json()["parent_id"], Value::Null);
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_removes_the_node(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let file = app.upload(&token, "name=a.txt", "a").await;
    let uri = format!("/api/v1/nodes/{}", id_of(&file));

    let deleted = app
        .send(Method::DELETE, &uri, Some(&token), Body::empty(), None)
        .await;
    let after = app.get(&uri, &token).await;

    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(after.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn oversized_upload_returns_413(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let big = "x".repeat(1024 * 1024 + 1);
    let response = app
        .send(
            Method::POST,
            "/api/v1/files?name=big.bin",
            Some(&token),
            Body::from(big),
            None,
        )
        .await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[sqlx::test(migrations = "../../migrations")]
async fn replace_content_changes_downloaded_bytes(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let file = app.upload(&token, "name=a.txt", "old").await;
    let uri = format!("/api/v1/files/{}/content", id_of(&file));

    app.send(
        Method::PUT,
        &uri,
        Some(&token),
        Body::from("new content"),
        None,
    )
    .await;

    assert_eq!(app.get(&uri, &token).await.body, b"new content");
}

#[sqlx::test(migrations = "../../migrations")]
async fn ancestors_endpoint_returns_the_chain_and_hides_foreign_nodes(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let parent = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&alice),
            json!({ "name": "docs" }),
        )
        .await
        .json()["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let child = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&alice),
            json!({ "name": "inner", "parent_id": parent }),
        )
        .await
        .json()["id"]
        .as_str()
        .expect("id")
        .to_owned();

    let own = app
        .get(&format!("/api/v1/nodes/{child}/ancestors"), &alice)
        .await;
    let foreign = app
        .get(&format!("/api/v1/nodes/{child}/ancestors"), &bob)
        .await;

    assert_eq!(own.json()[0]["name"], "docs");
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
}
