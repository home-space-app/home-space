use serde_json::json;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

use api_client::Client;
use protocol::{DeviceInfo, PLATFORM_LINUX, paths};

#[tokio::test]
async fn login_names_the_device_the_client_was_given() {
    let server = MockServer::start().await;
    let device = DeviceInfo {
        id: Uuid::from_u128(7),
        name: "Laptop".to_owned(),
        platform: PLATFORM_LINUX.to_owned(),
        version: Some("0.13.0".to_owned()),
        requires: Vec::new(),
    };
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .and(body_partial_json(json!({ "device": device })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 900,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(paths::ME))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "username": "alice",
            "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
        })))
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).with_device(device);

    client.login("alice", "pw").await.expect("login");
}

#[tokio::test]
async fn a_device_session_hands_back_its_refresh_token_and_keeps_the_clients_own() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 900,
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(paths::ME))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "username": "alice",
            "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::DEVICE_SESSIONS))
        .and(body_partial_json(json!({ "label": "share" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "share-access",
            "refresh_token": "share-refresh",
            "expires_in": 900,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri());
    client.login("alice", "pw").await.expect("login");

    let token = client.start_device_session("share").await.expect("session");

    assert_eq!(token, "share-refresh");
    assert_eq!(client.access_token().as_deref(), Some("access"));
}
