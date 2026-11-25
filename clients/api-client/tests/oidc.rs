use std::sync::{Arc, Mutex};

use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

use api_client::{ApiError, Client, Pkce, VERIFIER_BYTES};
use protocol::ErrorCode;

fn pkce() -> Pkce {
    Pkce::from_random([42; VERIFIER_BYTES])
}

#[tokio::test]
async fn providers_are_read_without_a_session() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/providers"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "password": false, "oidc": { "name": "Keycloak" } })),
        )
        .mount(&server)
        .await;

    let providers = Client::new(server.uri())
        .providers()
        .await
        .expect("providers");

    assert!(!providers.password);
    assert_eq!(providers.oidc.expect("oidc").name, "Keycloak");
}

#[test]
fn start_url_carries_the_redirect_and_challenge() {
    let client = Client::new("https://files.example.com");
    let pkce = pkce();
    let url = client
        .oidc_start_url("https://files.example.com/login/callback", &pkce)
        .expect("url");

    let parsed = reqwest::Url::parse(&url).expect("parse");
    assert_eq!(parsed.path(), "/api/v1/auth/oidc/start");
    let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(
        query["redirect_uri"],
        "https://files.example.com/login/callback"
    );
    assert_eq!(query["code_challenge"], pkce.challenge());
}

#[test]
fn start_url_fails_for_an_invalid_server_address() {
    let result = Client::new("").oidc_start_url("http://127.0.0.1:1/callback", &pkce());
    assert!(matches!(result, Err(ApiError::Transport(_))));
}

#[tokio::test]
async fn exchange_signs_in_and_reports_the_refresh_token() {
    let server = MockServer::start().await;
    let pkce = pkce();
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/oidc/exchange"))
        .and(body_json(
            json!({ "code": "login-code", "code_verifier": pkce.verifier() }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "access_token": "access-1", "refresh_token": "refresh-1", "expires_in": 900 }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("authorization", "Bearer access-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "username": "alice",
            "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
        })))
        .mount(&server)
        .await;
    let saved = Arc::new(Mutex::new(None::<String>));
    let listener = {
        let saved = saved.clone();
        Arc::new(move |token: Option<&str>| *saved.lock().expect("lock") = token.map(str::to_owned))
    };
    let client = Client::with_listener(server.uri(), listener);

    let me = client
        .oidc_exchange("login-code", &pkce)
        .await
        .expect("exchange");

    assert_eq!(me.username, "alice");
    assert_eq!(client.access_token().as_deref(), Some("access-1"));
    assert_eq!(saved.lock().expect("lock").as_deref(), Some("refresh-1"));
}

#[tokio::test]
async fn exchange_with_a_bad_code_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/oidc/exchange"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(
                json!({ "error": "unauthorized", "message": "invalid credentials" }),
            ),
        )
        .mount(&server)
        .await;
    let client = Client::new(server.uri());

    let result = client.oidc_exchange("bad", &pkce()).await;

    assert!(result.is_err());
    assert!(client.access_token().is_none());
}

#[tokio::test]
async fn a_refused_sign_in_keeps_the_servers_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "type": "/api/v1/problems/unauthorized",
            "title": "Not signed in",
            "status": 401,
            "detail": "Invalid username or password.",
            "error": "unauthorized",
            "message": "Invalid username or password.",
        })))
        .mount(&server)
        .await;
    let client = Client::new(server.uri());

    let result = client.login("alice", "wrong").await;

    assert_eq!(
        result.expect_err("refused"),
        ApiError::Api {
            code: ErrorCode::Unauthorized,
            message: "Invalid username or password.".to_owned(),
        }
    );
}

#[tokio::test]
async fn an_older_servers_refusal_stays_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(
                json!({ "error": "unauthorized", "message": "invalid credentials" }),
            ),
        )
        .mount(&server)
        .await;
    let client = Client::new(server.uri());

    let result = client.login("alice", "wrong").await;

    assert_eq!(result.expect_err("refused"), ApiError::Unauthorized);
}
