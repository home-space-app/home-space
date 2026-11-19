mod common;

use axum::http::{Method, StatusCode};
use sqlx::PgPool;

use common::setup;

fn cursor_of(response: &common::Response) -> i64 {
    response.json()["cursor"].as_i64().expect("cursor")
}

#[sqlx::test(migrations = "../../migrations")]
async fn changes_since_a_cursor_list_uploads_and_deletes(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let start = cursor_of(&app.get("/api/v1/changes", &token).await);
    let kept = app.upload(&token, "name=kept.txt", "a").await.json();
    let gone = app.upload(&token, "name=gone.txt", "b").await.json();
    let gone_id = gone["id"].as_str().expect("id");
    let deleted = app
        .send(
            Method::DELETE,
            &format!("/api/v1/nodes/{gone_id}"),
            Some(&token),
            axum::body::Body::empty(),
            None,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);

    let response = app
        .get(&format!("/api/v1/changes?since={start}"), &token)
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    let changes = body["changes"].as_array().expect("changes");
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["type"], "upserted");
    assert_eq!(changes[0]["node"]["id"], kept["id"]);
    assert_eq!(changes[1]["type"], "deleted");
    assert_eq!(changes[1]["id"], gone["id"]);
    assert_eq!(body["more"], false);
    assert_eq!(body["cursor"], changes[1]["seq"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn changes_before_the_pruned_journal_are_gone(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let start = cursor_of(&app.get("/api/v1/changes", &token).await);
    app.upload(&token, "name=a.txt", "a").await;
    app.db
        .prune_changes(time::OffsetDateTime::now_utc() + time::Duration::seconds(1))
        .await
        .expect("prune");

    let response = app
        .get(&format!("/api/v1/changes?since={start}"), &token)
        .await;
    assert_eq!(response.status, StatusCode::GONE);
    assert_eq!(response.json()["error"], "gone");
}

#[sqlx::test(migrations = "../../migrations")]
async fn upload_with_an_older_modification_time_keeps_the_current_content(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let file = app
        .upload(&token, "name=a.txt&modified_at=2026-01-02T00:00:00Z", "new")
        .await
        .json();
    assert_eq!(file["updated_at"], "2026-01-02T00:00:00Z");
    let older = app
        .upload(&token, "name=a.txt&modified_at=2026-01-01T00:00:00Z", "old")
        .await;
    assert_eq!(older.status, StatusCode::OK);
    assert_eq!(older.json()["version"], 2);
    let id = file["id"].as_str().expect("id");
    let content = app
        .get(&format!("/api/v1/files/{id}/content"), &token)
        .await;
    assert_eq!(content.body, b"new");
    let version = app
        .get(&format!("/api/v1/files/{id}/versions/1/content"), &token)
        .await;
    assert_eq!(version.body, b"old");
}
