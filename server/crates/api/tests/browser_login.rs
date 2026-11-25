//! Browser sign-in for apps: `start`, the web client's `flows`, `approve` / `deny`, and the
//! app's `exchange`.

mod common;

use axum::http::{Method, StatusCode};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;

use common::{Response, TestApp, setup, setup_two_tenants};

const LOOPBACK: &str = "http://127.0.0.1:54321/callback";
const MOBILE: &str = "app.homespace://oidc/callback";
const VERIFIER: &str = "0123456789abcdef0123456789abcdef0123456789abc";

fn challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))
}

fn start_uri(redirect: &str, challenge: &str) -> String {
    let mut url = Url::parse("http://server/api/v1/auth/browser/start").expect("url");
    url.query_pairs_mut()
        .append_pair("redirect_uri", redirect)
        .append_pair("code_challenge", challenge)
        .append_pair("device_name", "Laptop");
    format!("{}?{}", url.path(), url.query().expect("query"))
}

async fn web_app(pool: PgPool) -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = setup(pool).await.with_web(&dir);
    (app, dir)
}

async fn get(app: &TestApp, uri: &str, token: Option<&str>) -> Response {
    app.send(Method::GET, uri, token, axum::body::Body::empty(), None)
        .await
}

/// Starts a sign-in from `redirect` and returns the flow id the browser is sent with.
async fn start(app: &TestApp, redirect: &str) -> String {
    let response = get(app, &start_uri(redirect, &challenge()), None).await;
    assert_eq!(
        response.status,
        StatusCode::SEE_OTHER,
        "{:?}",
        response.json()
    );
    assert_eq!(response.headers["cache-control"], "no-store");
    let page = Url::parse("http://server")
        .expect("base")
        .join(&response.location())
        .expect("location");
    assert_eq!(page.path(), "/authorize");
    query(&page, "flow").expect("flow")
}

fn query(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

async fn decide(app: &TestApp, answer: &str, flow: &str, token: &str) -> Response {
    app.json(
        Method::POST,
        &format!("/api/v1/auth/browser/{answer}"),
        Some(token),
        json!({ "flow": flow }),
    )
    .await
}

/// Approves `flow` as `token`'s user and returns the code sent to the app.
async fn approve(app: &TestApp, flow: &str, token: &str) -> String {
    let response = decide(app, "approve", flow, token).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json());
    let redirect =
        Url::parse(response.json()["redirect"].as_str().expect("redirect")).expect("redirect url");
    assert_eq!(redirect.path(), "/callback");
    query(&redirect, "code").expect("code")
}

async fn exchange(app: &TestApp, code: &str, verifier: &str) -> Response {
    app.json(
        Method::POST,
        "/api/v1/auth/browser/exchange",
        None,
        json!({
            "code": code,
            "code_verifier": verifier,
            "device": {
                "id": "6f9619ff-8b86-4d01-b42d-00cf4fc964ff",
                "name": "Laptop",
                "platform": "linux",
            },
        }),
    )
    .await
}

fn problem_type(response: &Response) -> String {
    response.json()["type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_app_signs_in_through_the_web_page(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let web_token = app.user_token("alice").await;
    let flow = start(&app, LOOPBACK).await;

    let shown = get(
        &app,
        &format!("/api/v1/auth/browser/flows/{flow}"),
        Some(&web_token),
    )
    .await;
    assert_eq!(shown.status, StatusCode::OK);
    assert_eq!(
        shown.json(),
        json!({ "app": "desktop", "device_name": "Laptop" })
    );

    let code = approve(&app, &flow, &web_token).await;
    let session = exchange(&app, &code, VERIFIER).await;
    assert_eq!(session.status, StatusCode::OK, "{:?}", session.json());
    let pair = session.json();
    assert!(
        pair["refresh_token"]
            .as_str()
            .is_some_and(|t| !t.is_empty())
    );
    assert!(session.headers.get("set-cookie").is_none());

    let access = pair["access_token"].as_str().expect("access token");
    let me = app.get("/api/v1/me", access).await;
    assert_eq!(me.json()["username"], "alice");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_code_works_once_and_only_with_its_verifier(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let token = app.user_token("alice").await;

    let flow = start(&app, MOBILE).await;
    let code = approve(&app, &flow, &token).await;
    let wrong = exchange(&app, &code, &"x".repeat(43)).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    // The wrong verifier used the code up.
    assert_eq!(
        exchange(&app, &code, VERIFIER).await.status,
        StatusCode::UNAUTHORIZED
    );

    let flow = start(&app, MOBILE).await;
    let code = approve(&app, &flow, &token).await;
    assert_eq!(exchange(&app, &code, VERIFIER).await.status, StatusCode::OK);
    assert_eq!(
        exchange(&app, &code, VERIFIER).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_is_answered_only_once(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let token = app.user_token("alice").await;
    let flow = start(&app, LOOPBACK).await;

    let denied = decide(&app, "deny", &flow, &token).await;
    assert_eq!(denied.status, StatusCode::OK);
    assert_eq!(
        denied.json()["redirect"],
        "http://127.0.0.1:54321/callback?error=access_denied"
    );

    for answer in ["approve", "deny"] {
        let late = decide(&app, answer, &flow, &token).await;
        assert_eq!(late.status, StatusCode::GONE);
        assert_eq!(problem_type(&late), "login-expired");
    }
    let shown = get(
        &app,
        &format!("/api/v1/auth/browser/flows/{flow}"),
        Some(&token),
    )
    .await;
    assert_eq!(problem_type(&shown), "login-expired");
}

#[sqlx::test(migrations = "../../migrations")]
async fn answering_needs_a_signed_in_user(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let flow = start(&app, LOOPBACK).await;

    let shown = get(&app, &format!("/api/v1/auth/browser/flows/{flow}"), None).await;
    assert_eq!(shown.status, StatusCode::UNAUTHORIZED);
    let approved = app
        .json(
            Method::POST,
            "/api/v1/auth/browser/approve",
            None,
            json!({ "flow": flow }),
        )
        .await;
    assert_eq!(approved.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_app_redirects_and_s256_challenges_start(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    for redirect in [
        "https://evil.example.com/callback",
        "http://localhost:54321/callback",
        // The web client signs in on its own, not through this flow.
        "http://server/login/callback",
    ] {
        let response = get(&app, &start_uri(redirect, &challenge()), None).await;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{redirect}"
        );
    }
    let response = get(&app, &start_uri(LOOPBACK, "plain"), None).await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_redirect_is_taken_only_short_and_as_written(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let padded = format!("http://127.0.0.1:{}54321/callback", "0".repeat(10_000));
    let tabs = format!("http://127.0.0.1:54321/{}callback", "\t".repeat(10_000));
    for redirect in [
        padded.as_str(),
        tabs.as_str(),
        "http://127.0.0.1:54321/x/../callback",
    ] {
        let response = get(&app, &start_uri(redirect, &challenge()), None).await;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{} bytes",
            redirect.len()
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn one_address_may_start_only_so_many_sign_ins(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let start_from = |address: &'static str| {
        let app = &app;
        async move {
            app.send_with(
                Method::GET,
                &start_uri(LOOPBACK, &challenge()),
                None,
                axum::body::Body::empty(),
                None,
                &[("x-forwarded-for", address)],
            )
            .await
        }
    };
    for _ in 0..30 {
        assert_eq!(
            start_from("198.51.100.7").await.status,
            StatusCode::SEE_OTHER
        );
    }
    let refused = start_from("198.51.100.7").await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(problem_type(&refused), "too-many-attempts");
    assert_eq!(
        start_from("203.0.113.9").await.status,
        StatusCode::SEE_OTHER
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn without_the_web_client_apps_cannot_start(pool: PgPool) {
    let app = setup(pool).await;
    let response = get(&app, &start_uri(LOOPBACK, &challenge()), None).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem_type(&response), "browser-login-unavailable");

    let providers = get(&app, "/api/v1/auth/providers", None).await.json();
    assert_eq!(providers["browser_login"], Value::Bool(false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn providers_announce_browser_sign_in_with_the_web_client(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let providers = get(&app, "/api/v1/auth/providers", None).await.json();
    assert_eq!(providers["browser_login"], Value::Bool(true));
    // Older apps get a provider button that signs in on the web page.
    assert_eq!(
        providers["oidc"],
        json!({ "name": "your browser", "browser": true })
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_older_app_signs_in_on_the_web_page_through_its_provider_button(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let token = app.user_token("alice").await;
    let url = Url::parse_with_params(
        "http://server/api/v1/auth/oidc/start",
        [
            ("redirect_uri", MOBILE),
            ("code_challenge", challenge().as_str()),
        ],
    )
    .expect("url");
    let uri = format!("{}?{}", url.path(), url.query().expect("query"));
    let response = get(&app, &uri, None).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.headers["referrer-policy"], "no-referrer");
    let page = Url::parse("http://server")
        .expect("base")
        .join(&response.location())
        .expect("location");
    assert_eq!(page.path(), "/authorize");
    let flow = query(&page, "flow").expect("flow");

    let response = decide(&app, "approve", &flow, &token).await;
    let redirect =
        Url::parse(response.json()["redirect"].as_str().expect("redirect")).expect("redirect url");
    let code = query(&redirect, "code").expect("code");
    let tokens = app
        .json(
            Method::POST,
            "/api/v1/auth/oidc/exchange",
            None,
            json!({ "code": code, "code_verifier": VERIFIER }),
        )
        .await;
    assert_eq!(tokens.status, StatusCode::OK, "{:?}", tokens.json());
    let access = tokens.json()["access_token"]
        .as_str()
        .expect("access")
        .to_owned();
    let me = get(&app, "/api/v1/me", Some(&access)).await;
    assert_eq!(me.json()["username"], "alice");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_disabled_user_cannot_let_an_app_in(pool: PgPool) {
    let (app, _web) = web_app(pool).await;
    let token = app.user_token("alice").await;
    let flow = start(&app, LOOPBACK).await;
    app.db
        .set_disabled(app.user_id("alice").await, true)
        .await
        .expect("disable");

    // Disabling ends the account's sessions, so the web page's token stops working first.
    let response = decide(&app, "approve", &flow, &token).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_belongs_to_the_tenant_it_was_started_on(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let web = tempfile::tempdir().expect("tempdir");
    let (a, b) = (a.with_web(&web), b.with_web(&web));
    let flow = start(&a, LOOPBACK).await;
    let token_b = b.user_token("bob").await;

    let response = decide(&b, "approve", &flow, &token_b).await;
    assert_eq!(problem_type(&response), "login-expired");

    let token_a = a.user_token("alice").await;
    let code = approve(&a, &flow, &token_a).await;
    assert_eq!(
        exchange(&b, &code, VERIFIER).await.status,
        StatusCode::UNAUTHORIZED
    );
}
