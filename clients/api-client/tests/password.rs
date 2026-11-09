use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

use api_client::{ApiError, Client};
use protocol::{ErrorCode, paths};

async fn signed_in(server: &MockServer) -> Client {
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 900,
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(paths::ME))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "username": "alice",
            "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
        })))
        .mount(server)
        .await;
    let client = Client::new(server.uri());
    client.login("alice", "old-password").await.expect("login");
    client
}

#[tokio::test]
async fn change_password_sends_both_passwords() {
    let server = MockServer::start().await;
    let client = signed_in(&server).await;
    Mock::given(method("POST"))
        .and(path(paths::ME_PASSWORD))
        .and(header("authorization", "Bearer access"))
        .and(body_json(json!({
            "current_password": "old-password",
            "new_password": "new-password",
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    client
        .change_password("old-password", "new-password")
        .await
        .expect("changed");
}

#[tokio::test]
async fn a_wrong_current_password_keeps_the_session() {
    let server = MockServer::start().await;
    let client = signed_in(&server).await;
    Mock::given(method("POST"))
        .and(path(paths::ME_PASSWORD))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "bad_request",
            "message": "the current password is wrong",
        })))
        .mount(&server)
        .await;

    let error = client
        .change_password("guess", "new-password")
        .await
        .expect_err("refused");
    assert!(matches!(
        error,
        ApiError::Api { code: ErrorCode::BadRequest, ref message } if message == "the current password is wrong"
    ));
    client.me().await.expect("still signed in");
}
