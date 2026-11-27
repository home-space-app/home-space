use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

use api_client::{ApiError, Client};
use protocol::{AddUnlocker, ErrorCode, UnlockerKind, encoding::Base64Url, paths};

const UNLOCKER: &str = "0190d1f0-0000-7000-8000-0000000000a1";

fn tokens() -> serde_json::Value {
    json!({ "access_token": "access", "refresh_token": "refresh", "expires_in": 900 })
}

/// A client signed in to `server`.
async fn signed_in(server: &MockServer) -> Client {
    Mock::given(method("GET"))
        .and(path(paths::ME))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "username": "alice",
            "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens()))
        .mount(server)
        .await;
    let client = Client::new(server.uri());
    client.login("alice", "password").await.expect("login");
    client
}

fn folder_key() -> serde_json::Value {
    json!({
        "id": UNLOCKER,
        "kind": "security_key",
        "name": "YubiKey",
        "credential_id": "AQID",
        "rp_id": "files.example.com",
        "prf_salt": "AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM",
        "wrapped_vault_key": "AQ",
        "created_at": "2026-11-26T10:00:00Z",
    })
}

#[tokio::test]
async fn the_vault_lists_what_asks_a_folder_key_and_is_none_before_the_first() {
    let server = MockServer::start().await;
    let client = signed_in(&server).await;
    Mock::given(method("GET"))
        .and(path(paths::VAULT))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "type": "/api/v1/problems/not-found",
            "status": 404,
            "code": "not_found",
            "detail": "not found",
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert_eq!(client.vault().await.expect("no vault"), None);

    Mock::given(method("GET"))
        .and(path(paths::VAULT))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "epoch": 2, "unlockers": [folder_key()] })),
        )
        .mount(&server)
        .await;
    let vault = client.vault().await.expect("vault").expect("some");
    assert_eq!(vault.epoch, 2);
    let key = &vault.unlockers[0];
    assert_eq!(key.kind, UnlockerKind::SecurityKey);
    assert_eq!(key.name.as_deref(), Some("YubiKey"));
    assert_eq!(key.credential_id, Some(Base64Url::from(vec![1, 2, 3])));
    assert_eq!(key.rp_id.as_deref(), Some("files.example.com"));
    assert_eq!(key.prf_salt, Some(Base64Url::from(vec![3; 32])));
}

#[tokio::test]
async fn a_folder_key_is_posted_with_its_own_credential() {
    let server = MockServer::start().await;
    let client = signed_in(&server).await;
    Mock::given(method("POST"))
        .and(path(paths::VAULT_UNLOCKERS))
        .and(body_json(json!({
            "kind": "security_key",
            "name": "YubiKey",
            "credential_id": "AQID",
            "rp_id": "files.example.com",
            "prf_salt": "AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM",
            "wrapped_vault_key": "AQ",
            "epoch": 1,
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(folder_key()))
        .expect(1)
        .mount(&server)
        .await;
    let request = AddUnlocker {
        kind: UnlockerKind::SecurityKey,
        name: Some("YubiKey".to_owned()),
        credential_id: Some(Base64Url::from(vec![1, 2, 3])),
        rp_id: Some("files.example.com".to_owned()),
        prf_salt: Some(Base64Url::from(vec![3; 32])),
        wrapped_vault_key: Base64Url::from(vec![1]),
        epoch: 1,
    };
    let added = client.add_unlocker(&request).await.expect("added");
    assert_eq!(added.id.to_string(), UNLOCKER);
}

#[tokio::test]
async fn the_last_unlocker_of_locked_folders_is_kept() {
    let server = MockServer::start().await;
    let client = signed_in(&server).await;
    Mock::given(method("DELETE"))
        .and(path(paths::vault_unlocker(UNLOCKER.parse().expect("uuid"))))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "type": "/api/v1/problems/last-unlocker",
            "status": 409,
            "code": "conflict",
            "detail": "This is the last key that unlocks your protected folders.",
        })))
        .mount(&server)
        .await;
    let refused = client
        .remove_unlocker(UNLOCKER.parse().expect("uuid"))
        .await;
    assert!(
        matches!(&refused, Err(ApiError::Api { code: ErrorCode::Conflict, message }) if message.contains("last key")),
        "{refused:?}"
    );
}
