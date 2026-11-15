//! Shares made for a signed-in user: what they read through `/shared`, and copies they save.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::{Response, TestApp, setup};

fn id_of(response: &Response) -> String {
    response.json()["id"].as_str().expect("id").to_owned()
}

fn names(body: &Value) -> Vec<&str> {
    body.as_array()
        .expect("array")
        .iter()
        .map(|node| node["name"].as_str().expect("name"))
        .collect()
}

async fn folder(app: &TestApp, token: &str, name: &str, parent: Option<&str>) -> String {
    let response = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(token),
            json!({ "name": name, "parent_id": parent }),
        )
        .await;
    assert_eq!(response.status, StatusCode::CREATED);
    id_of(&response)
}

async fn file(
    app: &TestApp,
    token: &str,
    name: &str,
    parent: Option<&str>,
    content: &'static str,
) -> String {
    let query = match parent {
        Some(parent) => format!("name={name}&parent_id={parent}"),
        None => format!("name={name}"),
    };
    let response = app.upload(token, &query, content).await;
    assert_eq!(response.status, StatusCode::CREATED);
    id_of(&response)
}

/// Alice shares `node_ids` with bob; answers the share's id.
async fn share_with_bob(app: &TestApp, alice: &str, node_ids: &[&str]) -> String {
    let bob = app.user_id("bob").await.to_string();
    let response = app
        .json(
            Method::POST,
            "/api/v1/shares",
            Some(alice),
            json!({ "node_ids": node_ids, "recipients": [bob] }),
        )
        .await;
    assert_eq!(response.status, StatusCode::CREATED);
    id_of(&response)
}

async fn copy(app: &TestApp, token: &str, share: &str, body: Value) -> Response {
    app.json(
        Method::POST,
        &format!("/api/v1/shared/{share}/copy"),
        Some(token),
        body,
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_user_sees_what_was_shared_with_them(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let report = file(&app, &alice, "report.txt", None, "text").await;
    share_with_bob(&app, &alice, &[&photos, &report]).await;

    let response = app.get("/api/v1/shared", &bob).await;

    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    let shares = body.as_array().expect("shares");
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0]["owner"]["username"], "alice");
    assert_eq!(names(&shares[0]["items"]), ["Photos", "report.txt"]);
    assert!(shares[0]["shared_at"].is_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn others_do_not_see_a_share_that_is_not_for_them(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    app.user_token("bob").await;
    let carol = app.user_token("carol").await;
    let report = file(&app, &alice, "report.txt", None, "text").await;
    let id = share_with_bob(&app, &alice, &[&report]).await;

    let listing = app.get("/api/v1/shared", &carol).await;
    let content = app
        .get(
            &format!("/api/v1/shared/{id}/files/{report}/content"),
            &carol,
        )
        .await;

    assert_eq!(listing.json(), json!([]));
    assert_eq!(content.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_shared_folder_is_browsed_and_its_files_downloaded(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let beach = file(&app, &alice, "beach.jpg", Some(&photos), "sand").await;
    let id = share_with_bob(&app, &alice, &[&photos]).await;

    let children = app
        .get(
            &format!("/api/v1/shared/{id}/nodes?parent_id={photos}"),
            &bob,
        )
        .await;
    let content = app
        .get(&format!("/api/v1/shared/{id}/files/{beach}/content"), &bob)
        .await;

    assert_eq!(names(&children.json()), ["beach.jpg"]);
    assert_eq!(content.body, b"sand");
}

#[sqlx::test(migrations = "../../migrations")]
async fn nothing_outside_the_share_is_reachable(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let secret = file(&app, &alice, "secret.txt", None, "shh").await;
    let id = share_with_bob(&app, &alice, &[&photos]).await;

    let content = app
        .get(&format!("/api/v1/shared/{id}/files/{secret}/content"), &bob)
        .await;
    let copied = copy(&app, &bob, &id, json!({ "node_ids": [secret] })).await;

    assert_eq!(content.status, StatusCode::NOT_FOUND);
    assert_eq!(copied.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_ticket_downloads_a_shared_file_without_sign_in(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let report = file(&app, &alice, "report.txt", None, "text").await;
    let id = share_with_bob(&app, &alice, &[&report]).await;
    let ticket = app
        .json(
            Method::POST,
            "/api/v1/tickets",
            Some(&bob),
            json!({ "scope": "shared_file", "share_id": id, "node_id": report }),
        )
        .await;
    let ticket = ticket.json()["ticket"].as_str().expect("ticket").to_owned();

    let response = app
        .send(
            Method::GET,
            &format!("/api/v1/shared/{id}/files/{report}/content?ticket={ticket}"),
            None,
            Body::empty(),
            None,
        )
        .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, b"text");
}

#[sqlx::test(migrations = "../../migrations")]
async fn no_ticket_is_given_for_a_share_that_is_not_for_the_user(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    app.user_token("bob").await;
    let carol = app.user_token("carol").await;
    let report = file(&app, &alice, "report.txt", None, "text").await;
    let id = share_with_bob(&app, &alice, &[&report]).await;

    let response = app
        .json(
            Method::POST,
            "/api/v1/tickets",
            Some(&carol),
            json!({ "scope": "shared_archive", "share_id": id }),
        )
        .await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_copy_of_a_shared_folder_lands_in_the_readers_files(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let trip = folder(&app, &alice, "Trip", Some(&photos)).await;
    file(&app, &alice, "beach.jpg", Some(&trip), "sand").await;
    let id = share_with_bob(&app, &alice, &[&photos]).await;

    let response = copy(&app, &bob, &id, json!({ "node_ids": [photos] })).await;

    assert_eq!(response.status, StatusCode::OK);
    let top = response.json();
    assert_eq!(names(&top), ["Photos"]);
    let mine = top[0]["id"].as_str().expect("id").to_owned();
    assert_ne!(mine, photos);
    let inside = app
        .get(&format!("/api/v1/nodes?parent_id={mine}"), &bob)
        .await;
    assert_eq!(names(&inside.json()), ["Trip"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_copied_file_keeps_its_content_and_both_names(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    file(&app, &bob, "report.txt", None, "mine").await;
    let report = file(&app, &alice, "report.txt", None, "hers").await;
    let id = share_with_bob(&app, &alice, &[&report]).await;

    let response = copy(&app, &bob, &id, json!({ "node_ids": [report] })).await;

    let copied = response.json()[0].clone();
    assert_ne!(copied["name"], "report.txt");
    let content = app
        .get(
            &format!(
                "/api/v1/files/{}/content",
                copied["id"].as_str().expect("id")
            ),
            &bob,
        )
        .await;
    assert_eq!(content.body, b"hers");
    let original = app
        .get(&format!("/api/v1/files/{report}/content"), &alice)
        .await;
    assert_eq!(original.body, b"hers");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_share_is_gone(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let report = file(&app, &alice, "report.txt", None, "text").await;
    let id = share_with_bob(&app, &alice, &[&report]).await;
    sqlx::query("UPDATE shares SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid")
        .bind(&id)
        .execute(&pool)
        .await
        .expect("expire");

    let listing = app.get("/api/v1/shared", &bob).await;
    let content = app
        .get(&format!("/api/v1/shared/{id}/files/{report}/content"), &bob)
        .await;

    assert_eq!(listing.json(), json!([]));
    assert_eq!(content.status, StatusCode::GONE);
}
