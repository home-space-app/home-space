//! Read-only share links: the owner's `/shares` and what anyone with a link can read through
//! `/public/shares/{token}`.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use common::{Response, TestApp, setup};

const SHARE_PASSWORD: &str = "open-sesame";

fn id_of(response: &Response) -> String {
    response.json()["id"].as_str().expect("id").to_owned()
}

fn rfc3339(time: OffsetDateTime) -> String {
    time.format(&Rfc3339).expect("format")
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

async fn share(app: &TestApp, token: &str, body: Value) -> Response {
    app.json(Method::POST, "/api/v1/shares", Some(token), body)
        .await
}

/// Shares `node_ids` without password or expiry, returning the link's token and id.
async fn share_plain(app: &TestApp, token: &str, node_ids: &[&str]) -> (String, String) {
    let response = share(app, token, json!({ "node_ids": node_ids })).await;
    assert_eq!(response.status, StatusCode::CREATED);
    let body = response.json();
    (
        body["token"].as_str().expect("token").to_owned(),
        body["id"].as_str().expect("id").to_owned(),
    )
}

async fn public(app: &TestApp, uri: &str, extra: &[(&str, &str)]) -> Response {
    app.send_with(Method::GET, uri, None, Body::empty(), None, extra)
        .await
}

async fn unlock(app: &TestApp, token: &str, password: &str) -> Response {
    app.json(
        Method::POST,
        &format!("/api/v1/public/shares/{token}/unlock"),
        None,
        json!({ "password": password }),
    )
    .await
}

fn names(body: &Value) -> Vec<&str> {
    body.as_array()
        .expect("array")
        .iter()
        .map(|node| node["name"].as_str().expect("name"))
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_link_shares_several_items(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let report = file(&app, &alice, "report.pdf", None, "pdf").await;

    let response = share(
        &app,
        &alice,
        json!({ "node_ids": [photos, report, photos] }),
    )
    .await;

    assert_eq!(response.status, StatusCode::CREATED);
    let body = response.json();
    assert_eq!(names(&body["items"]), vec!["Photos", "report.pdf"]);
    assert_eq!(body["has_password"], false);
    assert_eq!(body["permission"], "read");
    assert!(body["expires_at"].is_null());
    let token = body["token"].as_str().expect("token");
    assert!(token.len() >= 43);
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_link_needs_at_least_one_item(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;

    let response = share(&app, &alice, json!({ "node_ids": [] })).await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_users_file_cannot_be_shared(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let theirs = file(&app, &bob, "secret.txt", None, "bob's").await;

    let response = share(&app, &alice, json!({ "node_ids": [theirs] })).await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_expiry_in_the_past_is_refused(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.pdf", None, "pdf").await;
    let past = rfc3339(OffsetDateTime::now_utc() - Duration::hours(1));

    let response = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "expires_at": past }),
    )
    .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_owner_finds_the_links_of_a_node(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.pdf", None, "pdf").await;
    let other = file(&app, &alice, "other.txt", None, "other").await;
    let (_, id) = share_plain(&app, &alice, &[&report]).await;
    share_plain(&app, &alice, &[&other]).await;

    let response = app
        .get(&format!("/api/v1/shares?node_id={report}"), &alice)
        .await;

    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    let shares = body.as_array().expect("array");
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0]["id"], id.as_str());
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_users_do_not_see_or_change_a_link(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let report = file(&app, &alice, "report.pdf", None, "pdf").await;
    let (_, id) = share_plain(&app, &alice, &[&report]).await;

    let listed = app.get("/api/v1/shares", &bob).await;
    let updated = app
        .json(
            Method::PATCH,
            &format!("/api/v1/shares/{id}"),
            Some(&bob),
            json!({ "expires_at": null }),
        )
        .await;
    let deleted = app
        .send(
            Method::DELETE,
            &format!("/api/v1/shares/{id}"),
            Some(&bob),
            Body::empty(),
            None,
        )
        .await;

    assert_eq!(listed.json(), json!([]));
    assert_eq!(updated.status, StatusCode::NOT_FOUND);
    assert_eq!(deleted.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn anyone_with_the_link_sees_the_shared_items(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let report = file(&app, &alice, "report.pdf", None, "pdf").await;
    file(&app, &alice, "private.txt", None, "mine").await;
    let (token, _) = share_plain(&app, &alice, &[&photos, &report]).await;

    let response = public(&app, &format!("/api/v1/public/shares/{token}"), &[]).await;

    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    assert_eq!(body["owner"], "alice");
    assert_eq!(body["item_count"], 2);
    assert_eq!(body["needs_password"], false);
    assert_eq!(names(&body["items"]), vec!["Photos", "report.pdf"]);
    assert!(body["items"][0].get("parent_id").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_link_is_not_found(pool: PgPool) {
    let app = setup(pool).await;

    let response = public(
        &app,
        "/api/v1/public/shares/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        &[],
    )
    .await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_shared_folder_can_be_browsed(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let trip = folder(&app, &alice, "Trip", Some(&photos)).await;
    file(&app, &alice, "beach.jpg", Some(&trip), "sand").await;
    let (token, _) = share_plain(&app, &alice, &[&photos]).await;

    let top = public(
        &app,
        &format!("/api/v1/public/shares/{token}/nodes?parent_id={photos}"),
        &[],
    )
    .await;
    let inner = public(
        &app,
        &format!("/api/v1/public/shares/{token}/nodes?parent_id={trip}"),
        &[],
    )
    .await;

    assert_eq!(names(&top.json()), vec!["Trip"]);
    assert_eq!(names(&inner.json()), vec!["beach.jpg"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_file_in_a_shared_folder_can_be_downloaded(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let beach = file(&app, &alice, "beach.jpg", Some(&photos), "sand").await;
    let (token, _) = share_plain(&app, &alice, &[&photos]).await;

    let response = public(
        &app,
        &format!("/api/v1/public/shares/{token}/files/{beach}/content"),
        &[],
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, b"sand");
    assert_eq!(
        response.headers.get("accept-ranges").expect("ranges"),
        "bytes"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_download_resumes_from_a_range(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "0123456789").await;
    let (token, _) = share_plain(&app, &alice, &[&report]).await;

    let response = public(
        &app,
        &format!("/api/v1/public/shares/{token}/files/{report}/content"),
        &[("range", "bytes=4-")],
    )
    .await;

    assert_eq!(response.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.body, b"456789");
    assert_eq!(
        response.headers.get("content-range").expect("range"),
        "bytes 4-9/10"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn nothing_outside_the_shared_items_is_reachable(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let home = folder(&app, &alice, "Home", None).await;
    let photos = folder(&app, &alice, "Photos", Some(&home)).await;
    let sibling = file(&app, &alice, "taxes.pdf", Some(&home), "taxes").await;
    let top = file(&app, &alice, "diary.txt", None, "diary").await;
    let bobs = file(&app, &bob, "bob.txt", None, "bob").await;
    let (token, _) = share_plain(&app, &alice, &[&photos]).await;

    for id in [&sibling, &top, &bobs] {
        let content = public(
            &app,
            &format!("/api/v1/public/shares/{token}/files/{id}/content"),
            &[],
        )
        .await;
        assert_eq!(content.status, StatusCode::NOT_FOUND, "content of {id}");
    }
    let parent = public(
        &app,
        &format!("/api/v1/public/shares/{token}/nodes?parent_id={home}"),
        &[],
    )
    .await;
    assert_eq!(parent.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_shared_folder_cannot_be_downloaded_as_a_file(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let (token, _) = share_plain(&app, &alice, &[&photos]).await;

    let response = public(
        &app,
        &format!("/api/v1/public/shares/{token}/files/{photos}/content"),
        &[],
    )
    .await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_binned_item_is_no_longer_shared(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let beach = file(&app, &alice, "beach.jpg", Some(&photos), "sand").await;
    let report = file(&app, &alice, "report.pdf", None, "pdf").await;
    let (token, _) = share_plain(&app, &alice, &[&photos, &report]).await;

    let binned = app
        .send(
            Method::DELETE,
            &format!("/api/v1/nodes/{photos}"),
            Some(&alice),
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(binned.status, StatusCode::NO_CONTENT);

    let listing = public(&app, &format!("/api/v1/public/shares/{token}"), &[]).await;
    let content = public(
        &app,
        &format!("/api/v1/public/shares/{token}/files/{beach}/content"),
        &[],
    )
    .await;
    assert_eq!(names(&listing.json()["items"]), vec!["report.pdf"]);
    assert_eq!(listing.json()["item_count"], 1);
    assert_eq!(content.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_hides_the_items_until_it_is_given(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    assert_eq!(created.json()["has_password"], true);
    let token = created.json()["token"].as_str().expect("token").to_owned();

    let locked = public(&app, &format!("/api/v1/public/shares/{token}"), &[]).await;
    let content = public(
        &app,
        &format!("/api/v1/public/shares/{token}/files/{report}/content"),
        &[],
    )
    .await;

    assert_eq!(locked.status, StatusCode::OK);
    assert_eq!(locked.json()["needs_password"], true);
    assert_eq!(locked.json()["item_count"], 1);
    assert!(locked.json().get("items").is_none());
    assert_eq!(content.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_right_password_unlocks_the_link(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let token = created.json()["token"].as_str().expect("token").to_owned();

    let unlocked = unlock(&app, &token, SHARE_PASSWORD).await;
    assert_eq!(unlocked.status, StatusCode::OK);
    let access = unlocked.json()["access"]
        .as_str()
        .expect("access")
        .to_owned();

    let listing = public(
        &app,
        &format!("/api/v1/public/shares/{token}"),
        &[("share-access", &access)],
    )
    .await;
    let content = public(
        &app,
        &format!("/api/v1/public/shares/{token}/files/{report}/content?access={access}"),
        &[],
    )
    .await;
    assert_eq!(listing.json()["needs_password"], false);
    assert_eq!(names(&listing.json()["items"]), vec!["report.txt"]);
    assert_eq!(content.status, StatusCode::OK);
    assert_eq!(content.body, b"pdf");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_wrong_password_is_refused(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let token = created.json()["token"].as_str().expect("token").to_owned();

    let response = unlock(&app, &token, "not-the-password").await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn wrong_passwords_are_throttled(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let token = created.json()["token"].as_str().expect("token").to_owned();
    for _ in 0..3 {
        unlock(&app, &token, "not-the-password").await;
    }

    let response = unlock(&app, &token, SHARE_PASSWORD).await;

    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_new_password_ends_the_access_given_for_the_old_one(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let token = created.json()["token"].as_str().expect("token").to_owned();
    let id = id_of(&created);
    let access = unlock(&app, &token, SHARE_PASSWORD).await.json()["access"]
        .as_str()
        .expect("access")
        .to_owned();

    let updated = app
        .json(
            Method::PATCH,
            &format!("/api/v1/shares/{id}"),
            Some(&alice),
            json!({ "password": "another-password" }),
        )
        .await;
    assert_eq!(updated.status, StatusCode::OK);

    let listing = public(
        &app,
        &format!("/api/v1/public/shares/{token}"),
        &[("share-access", &access)],
    )
    .await;
    assert_eq!(listing.json()["needs_password"], true);
}

#[sqlx::test(migrations = "../../migrations")]
async fn removing_the_password_opens_the_link(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let token = created.json()["token"].as_str().expect("token").to_owned();
    let id = id_of(&created);

    let updated = app
        .json(
            Method::PATCH,
            &format!("/api/v1/shares/{id}"),
            Some(&alice),
            json!({ "password": null }),
        )
        .await;

    assert_eq!(updated.json()["has_password"], false);
    let listing = public(&app, &format!("/api/v1/public/shares/{token}"), &[]).await;
    assert_eq!(listing.json()["needs_password"], false);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_access_token_of_another_link_does_not_unlock(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let first = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let second = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let first_token = first.json()["token"].as_str().expect("token").to_owned();
    let second_token = second.json()["token"].as_str().expect("token").to_owned();
    let access = unlock(&app, &first_token, SHARE_PASSWORD).await.json()["access"]
        .as_str()
        .expect("access")
        .to_owned();

    let listing = public(
        &app,
        &format!("/api/v1/public/shares/{second_token}"),
        &[("share-access", &access)],
    )
    .await;

    assert_eq!(listing.json()["needs_password"], true);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_users_access_token_does_not_unlock_a_link(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let token = created.json()["token"].as_str().expect("token").to_owned();

    let listing = public(
        &app,
        &format!("/api/v1/public/shares/{token}"),
        &[("share-access", &alice)],
    )
    .await;

    assert_eq!(listing.json()["needs_password"], true);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_link_is_gone(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let soon = rfc3339(OffsetDateTime::now_utc() + Duration::hours(1));
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "expires_at": soon }),
    )
    .await;
    let token = created.json()["token"].as_str().expect("token").to_owned();
    let id = id_of(&created);
    sqlx::query("UPDATE shares SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid")
        .bind(&id)
        .execute(&pool)
        .await
        .expect("expire");

    let listing = public(&app, &format!("/api/v1/public/shares/{token}"), &[]).await;
    let content = public(
        &app,
        &format!("/api/v1/public/shares/{token}/files/{report}/content"),
        &[],
    )
    .await;

    assert_eq!(listing.status, StatusCode::GONE);
    assert_eq!(content.status, StatusCode::GONE);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_link_can_be_made_to_never_expire(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let soon = rfc3339(OffsetDateTime::now_utc() + Duration::days(7));
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "expires_at": soon }),
    )
    .await;
    assert!(created.json()["expires_at"].is_string());
    let id = id_of(&created);

    let updated = app
        .json(
            Method::PATCH,
            &format!("/api/v1/shares/{id}"),
            Some(&alice),
            json!({ "expires_at": null }),
        )
        .await;

    assert_eq!(updated.status, StatusCode::OK);
    assert!(updated.json()["expires_at"].is_null());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_stopped_link_no_longer_works(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let (token, id) = share_plain(&app, &alice, &[&report]).await;

    let deleted = app
        .send(
            Method::DELETE,
            &format!("/api/v1/shares/{id}"),
            Some(&alice),
            Body::empty(),
            None,
        )
        .await;

    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let listing = public(&app, &format!("/api/v1/public/shares/{token}"), &[]).await;
    assert_eq!(listing.status, StatusCode::NOT_FOUND);
}

fn zip_names(response: &Response) -> Vec<String> {
    let zip = zip::ZipArchive::new(std::io::Cursor::new(response.body.clone())).expect("zip");
    zip.file_names()
        .map(|name| name.expect("name").into_owned())
        .collect()
}

fn zip_text(response: &Response, name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(response.body.clone())).expect("zip");
    let mut text = String::new();
    std::io::Read::read_to_string(&mut zip.by_name(name).expect("entry"), &mut text).expect("read");
    text
}

fn disposition(response: &Response) -> &str {
    response.headers["content-disposition"]
        .to_str()
        .expect("disposition")
}

#[sqlx::test(migrations = "../../migrations")]
async fn everything_a_link_shares_downloads_as_one_zip(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let trip = folder(&app, &alice, "Trip", Some(&photos)).await;
    file(&app, &alice, "beach.jpg", Some(&trip), "sand").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let (token, _) = share_plain(&app, &alice, &[&photos, &report]).await;

    let response = public(&app, &format!("/api/v1/public/shares/{token}/archive"), &[]).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers["content-type"], "application/zip");
    assert!(disposition(&response).ends_with("Shared%20by%20alice.zip"));
    let mut names = zip_names(&response);
    names.sort();
    assert_eq!(
        names,
        [
            "Photos/",
            "Photos/Trip/",
            "Photos/Trip/beach.jpg",
            "report.txt"
        ]
    );
    assert_eq!(zip_text(&response, "Photos/Trip/beach.jpg"), "sand");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_folder_inside_a_link_downloads_on_its_own(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos", None).await;
    let trip = folder(&app, &alice, "Trip", Some(&photos)).await;
    file(&app, &alice, "beach.jpg", Some(&trip), "sand").await;
    file(&app, &alice, "cover.jpg", Some(&photos), "cover").await;
    let (token, _) = share_plain(&app, &alice, &[&photos]).await;

    let response = public(
        &app,
        &format!("/api/v1/public/shares/{token}/archive?folder_id={trip}"),
        &[],
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert!(disposition(&response).ends_with("Trip.zip"));
    assert_eq!(zip_names(&response), ["Trip/", "Trip/beach.jpg"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn shared_items_with_the_same_name_are_numbered_in_the_zip(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let work = folder(&app, &alice, "Work", None).await;
    let home = folder(&app, &alice, "Home", None).await;
    let first = file(&app, &alice, "report.txt", Some(&home), "home").await;
    let second = file(&app, &alice, "report.txt", Some(&work), "work").await;
    let (token, _) = share_plain(&app, &alice, &[&first, &second]).await;

    let response = public(&app, &format!("/api/v1/public/shares/{token}/archive"), &[]).await;

    assert_eq!(response.status, StatusCode::OK);
    let mut names = zip_names(&response);
    names.sort();
    assert_eq!(names, ["report (2).txt", "report.txt"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_folder_outside_the_link_cannot_be_zipped(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let home = folder(&app, &alice, "Home", None).await;
    let photos = folder(&app, &alice, "Photos", Some(&home)).await;
    let report = file(&app, &alice, "report.txt", Some(&photos), "pdf").await;
    let (token, _) = share_plain(&app, &alice, &[&photos]).await;

    for id in [&home, &report] {
        let response = public(
            &app,
            &format!("/api/v1/public/shares/{token}/archive?folder_id={id}"),
            &[],
        )
        .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "zip of {id}");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_zip_of_a_locked_link_needs_its_access_token(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let report = file(&app, &alice, "report.txt", None, "pdf").await;
    let created = share(
        &app,
        &alice,
        json!({ "node_ids": [report], "password": SHARE_PASSWORD }),
    )
    .await;
    let token = created.json()["token"].as_str().expect("token").to_owned();
    let archive = format!("/api/v1/public/shares/{token}/archive");

    let locked = public(&app, &archive, &[]).await;
    let access = unlock(&app, &token, SHARE_PASSWORD).await.json()["access"]
        .as_str()
        .expect("access")
        .to_owned();
    let unlocked = public(&app, &format!("{archive}?access={access}"), &[]).await;

    assert_eq!(locked.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unlocked.status, StatusCode::OK);
    assert!(disposition(&unlocked).ends_with("report.txt.zip"));
    assert_eq!(zip_names(&unlocked), ["report.txt"]);
}
