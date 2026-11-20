mod common;

use axum::{body::Body, http::Method};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::{TestApp, setup};

fn id_of(response: &common::Response) -> String {
    response.json()["id"].as_str().expect("id").to_owned()
}

async fn folder(app: &TestApp, token: &str, name: &str) -> String {
    let response = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(token),
            json!({ "name": name }),
        )
        .await;
    id_of(&response)
}

async fn report(app: &TestApp, token: &str) -> Value {
    let response = app.get("/api/v1/duplicates", token).await;
    assert_eq!(response.status, 200);
    response.json()
}

fn names(files: &Value) -> Vec<&str> {
    files
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["node"]["name"].as_str().expect("name"))
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn groups_live_copies_with_their_locations(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let photos = folder(&app, &alice, "Photos").await;
    app.upload(&alice, "name=a.txt", "same").await;
    app.upload(&alice, &format!("name=copy.txt&parent_id={photos}"), "same")
        .await;
    app.upload(&alice, "name=unique.txt", "other").await;
    app.upload(&alice, "name=empty-1.txt", "").await;
    app.upload(&alice, "name=empty-2.txt", "").await;

    let report = report(&app, &alice).await;

    let groups = report["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["size"], 4);
    assert_eq!(names(&groups[0]["files"]), vec!["a.txt", "copy.txt"]);
    assert_eq!(groups[0]["files"][0]["location"], json!([]));
    assert_eq!(groups[0]["files"][1]["location"], json!(["Photos"]));
    assert_eq!(report["outdated"], json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn binned_copies_and_other_users_are_left_out(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    app.upload(&alice, "name=a.txt", "same").await;
    let copy = id_of(&app.upload(&alice, "name=b.txt", "same").await);
    app.upload(&bob, "name=a.txt", "same").await;

    assert_eq!(
        report(&app, &alice).await["groups"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(report(&app, &bob).await["groups"], json!([]));

    app.send(
        Method::DELETE,
        &format!("/api/v1/nodes/{copy}"),
        Some(&alice),
        Body::empty(),
        None,
    )
    .await;

    assert_eq!(report(&app, &alice).await["groups"], json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_file_holding_an_old_version_of_another_is_outdated(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let original = id_of(&app.upload(&alice, "name=report.txt", "draft").await);
    app.upload(&alice, "name=report-copy.txt", "draft").await;
    app.send(
        Method::PUT,
        &format!("/api/v1/files/{original}/content"),
        Some(&alice),
        Body::from("final"),
        None,
    )
    .await;

    let report = report(&app, &alice).await;

    assert_eq!(report["groups"], json!([]));
    let outdated = report["outdated"].as_array().expect("outdated");
    assert_eq!(outdated.len(), 1);
    assert_eq!(outdated[0]["file"]["node"]["name"], "report-copy.txt");
    assert_eq!(outdated[0]["source"]["node"]["name"], "report.txt");
    assert_eq!(outdated[0]["version"], 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn requires_sign_in(pool: PgPool) {
    let app = setup(pool).await;
    let response = app
        .send(Method::GET, "/api/v1/duplicates", None, Body::empty(), None)
        .await;
    assert_eq!(response.status, 401);
}
