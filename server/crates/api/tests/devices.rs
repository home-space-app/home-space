//! Clients name the device they sign in from; the user sees their devices and can sign one
//! out, which also cancels the uploads it left unfinished.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::{TestApp, setup};

const DEVICES: &str = "/api/v1/devices";

async fn devices(app: &TestApp, token: &str) -> Vec<Value> {
    let response = app.get(DEVICES, token).await;
    assert_eq!(response.status, StatusCode::OK);
    response.json().as_array().expect("devices").clone()
}

async fn start_upload(app: &TestApp, token: &str) -> String {
    let created = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(token),
            json!({ "name": "big.bin", "size": 10 }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    created.json()["id"].as_str().expect("id").to_owned()
}

async fn delete(app: &TestApp, uri: &str, token: &str) -> StatusCode {
    app.send(Method::DELETE, uri, Some(token), Body::empty(), None)
        .await
        .status
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_device_list_marks_the_current_one(pool: PgPool) {
    let app = setup(pool).await;
    let (laptop, phone) = (Uuid::now_v7(), Uuid::now_v7());
    app.sign_in_from("alice", laptop, "Laptop").await;
    let token = app.sign_in_from("alice", phone, "Phone").await;

    let listed = devices(&app, &token).await;

    let current = listed
        .iter()
        .map(|device| (device["name"].as_str(), device["current"].as_bool()))
        .collect::<Vec<_>>();
    assert_eq!(current.len(), 2);
    assert!(current.contains(&(Some("Phone"), Some(true))));
    assert!(current.contains(&(Some("Laptop"), Some(false))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn signing_out_a_device_ends_its_sessions_and_cancels_its_uploads(pool: PgPool) {
    let app = setup(pool).await;
    let (laptop, phone) = (Uuid::now_v7(), Uuid::now_v7());
    let on_laptop = app.sign_in_from("alice", laptop, "Laptop").await;
    let on_phone = app.sign_in_from("alice", phone, "Phone").await;
    let upload = start_upload(&app, &on_laptop).await;
    let listed = devices(&app, &on_phone).await;
    assert!(
        listed
            .iter()
            .any(|device| device["id"] == json!(laptop) && device["uploads"] == 1)
    );

    let status = delete(&app, &format!("{DEVICES}/{laptop}"), &on_phone).await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.get("/api/v1/me", &on_laptop).await.status,
        StatusCode::UNAUTHORIZED
    );
    let status = app
        .get(&format!("/api/v1/uploads/{upload}"), &on_phone)
        .await
        .status;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let laptop_row = devices(&app, &on_phone)
        .await
        .into_iter()
        .find(|device| device["id"] == json!(laptop))
        .expect("laptop stays listed");
    assert_eq!(laptop_row["signed_in"], false);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_can_be_renamed_but_not_to_a_blank_name(pool: PgPool) {
    let app = setup(pool).await;
    let laptop = Uuid::now_v7();
    let token = app.sign_in_from("alice", laptop, "Laptop").await;
    let uri = format!("{DEVICES}/{laptop}");

    let blank = app
        .json(Method::PATCH, &uri, Some(&token), json!({ "name": "  " }))
        .await;
    let renamed = app
        .json(Method::PATCH, &uri, Some(&token), json!({ "name": "Work" }))
        .await;

    assert_eq!(blank.status, StatusCode::BAD_REQUEST);
    assert_eq!(renamed.status, StatusCode::NO_CONTENT);
    assert_eq!(devices(&app, &token).await[0]["name"], "Work");
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_users_device_is_not_found(pool: PgPool) {
    let app = setup(pool).await;
    let laptop = Uuid::now_v7();
    app.sign_in_from("alice", laptop, "Laptop").await;
    let bob = app.sign_in_from("bob", Uuid::now_v7(), "Bob's").await;

    let status = delete(&app, &format!("{DEVICES}/{laptop}"), &bob).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_lists_only_its_own_uploads_when_asked(pool: PgPool) {
    let app = setup(pool).await;
    let on_laptop = app.sign_in_from("alice", Uuid::now_v7(), "Laptop").await;
    let on_phone = app.sign_in_from("alice", Uuid::now_v7(), "Phone").await;
    let from_laptop = start_upload(&app, &on_laptop).await;
    start_upload(&app, &on_phone).await;

    let all = app.get("/api/v1/uploads", &on_laptop).await.json();
    let own = app
        .get("/api/v1/uploads?this_device=true", &on_laptop)
        .await
        .json();

    assert_eq!(all.as_array().map(Vec::len), Some(2));
    let own = own.as_array().expect("uploads");
    assert_eq!(own.len(), 1);
    assert_eq!(own[0]["id"], from_laptop);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_client_that_names_no_device_still_signs_in(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    assert!(devices(&app, &token).await.is_empty());
    let own = app.get("/api/v1/uploads?this_device=true", &token).await;
    assert_eq!(own.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_change_feed_names_the_device_a_change_came_from(pool: PgPool) {
    let app = setup(pool).await;
    let laptop = Uuid::now_v7();
    let token = app.sign_in_from("alice", laptop, "Laptop").await;
    let start = app.get("/api/v1/changes", &token).await.json()["cursor"].clone();
    let created = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            json!({ "name": "docs" }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);

    let changes = app
        .get(&format!("/api/v1/changes?since={start}"), &token)
        .await
        .json();

    let change = &changes["changes"][0];
    assert_eq!(change["node"]["id"], created.json()["id"]);
    assert_eq!(change["device_id"], json!(laptop));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_node_names_the_device_it_was_made_on(pool: PgPool) {
    let app = setup(pool).await;
    let laptop = Uuid::now_v7();
    let token = app.sign_in_from("alice", laptop, "Laptop").await;
    let created = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            json!({ "name": "docs" }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.json()["id"].clone();

    let node = app
        .get(
            &format!("/api/v1/nodes/{}", id.as_str().unwrap_or_default()),
            &token,
        )
        .await
        .json();

    assert_eq!(created.json()["edited_on"], json!(laptop));
    assert_eq!(node["edited_on"], json!(laptop));
}
