mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::json;
use sqlx::PgPool;

use common::{PASSWORD, setup};

#[sqlx::test(migrations = "../../migrations")]
async fn requests_without_a_token_are_unauthorized(pool: PgPool) {
    let app = setup(pool).await;
    let response = app
        .send(Method::GET, "/api/v1/nodes", None, Default::default(), None)
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn garbage_token_is_unauthorized(pool: PgPool) {
    let app = setup(pool).await;
    let response = app.get("/api/v1/nodes", "garbage").await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn wrong_password_is_unauthorized(pool: PgPool) {
    let app = setup(pool).await;
    app.user_token("alice").await;
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({ "username": "alice", "password": "wrong-password" }),
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn me_returns_the_logged_in_user(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let response = app.get("/api/v1/me", &token).await;
    assert_eq!(response.json()["username"], "alice");
}

#[sqlx::test(migrations = "../../migrations")]
async fn me_reports_the_lost_and_found_folder(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let me = app.get("/api/v1/me", &token).await.json();
    let listing = app.get("/api/v1/nodes", &token).await.json();
    let folder = listing
        .as_array()
        .expect("array")
        .iter()
        .find(|node| node["name"] == "Lost & Found")
        .expect("lost and found");
    assert_eq!(me["lost_and_found_id"], folder["id"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_issues_new_tokens_and_old_refresh_token_stops_working(pool: PgPool) {
    let app = setup(pool).await;
    app.user_token("alice").await;
    let login = app
        .json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({ "username": "alice", "password": PASSWORD }),
        )
        .await;
    let old_refresh = login.json()["refresh_token"]
        .as_str()
        .expect("refresh")
        .to_owned();

    let refreshed = app
        .json(
            Method::POST,
            "/api/v1/auth/refresh",
            None,
            json!({ "refresh_token": old_refresh }),
        )
        .await;
    let reused = app
        .json(
            Method::POST,
            "/api/v1/auth/refresh",
            None,
            json!({ "refresh_token": old_refresh }),
        )
        .await;

    assert_eq!(refreshed.status, StatusCode::OK);
    assert_eq!(reused.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn logout_revokes_the_refresh_token(pool: PgPool) {
    let app = setup(pool).await;
    app.user_token("alice").await;
    let login = app
        .json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({ "username": "alice", "password": PASSWORD }),
        )
        .await;
    let refresh = login.json()["refresh_token"]
        .as_str()
        .expect("refresh")
        .to_owned();

    let logout = app
        .json(
            Method::POST,
            "/api/v1/auth/logout",
            None,
            json!({ "refresh_token": refresh }),
        )
        .await;
    let after = app
        .json(
            Method::POST,
            "/api/v1/auth/refresh",
            None,
            json!({ "refresh_token": refresh }),
        )
        .await;

    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_fourth_wrong_password_in_a_row_is_throttled(pool: PgPool) {
    let app = setup(pool).await;
    app.user_token("alice").await;
    let login = |password: &'static str| {
        let app = &app;
        async move {
            app.json(
                Method::POST,
                "/api/v1/auth/login",
                None,
                json!({ "username": "alice", "password": password }),
            )
            .await
        }
    };
    for _ in 0..3 {
        assert_eq!(
            login("wrong-password").await.status,
            StatusCode::UNAUTHORIZED
        );
    }

    let throttled = login(PASSWORD).await;

    assert_eq!(throttled.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(throttled.json()["error"], "too_many_requests");
    let retry_after: u64 = throttled.headers["retry-after"]
        .to_str()
        .expect("header")
        .parse()
        .expect("seconds");
    assert!((29..=31).contains(&retry_after), "{retry_after}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn parallel_wrong_passwords_get_three_checks_at_most(pool: PgPool) {
    let app = setup(pool).await;
    app.user_token("alice").await;
    let attempts = (0..10).map(|_| {
        app.json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({ "username": "alice", "password": "wrong-password" }),
        )
    });

    let responses = futures_util::future::join_all(attempts).await;

    let checked = responses
        .iter()
        .filter(|response| response.status == StatusCode::UNAUTHORIZED)
        .count();
    assert_eq!(checked, 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn wrong_passwords_from_one_address_do_not_lock_out_another(pool: PgPool) {
    let app = setup(pool).await;
    app.user_token("alice").await;
    let login = |password: &'static str, forwarded_for: &'static str| {
        let app = &app;
        async move {
            app.send_with(
                Method::POST,
                "/api/v1/auth/login",
                None,
                Body::from(json!({ "username": "alice", "password": password }).to_string()),
                Some("application/json"),
                &[("x-forwarded-for", forwarded_for)],
            )
            .await
            .status
        }
    };
    for _ in 0..3 {
        assert_eq!(
            login("wrong-password", "198.51.100.7").await,
            StatusCode::UNAUTHORIZED
        );
    }

    assert_eq!(
        login(PASSWORD, "198.51.100.7").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // A forged first entry does not get the guesser out of the block.
    assert_eq!(
        login(PASSWORD, "192.0.2.1, 198.51.100.7").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(login(PASSWORD, "192.0.2.1").await, StatusCode::OK);
}
