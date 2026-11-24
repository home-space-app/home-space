use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

use api_client::{Client, Pkce, VERIFIER_BYTES};
use protocol::{BrowserLoginApp, DeviceInfo};

fn pkce() -> Pkce {
    Pkce::from_random([7; VERIFIER_BYTES])
}

fn device() -> DeviceInfo {
    DeviceInfo {
        id: uuid::Uuid::nil(),
        name: "Work laptop".to_owned(),
        platform: "linux".to_owned(),
        version: None,
        requires: Vec::new(),
    }
}

#[test]
fn start_url_names_the_device() {
    let client = Client::new("https://files.example.com").with_device(device());
    let pkce = pkce();
    let url = client
        .browser_login_start_url("http://127.0.0.1:4711/callback", &pkce)
        .expect("url");

    let parsed = reqwest::Url::parse(&url).expect("parse");
    assert_eq!(parsed.path(), "/api/v1/auth/browser/start");
    let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(query["redirect_uri"], "http://127.0.0.1:4711/callback");
    assert_eq!(query["code_challenge"], pkce.challenge());
    assert_eq!(query["device_name"], "Work laptop");
}

#[test]
fn start_url_leaves_out_an_unnamed_device() {
    let url = Client::new("https://files.example.com")
        .browser_login_start_url("app.homespace://oidc/callback", &pkce())
        .expect("url");
    assert!(!url.contains("device_name"));
}

#[tokio::test]
async fn the_code_is_exchanged_with_the_verifier_and_device() {
    let server = MockServer::start().await;
    let pkce = pkce();
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/browser/exchange"))
        .and(body_json(json!({
            "code": "one-time",
            "code_verifier": pkce.verifier(),
            "device": { "id": uuid::Uuid::nil(), "name": "Work laptop", "platform": "linux" },
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access", "refresh_token": "refresh", "expires_in": 900,
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/capabilities"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "upload_chunk_bytes": 1024, "stream_tickets": true,
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": uuid::Uuid::nil(), "username": "alice",
            "lost_and_found_id": uuid::Uuid::nil(),
        })))
        .mount(&server)
        .await;

    let client = Client::new(server.uri()).with_device(device());
    let me = client
        .browser_login_exchange("one-time", &pkce)
        .await
        .expect("signed in");
    assert_eq!(me.username, "alice");
}

#[tokio::test]
async fn the_signed_in_user_answers_a_flow() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access", "refresh_token": "refresh", "expires_in": 900,
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/browser/flows/abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "app": "mobile" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/browser/approve"))
        .and(body_json(json!({ "flow": "abc" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "redirect": "app.homespace://oidc/callback?code=xyz",
        })))
        .mount(&server)
        .await;
    for (route, body) in [
        (
            "/api/v1/capabilities",
            json!({ "upload_chunk_bytes": 1024, "stream_tickets": true }),
        ),
        (
            "/api/v1/me",
            json!({ "id": uuid::Uuid::nil(), "username": "alice",
                    "lost_and_found_id": uuid::Uuid::nil() }),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }

    let client = Client::new(server.uri());
    client.login("alice", "secret").await.expect("login");
    let flow = client.browser_login_flow("abc").await.expect("flow");
    assert_eq!(flow.app, BrowserLoginApp::Mobile);
    assert_eq!(flow.device_name, None);
    let redirect = client.browser_login_approve("abc").await.expect("approve");
    assert_eq!(redirect, "app.homespace://oidc/callback?code=xyz");
}
