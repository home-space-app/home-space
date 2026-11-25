use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

use api_client::{ApiError, Client, Confirmation, SecondFactor};
use protocol::{ErrorCode, PublicKeyCredential, WebAuthnKind, paths};

fn me() -> serde_json::Value {
    json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "username": "alice",
        "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
        "second_factors": { "totp": false, "webauthn": 1, "recovery_codes_left": 10 },
    })
}

fn tokens() -> serde_json::Value {
    json!({ "access_token": "access", "refresh_token": "refresh", "expires_in": 900 })
}

fn answer() -> serde_json::Value {
    json!({
        "id": "AQID",
        "rawId": "AQID",
        "type": "public-key",
        "response": {
            "authenticatorData": "AQID",
            "clientDataJSON": "AQID",
            "signature": "AQID",
            "userHandle": "AQID",
        },
        "clientExtensionResults": {},
    })
}

fn request_options() -> serde_json::Value {
    json!({
        "publicKey": {
            "challenge": "AQID",
            "rpId": "files.example.com",
            "allowCredentials": [],
            "userVerification": "required",
        }
    })
}

async fn mount_me(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(paths::ME))
        .respond_with(ResponseTemplate::new(200).set_body_json(me()))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_passkey_signs_in_with_its_answer_to_the_challenge() {
    let server = MockServer::start().await;
    mount_me(&server).await;
    let challenge_id = "0190d1f0-0000-7000-8000-000000000001";
    Mock::given(method("POST"))
        .and(path(paths::PASSKEY_START))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({ "challenge_id": challenge_id, "options": request_options() }),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::PASSKEY_FINISH))
        .and(body_partial_json(
            json!({ "challenge_id": challenge_id, "credential": { "id": "AQID" } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens()))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri());
    let challenge = client.passkey_start().await.expect("challenge");
    assert_eq!(challenge.options.public_key.rp_id, "files.example.com");
    let credential: PublicKeyCredential = serde_json::from_value(answer()).expect("answer");
    let me = client
        .passkey_finish(challenge.challenge_id.expect("id"), &credential)
        .await
        .expect("signed in");
    assert_eq!(me.second_factors.webauthn, 1);
}

#[tokio::test]
async fn a_key_answers_the_second_step_and_confirms_it_is_the_user() {
    let server = MockServer::start().await;
    mount_me(&server).await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN_WEBAUTHN))
        .and(body_partial_json(json!({ "login_token": "pending" })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "options": request_options() })),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN_VERIFY))
        .and(body_partial_json(
            json!({ "login_token": "pending", "webauthn": { "id": "AQID" } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens()))
        .expect(1)
        .mount(&server)
        .await;
    let challenge_id = "0190d1f0-0000-7000-8000-000000000002";
    Mock::given(method("POST"))
        .and(path(paths::ME_REAUTHENTICATE))
        .and(body_partial_json(
            json!({ "webauthn": { "challenge_id": challenge_id, "credential": { "id": "AQID" } } }),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri());
    let challenge = client
        .login_webauthn_options("pending")
        .await
        .expect("options");
    assert!(challenge.challenge_id.is_none());
    let credential: PublicKeyCredential = serde_json::from_value(answer()).expect("answer");
    client
        .login_verify("pending", SecondFactor::WebAuthn(&credential))
        .await
        .expect("signed in");
    client
        .reauthenticate(Confirmation::WebAuthn {
            challenge_id: challenge_id.parse().expect("uuid"),
            credential: &credential,
        })
        .await
        .expect("confirmed");
}

#[tokio::test]
async fn too_many_keys_is_a_conflict_with_the_servers_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::ME_WEBAUTHN_START))
        .and(body_partial_json(json!({ "kind": "security_key" })))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "type": "/api/v1/problems/too-many-keys",
            "status": 409,
            "code": "conflict",
            "detail": "This account has 20 passkeys and security keys.",
        })))
        .mount(&server)
        .await;
    mount_me(&server).await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens()))
        .mount(&server)
        .await;
    let client = Client::new(server.uri());
    client.login("alice", "password").await.expect("login");
    let refused = client.start_webauthn(WebAuthnKind::SecurityKey).await;
    assert!(
        matches!(&refused, Err(ApiError::Api { code: ErrorCode::Conflict, message }) if message.contains("20 passkeys")),
        "{refused:?}"
    );
}
