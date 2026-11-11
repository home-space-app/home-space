//! Account management by administrators: creating, disabling, quotas, passwords and deletion.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::{PASSWORD, TestApp, setup};

const USERS: &str = "/api/v1/admin/users";
const LOGIN: &str = "/api/v1/auth/login";
const ME: &str = "/api/v1/me";
const NEW_PASSWORD: &str = "another-password-456";

/// Signs in `username` as an administrator, returning the access token.
async fn admin(app: &TestApp, username: &str) -> String {
    let token = app.user_token(username).await;
    let id = app.user_id(username).await;
    app.state.db.set_admin(id, true).await.expect("admin");
    token
}

async fn login(app: &TestApp, username: &str, password: &str) -> StatusCode {
    app.json(
        Method::POST,
        LOGIN,
        None,
        json!({ "username": username, "password": password }),
    )
    .await
    .status
}

async fn patch(app: &TestApp, token: &str, id: db::UserId, body: Value) -> (StatusCode, Value) {
    let response = app
        .json(Method::PATCH, &format!("{USERS}/{id}"), Some(token), body)
        .await;
    (response.status, response.json())
}

async fn delete(app: &TestApp, token: &str, uri: &str) -> (StatusCode, Value) {
    let response = app
        .send(Method::DELETE, uri, Some(token), Body::empty(), None)
        .await;
    (response.status, response.json())
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_administrators_manage_accounts(pool: PgPool) {
    let app = setup(pool).await;
    let member = app.user_token("bob").await;
    assert_eq!(app.get(USERS, &member).await.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_administrator_creates_an_account_with_a_quota(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;

    let created = app
        .json(
            Method::POST,
            USERS,
            Some(&token),
            json!({
                "username": " anna ",
                "password": PASSWORD,
                "is_admin": false,
                "quota_bytes": 50_000_000_000u64,
            }),
        )
        .await;

    assert_eq!(created.status, StatusCode::CREATED);
    let body = created.json();
    assert_eq!(body["username"], "anna");
    assert_eq!(body["quota_bytes"], 50_000_000_000u64);
    assert_eq!(body["disabled"], false);
    assert_eq!(body["delete_at"], Value::Null);
    assert_eq!(login(&app, "anna", PASSWORD).await, StatusCode::OK);

    let list = app.get(USERS, &token).await.json();
    let names: Vec<_> = list
        .as_array()
        .expect("list")
        .iter()
        .map(|user| user["username"].clone())
        .collect();
    assert_eq!(names, ["anna", "sam"]);

    let taken = app
        .json(
            Method::POST,
            USERS,
            Some(&token),
            json!({ "username": "anna", "password": PASSWORD }),
        )
        .await;
    assert_eq!(taken.status, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_quota_can_be_changed_and_removed(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;
    app.user_token("bob").await;
    let bob = app.user_id("bob").await;

    let (status, body) = patch(&app, &token, bob, json!({ "quota_bytes": 1024 })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["quota_bytes"], 1024);

    // Leaving it out keeps it.
    let (_, body) = patch(&app, &token, bob, json!({ "is_admin": true })).await;
    assert_eq!(body["quota_bytes"], 1024);
    assert_eq!(body["is_admin"], true);

    let (_, body) = patch(&app, &token, bob, json!({ "quota_bytes": null })).await;
    assert_eq!(body["quota_bytes"], Value::Null);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_disabled_user_is_signed_out_and_cannot_sign_in(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;
    let guest = app.user_token("guest").await;
    let id = app.user_id("guest").await;

    let (status, body) = patch(&app, &token, id, json!({ "disabled": true })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["disabled"], true);

    assert_eq!(app.get(ME, &guest).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "guest", PASSWORD).await, StatusCode::FORBIDDEN);
    // A wrong password still reads as a wrong password, not as a disabled account.
    assert_eq!(
        login(&app, "guest", "wrong").await,
        StatusCode::UNAUTHORIZED
    );

    patch(&app, &token, id, json!({ "disabled": false })).await;
    assert_eq!(login(&app, "guest", PASSWORD).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn administrators_cannot_lock_themselves_out(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;
    let sam = app.user_id("sam").await;

    for change in [json!({ "disabled": true }), json!({ "is_admin": false })] {
        let (status, _) = patch(&app, &token, sam, change).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _) = delete(&app, &token, &format!("{USERS}/{sam}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_administrator_sets_a_password_and_signs_the_user_out(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;
    let peter = app.user_token("peter").await;
    let id = app.user_id("peter").await;

    let response = app
        .json(
            Method::PUT,
            &format!("{USERS}/{id}/password"),
            Some(&token),
            json!({ "password": NEW_PASSWORD }),
        )
        .await;

    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(app.get(ME, &peter).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        login(&app, "peter", PASSWORD).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(login(&app, "peter", NEW_PASSWORD).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_less_user_gets_no_password(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;
    let user = app
        .state
        .db
        .create_user_with_identity("maria", "https://idp.example", "sub", None)
        .await
        .expect("user");

    let response = app
        .json(
            Method::PUT,
            &format!("{USERS}/{}/password", user.id),
            Some(&token),
            json!({ "password": NEW_PASSWORD }),
        )
        .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    let found = app
        .state
        .db
        .find_user(user.id)
        .await
        .expect("db")
        .expect("user");
    assert!(found.password_hash.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_deleted_user_is_marked_and_can_be_brought_back(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;
    let peter = app.user_token("peter").await;
    let id = app.user_id("peter").await;

    let (status, body) = delete(&app, &token, &format!("{USERS}/{id}")).await;

    assert_eq!(status, StatusCode::OK);
    let delete_at = time::OffsetDateTime::parse(
        body["delete_at"].as_str().expect("delete_at"),
        &time::format_description::well_known::Rfc3339,
    )
    .expect("time");
    let delay = delete_at - time::OffsetDateTime::now_utc();
    assert!(delay > time::Duration::minutes(9) && delay <= time::Duration::minutes(10));
    assert_eq!(app.get(ME, &peter).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "peter", PASSWORD).await, StatusCode::FORBIDDEN);
    // Nothing is due yet, so the job deletes nothing.
    assert!(
        app.state
            .files
            .delete_due_users()
            .await
            .expect("job")
            .is_empty()
    );

    let (status, body) = delete(&app, &token, &format!("{USERS}/{id}/deletion")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delete_at"], Value::Null);
    assert_eq!(login(&app, "peter", PASSWORD).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_deletion_job_removes_a_due_user_and_their_files(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;
    let peter = app.user_token("peter").await;
    let id = app.user_id("peter").await;
    app.upload(&peter, "name=a.txt", "one").await;
    app.upload(&peter, "name=a.txt", "two").await;
    let root = app.state.files.store().root().to_owned();
    let blobs = root.join("blobs").join(id.to_string());
    let versions = root.join("versions").join(id.to_string());
    assert!(blobs.exists() && versions.exists());

    delete(&app, &token, &format!("{USERS}/{id}")).await;
    // Bring the deletion forward so that it is due now.
    app.state.db.cancel_deletion(id).await.expect("cancel");
    app.state
        .db
        .mark_for_deletion(
            id,
            time::OffsetDateTime::now_utc() - time::Duration::seconds(1),
        )
        .await
        .expect("due");

    let deleted = app.state.files.delete_due_users().await.expect("job");

    assert_eq!(deleted, vec![id]);
    assert!(app.state.db.find_user(id).await.expect("db").is_none());
    assert!(!blobs.exists() && !versions.exists());
    let list = app.get(USERS, &token).await.json();
    assert_eq!(list.as_array().expect("list").len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_for_deletion_disables_and_cancelling_enables(pool: PgPool) {
    let app = setup(pool).await;
    let token = admin(&app, "sam").await;
    app.user_token("peter").await;
    let id = app.user_id("peter").await;
    // Disabled before being marked, which cancelling undoes as well.
    patch(&app, &token, id, json!({ "disabled": true })).await;

    let (_, body) = delete(&app, &token, &format!("{USERS}/{id}")).await;
    assert_eq!(body["disabled"], true);
    let (status, _) = patch(&app, &token, id, json!({ "disabled": false })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(login(&app, "peter", PASSWORD).await, StatusCode::FORBIDDEN);

    let (_, body) = delete(&app, &token, &format!("{USERS}/{id}/deletion")).await;
    assert_eq!(body["disabled"], false);
    assert_eq!(body["delete_at"], Value::Null);
    assert_eq!(login(&app, "peter", PASSWORD).await, StatusCode::OK);

    // Cancelling when nothing is marked leaves a disabled user disabled.
    patch(&app, &token, id, json!({ "disabled": true })).await;
    let (_, body) = delete(&app, &token, &format!("{USERS}/{id}/deletion")).await;
    assert_eq!(body["disabled"], true);
}
