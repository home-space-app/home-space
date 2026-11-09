//! What the server does for a browser: the refresh token in a cookie no script can read,
//! requests from other sites refused, and the security headers on every response.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue},
};
use tower::ServiceExt;

use common::{PASSWORD, Response, TestApp, setup};

const HOST: &str = "files.example.com";
const ORIGIN: &str = "https://files.example.com";
const OTHER_ORIGIN: &str = "https://evil.example.net";
const COOKIE_NAME: &str = "homespace_refresh";
const LOGIN: &str = "/api/v1/auth/login";
const REFRESH: &str = "/api/v1/auth/refresh";
const LOGOUT: &str = "/api/v1/auth/logout";

/// A request as a page at `origin` would send it, with `cookie` when the browser has one.
async fn browser_post(
    app: &TestApp,
    uri: &str,
    origin: Option<&str>,
    cookie: Option<&str>,
    payload: Value,
) -> Response {
    let cookie = cookie.map(|token| format!("{COOKIE_NAME}={token}"));
    let mut headers = vec![("host", HOST)];
    if let Some(origin) = origin {
        headers.push(("origin", origin));
    }
    if let Some(cookie) = &cookie {
        headers.push(("cookie", cookie));
    }
    app.send_with(
        Method::POST,
        uri,
        None,
        Body::from(payload.to_string()),
        Some("application/json"),
        &headers,
    )
    .await
}

fn set_cookie(response: &Response) -> String {
    response
        .headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("set-cookie header")
        .to_owned()
}

/// The token a `Set-Cookie` header stores.
fn cookie_token(response: &Response) -> String {
    let header = set_cookie(response);
    let (pair, _) = header.split_once(';').expect("attributes");
    let (name, token) = pair.split_once('=').expect("name and value");
    assert_eq!(name, COOKIE_NAME);
    token.to_owned()
}

/// Signs alice in from a page of the server itself.
async fn sign_in(app: &TestApp) -> Response {
    app.state
        .auth
        .create_user("alice", PASSWORD)
        .await
        .expect("create user");
    let response = browser_post(
        app,
        LOGIN,
        Some(ORIGIN),
        None,
        json!({ "username": "alice", "password": PASSWORD, "cookie": true }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    response
}

fn access_token(response: &Response) -> String {
    response.json()["access_token"]
        .as_str()
        .expect("access token")
        .to_owned()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_browser_gets_its_refresh_token_in_a_cookie_only(pool: PgPool) {
    let app = setup(pool).await;
    let login = sign_in(&app).await;

    assert!(login.json().get("refresh_token").is_none());
    let cookie = set_cookie(&login);
    for attribute in [
        "; HttpOnly",
        "; SameSite=Strict",
        "; Secure",
        "; Path=/api/v1/auth;",
        "; Max-Age=2592000",
    ] {
        assert!(cookie.contains(attribute), "{attribute} in {cookie}");
    }
    let me = app.get("/api/v1/me", &access_token(&login)).await;
    assert_eq!(me.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_page_served_without_tls_gets_a_cookie_it_can_send_back(pool: PgPool) {
    let app = setup(pool).await;
    app.state
        .auth
        .create_user("alice", PASSWORD)
        .await
        .expect("create user");
    let login = browser_post(
        &app,
        LOGIN,
        Some("http://files.example.com"),
        None,
        json!({ "username": "alice", "password": PASSWORD, "cookie": true }),
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);
    assert!(!set_cookie(&login).contains("Secure"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_app_still_gets_its_refresh_token_in_the_body(pool: PgPool) {
    let app = setup(pool).await;
    app.user_token("alice").await;
    let login = app
        .json(
            Method::POST,
            LOGIN,
            None,
            json!({ "username": "alice", "password": PASSWORD }),
        )
        .await;
    assert!(login.json()["refresh_token"].is_string());
    assert!(login.headers.get("set-cookie").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_cookie_refreshes_the_session_and_is_replaced(pool: PgPool) {
    let app = setup(pool).await;
    let first = cookie_token(&sign_in(&app).await);

    let refreshed = browser_post(
        &app,
        REFRESH,
        Some(ORIGIN),
        Some(&first),
        json!({ "cookie": true }),
    )
    .await;

    assert_eq!(refreshed.status, StatusCode::OK);
    assert!(refreshed.json().get("refresh_token").is_none());
    assert_ne!(cookie_token(&refreshed), first);
    let me = app.get("/api/v1/me", &access_token(&refreshed)).await;
    assert_eq!(me.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn two_tabs_refreshing_at_once_both_stay_signed_in(pool: PgPool) {
    let app = setup(pool).await;
    let shared = cookie_token(&sign_in(&app).await);
    let refresh = || browser_post(&app, REFRESH, Some(ORIGIN), Some(&shared), json!({}));

    let one = refresh().await;
    let other = refresh().await;

    assert_eq!((one.status, other.status), (StatusCode::OK, StatusCode::OK));
    // The tab that came second leaves the cookie alone: it holds the first tab's new token.
    assert!(other.headers.get("set-cookie").is_none());
    let me = app.get("/api/v1/me", &access_token(&other)).await;
    assert_eq!(me.status, StatusCode::OK);
    let next = browser_post(
        &app,
        REFRESH,
        Some(ORIGIN),
        Some(&cookie_token(&one)),
        json!({}),
    )
    .await;
    assert_eq!(next.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_site_cannot_use_the_cookie(pool: PgPool) {
    let app = setup(pool).await;
    let token = cookie_token(&sign_in(&app).await);

    for origin in [Some(OTHER_ORIGIN), Some("null"), None] {
        for uri in [REFRESH, LOGOUT] {
            let response = browser_post(&app, uri, origin, Some(&token), json!({})).await;
            assert_eq!(
                response.status,
                StatusCode::FORBIDDEN,
                "{uri} from {origin:?}"
            );
            assert!(response.headers.get("set-cookie").is_none());
        }
    }
    // None of that used the token up or ended the session.
    let refreshed = browser_post(&app, REFRESH, Some(ORIGIN), Some(&token), json!({})).await;
    assert_eq!(refreshed.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_site_cannot_sign_a_browser_in(pool: PgPool) {
    let app = setup(pool).await;
    app.state
        .auth
        .create_user("alice", PASSWORD)
        .await
        .expect("create user");
    let login = browser_post(
        &app,
        LOGIN,
        Some(OTHER_ORIGIN),
        None,
        json!({ "username": "alice", "password": PASSWORD, "cookie": true }),
    )
    .await;
    assert_eq!(login.status, StatusCode::FORBIDDEN);
    assert!(login.headers.get("set-cookie").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn signing_out_ends_the_session_and_clears_the_cookie(pool: PgPool) {
    let app = setup(pool).await;
    let login = sign_in(&app).await;
    let token = cookie_token(&login);

    let logout = browser_post(&app, LOGOUT, Some(ORIGIN), Some(&token), json!({})).await;

    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert!(set_cookie(&logout).contains("Max-Age=0"));
    assert_eq!(cookie_token(&logout), "");
    let me = app.get("/api/v1/me", &access_token(&login)).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_cookie_that_no_longer_works_is_cleared(pool: PgPool) {
    let app = setup(pool).await;
    let refreshed = browser_post(&app, REFRESH, Some(ORIGIN), Some("not-a-token"), json!({})).await;
    assert_eq!(refreshed.status, StatusCode::UNAUTHORIZED);
    assert!(set_cookie(&refreshed).contains("Max-Age=0"));

    let none = browser_post(&app, REFRESH, Some(ORIGIN), None, json!({})).await;
    assert_eq!(none.status, StatusCode::UNAUTHORIZED);
}

/// A browser that still has a token from before the cookie existed hands it over once.
#[sqlx::test(migrations = "../../migrations")]
async fn a_token_from_the_body_moves_into_the_cookie(pool: PgPool) {
    let app = setup(pool).await;
    app.user_token("alice").await;
    let login = app
        .json(
            Method::POST,
            LOGIN,
            None,
            json!({ "username": "alice", "password": PASSWORD }),
        )
        .await;
    let old = login.json()["refresh_token"].clone();

    let moved = browser_post(
        &app,
        REFRESH,
        Some(ORIGIN),
        None,
        json!({ "refresh_token": old, "cookie": true }),
    )
    .await;

    assert_eq!(moved.status, StatusCode::OK);
    assert!(moved.json().get("refresh_token").is_none());
    assert!(!cookie_token(&moved).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_api_response_carries_the_security_headers(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    for response in [
        app.get("/api/v1/me", &alice).await,
        app.get("/api/v1/nope", &alice).await,
        app.json(Method::POST, LOGIN, None, json!({})).await,
    ] {
        let header = |name: &str| {
            response
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        assert_eq!(
            header("content-security-policy"),
            "default-src 'none'; frame-ancestors 'none'"
        );
        assert_eq!(header("x-content-type-options"), "nosniff");
        assert_eq!(header("x-frame-options"), "DENY");
        assert_eq!(header("referrer-policy"), "no-referrer");
        assert_eq!(header("cache-control"), "no-store");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_web_client_may_run_only_its_own_scripts(pool: PgPool) {
    const INDEX: &str = "<html><head><script type=\"module\">start()</script></head></html>";
    // base64 of the SHA-256 of `start()`.
    const START_HASH: &str = "'sha256-DIm7WJS6ZKDYe5qFLPy+h4JFI9Bol5QmYC57mt3Fb00='";
    let app = setup(pool).await;
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("index.html"), INDEX).expect("index");
    let router = api::router_with_web(app.state.clone(), dir.path());

    let response = router
        .oneshot(
            axum::http::Request::builder()
                .uri("/folder/some-id")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    let policy = response
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .expect("policy");
    assert!(
        policy.contains(&format!(
            "script-src 'self' 'wasm-unsafe-eval' {START_HASH};"
        )),
        "{policy}"
    );
    assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .expect("nosniff"),
        "nosniff"
    );
    assert!(response.headers().get("cache-control").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_page_of_another_site_cannot_open_the_websocket(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app.router.clone()).into_future());
    let connect = async |origin: &str| {
        let ticket = app
            .json(
                Method::POST,
                "/api/v1/tickets",
                Some(&alice),
                json!({ "scope": "events" }),
            )
            .await
            .json()["ticket"]
            .as_str()
            .expect("ticket")
            .to_owned();
        let mut request = format!("ws://{addr}/api/v1/ws?ticket={ticket}")
            .into_client_request()
            .expect("request");
        request
            .headers_mut()
            .insert("origin", HeaderValue::from_str(origin).expect("origin"));
        connect_async(request).await
    };

    assert!(connect(OTHER_ORIGIN).await.is_err());
    assert!(connect(&format!("http://{addr}")).await.is_ok());
}

#[tokio::test]
async fn a_database_failure_preserves_the_refresh_cookie() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@localhost/unused")
        .expect("pool");
    pool.close().await;
    let app = setup(pool).await;
    let response = browser_post(
        &app,
        REFRESH,
        Some(ORIGIN),
        Some("existing-token"),
        json!({}),
    )
    .await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers.get("set-cookie").is_none());
}
