//! An app's extensions get sessions of their own on the app's device, which end with it.

mod common;

use axum::{body::Body, http::Method, http::StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::{PASSWORD, TestApp, setup};

struct Pair {
    access: String,
    refresh: String,
}

fn pair(tokens: Value) -> Pair {
    Pair {
        access: tokens["access_token"].as_str().expect("access").to_owned(),
        refresh: tokens["refresh_token"]
            .as_str()
            .expect("refresh")
            .to_owned(),
    }
}

async fn sign_in(app: &TestApp, device: Option<Uuid>) -> Pair {
    app.state.auth.create_user("alice", PASSWORD).await.ok();
    let mut body = json!({ "username": "alice", "password": PASSWORD });
    if let Some(device) = device {
        body["device"] = json!({ "id": device, "name": "Phone", "platform": "ios" });
    }
    let response = app
        .json(Method::POST, "/api/v1/auth/login", None, body)
        .await;
    assert_eq!(response.status, StatusCode::OK);
    pair(response.json())
}

async fn extension(app: &TestApp, token: &str, label: &str) -> Result<Pair, StatusCode> {
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/device-sessions",
            Some(token),
            json!({ "label": label }),
        )
        .await;
    if response.status == StatusCode::OK {
        Ok(pair(response.json()))
    } else {
        Err(response.status)
    }
}

async fn status_of(app: &TestApp, token: &str) -> StatusCode {
    app.get("/api/v1/me", token).await.status
}

async fn refresh(app: &TestApp, token: &str) -> Result<Pair, StatusCode> {
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/refresh",
            None,
            json!({ "refresh_token": token }),
        )
        .await;
    if response.status == StatusCode::OK {
        Ok(pair(response.json()))
    } else {
        Err(response.status)
    }
}

async fn logout(app: &TestApp, token: &str) {
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/logout",
            None,
            json!({ "refresh_token": token }),
        )
        .await;
    assert!(response.status.is_success());
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_extension_session_refreshes_on_its_own(pool: PgPool) {
    let app = setup(pool).await;
    let phone = sign_in(&app, Some(Uuid::now_v7())).await;
    let share = extension(&app, &phone.access, "share")
        .await
        .expect("share");

    let next = refresh(&app, &share.refresh).await.expect("refresh share");
    assert_eq!(status_of(&app, &next.access).await, StatusCode::OK);
    // The app's own token is untouched by it.
    let app_next = refresh(&app, &phone.refresh).await.expect("refresh app");
    assert_eq!(status_of(&app, &app_next.access).await, StatusCode::OK);
    assert_eq!(status_of(&app, &next.access).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_holds_one_session_per_label(pool: PgPool) {
    let app = setup(pool).await;
    let phone = sign_in(&app, Some(Uuid::now_v7())).await;
    let first = extension(&app, &phone.access, "files")
        .await
        .expect("first");
    let share = extension(&app, &phone.access, "share")
        .await
        .expect("share");

    let second = extension(&app, &phone.access, "files")
        .await
        .expect("second");

    assert_eq!(
        status_of(&app, &first.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(refresh(&app, &first.refresh).await.is_err());
    assert_eq!(status_of(&app, &second.access).await, StatusCode::OK);
    assert_eq!(status_of(&app, &share.access).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_the_apps_own_session_on_a_named_device_may_start_one(pool: PgPool) {
    let app = setup(pool).await;
    let unnamed = sign_in(&app, None).await;
    assert_eq!(
        extension(&app, &unnamed.access, "share").await.err(),
        Some(StatusCode::BAD_REQUEST)
    );
    let phone = sign_in(&app, Some(Uuid::now_v7())).await;
    assert_eq!(
        extension(&app, &phone.access, "photos").await.err(),
        Some(StatusCode::BAD_REQUEST)
    );
    let share = extension(&app, &phone.access, "share")
        .await
        .expect("share");
    assert_eq!(
        extension(&app, &share.access, "files").await.err(),
        Some(StatusCode::BAD_REQUEST)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn signing_the_app_out_ends_its_extensions_on_that_device_only(pool: PgPool) {
    let app = setup(pool).await;
    let phone = sign_in(&app, Some(Uuid::now_v7())).await;
    let tablet = sign_in(&app, Some(Uuid::now_v7())).await;
    let phone_share = extension(&app, &phone.access, "share")
        .await
        .expect("phone");
    let tablet_share = extension(&app, &tablet.access, "share")
        .await
        .expect("tablet");

    logout(&app, &phone.refresh).await;

    assert_eq!(
        status_of(&app, &phone_share.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(status_of(&app, &tablet.access).await, StatusCode::OK);
    assert_eq!(status_of(&app, &tablet_share.access).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_extension_signing_out_leaves_the_app_signed_in(pool: PgPool) {
    let app = setup(pool).await;
    let phone = sign_in(&app, Some(Uuid::now_v7())).await;
    let share = extension(&app, &phone.access, "share")
        .await
        .expect("share");
    let files = extension(&app, &phone.access, "files")
        .await
        .expect("files");

    logout(&app, &share.refresh).await;

    assert_eq!(
        status_of(&app, &share.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(status_of(&app, &phone.access).await, StatusCode::OK);
    assert_eq!(status_of(&app, &files.access).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn signing_the_device_out_ends_its_extensions(pool: PgPool) {
    let app = setup(pool).await;
    let device = Uuid::now_v7();
    let phone = sign_in(&app, Some(device)).await;
    let laptop = sign_in(&app, Some(Uuid::now_v7())).await;
    let share = extension(&app, &phone.access, "share")
        .await
        .expect("share");

    let status = app
        .send(
            Method::DELETE,
            &format!("/api/v1/devices/{device}"),
            Some(&laptop.access),
            Body::empty(),
            None,
        )
        .await
        .status;
    assert!(status.is_success(), "{status}");

    assert_eq!(
        status_of(&app, &share.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(status_of(&app, &laptop.access).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_change_keeps_the_extensions_of_the_device_that_changed_it(pool: PgPool) {
    let app = setup(pool).await;
    let phone = sign_in(&app, Some(Uuid::now_v7())).await;
    let laptop = sign_in(&app, Some(Uuid::now_v7())).await;
    let phone_share = extension(&app, &phone.access, "share")
        .await
        .expect("phone");
    let laptop_share = extension(&app, &laptop.access, "share")
        .await
        .expect("laptop");

    let response = app
        .json(
            Method::POST,
            "/api/v1/me/password",
            Some(&phone.access),
            json!({ "current_password": PASSWORD, "new_password": "another-password-2" }),
        )
        .await;
    assert!(response.status.is_success(), "{}", response.status);

    assert_eq!(status_of(&app, &phone.access).await, StatusCode::OK);
    assert_eq!(status_of(&app, &phone_share.access).await, StatusCode::OK);
    assert_eq!(
        status_of(&app, &laptop.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(&app, &laptop_share.access).await,
        StatusCode::UNAUTHORIZED
    );
}

/// The device id is the client's word: a second sign-in that names the same device must not
/// get a session that outlives a password change, nor touch the first one's extensions.
#[sqlx::test(migrations = "../../migrations")]
async fn a_password_change_ends_extensions_of_another_session_naming_the_same_device(pool: PgPool) {
    let app = setup(pool).await;
    let device = Uuid::now_v7();
    let phone = sign_in(&app, Some(device)).await;
    let phone_share = extension(&app, &phone.access, "share")
        .await
        .expect("phone share");
    let intruder = sign_in(&app, Some(device)).await;
    let intruder_files = extension(&app, &intruder.access, "files")
        .await
        .expect("intruder files");
    let intruder_share = extension(&app, &intruder.access, "share")
        .await
        .expect("intruder share");
    // Starting the same label from another session leaves the phone's own alone.
    assert_eq!(status_of(&app, &phone_share.access).await, StatusCode::OK);

    let response = app
        .json(
            Method::POST,
            "/api/v1/me/password",
            Some(&phone.access),
            json!({ "current_password": PASSWORD, "new_password": "another-password-2" }),
        )
        .await;
    assert!(response.status.is_success(), "{}", response.status);

    assert_eq!(status_of(&app, &phone.access).await, StatusCode::OK);
    assert_eq!(status_of(&app, &phone_share.access).await, StatusCode::OK);
    for ended in [&intruder, &intruder_files, &intruder_share] {
        assert_eq!(
            status_of(&app, &ended.access).await,
            StatusCode::UNAUTHORIZED
        );
        assert!(refresh(&app, &ended.refresh).await.is_err());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_extension_changing_the_password_stays_signed_in_with_its_app(pool: PgPool) {
    let app = setup(pool).await;
    let phone = sign_in(&app, Some(Uuid::now_v7())).await;
    let laptop = sign_in(&app, Some(Uuid::now_v7())).await;
    let share = extension(&app, &phone.access, "share")
        .await
        .expect("share");
    let files = extension(&app, &phone.access, "files")
        .await
        .expect("files");

    let response = app
        .json(
            Method::POST,
            "/api/v1/me/password",
            Some(&share.access),
            json!({ "current_password": PASSWORD, "new_password": "another-password-2" }),
        )
        .await;
    assert!(response.status.is_success(), "{}", response.status);

    assert_eq!(status_of(&app, &share.access).await, StatusCode::OK);
    assert_eq!(status_of(&app, &phone.access).await, StatusCode::OK);
    assert_eq!(
        status_of(&app, &files.access).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(&app, &laptop.access).await,
        StatusCode::UNAUTHORIZED
    );
}
