use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_partial_json, header, method, path},
};

use api_client::{ApiError, Client, Confirmation, SecondFactor, SecondFactorStep, SignIn};
use protocol::{AuthMethod, ErrorCode, paths};

fn me() -> serde_json::Value {
    json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "username": "alice",
        "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
        "second_factors": { "totp": true, "recovery_codes_left": 9 },
    })
}

fn tokens() -> serde_json::Value {
    json!({ "access_token": "access", "refresh_token": "refresh", "expires_in": 900 })
}

async fn mount_me(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(paths::ME))
        .respond_with(ResponseTemplate::new(200).set_body_json(me()))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_sign_in_that_needs_a_code_finishes_with_it() {
    let server = MockServer::start().await;
    mount_me(&server).await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .and(body_partial_json(
            json!({ "username": "alice", "two_step": true }),
        ))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "type": "/api/v1/problems/second-factor-required",
            "status": 403,
            "code": "forbidden",
            "detail": "This account uses two-factor sign-in.",
            "login_token": "pending",
            "methods": ["totp", "recovery"],
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN_VERIFY))
        .and(body_json(
            json!({ "login_token": "pending", "code": "123456" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens()))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::new(server.uri());
    let step = client
        .login_two_step("alice", "password")
        .await
        .expect("first step");
    assert_eq!(
        step,
        SignIn::SecondFactor(SecondFactorStep {
            login_token: "pending".to_owned(),
            methods: vec![AuthMethod::Totp, AuthMethod::Recovery],
        })
    );
    let me = client
        .login_verify("pending", SecondFactor::Code("123456"))
        .await
        .expect("signed in");
    assert!(me.second_factors.totp);
    assert_eq!(client.access_token().as_deref(), Some("access"));
}

#[tokio::test]
async fn a_sign_in_without_a_second_factor_is_done_at_once() {
    let server = MockServer::start().await;
    mount_me(&server).await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens()))
        .mount(&server)
        .await;
    let client = Client::new(server.uri());
    let done = client
        .login_two_step("alice", "password")
        .await
        .expect("done");
    assert!(matches!(done, SignIn::Done(me) if me.username == "alice"));
}

#[tokio::test]
async fn a_change_that_needs_a_proof_says_which_and_takes_a_code() {
    let server = MockServer::start().await;
    mount_me(&server).await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::ME_RECOVERY_CODES))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "type": "/api/v1/problems/reauthentication-required",
            "status": 403,
            "code": "forbidden",
            "detail": "Confirm it is you.",
            "methods": ["password", "totp"],
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::ME_REAUTHENTICATE))
        .and(header("authorization", "Bearer access"))
        .and(body_json(json!({ "code": "123456" })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri());
    client.login("alice", "password").await.expect("login");

    let error = client
        .new_recovery_codes()
        .await
        .expect_err("needs a proof");
    assert_eq!(
        error,
        ApiError::ReauthenticationRequired {
            message: "Confirm it is you.".to_owned(),
            methods: vec![AuthMethod::Password, AuthMethod::Totp],
        }
    );
    assert_eq!(error.code(), "forbidden");
    client
        .reauthenticate(Confirmation::Code("123456"))
        .await
        .expect("confirmed");
}

#[tokio::test]
async fn a_wrong_code_is_said_as_the_server_says_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::LOGIN_VERIFY))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "type": "/api/v1/problems/invalid-code",
            "status": 422,
            "code": "bad_request",
            "detail": "The code is wrong. Check it and try again.",
        })))
        .mount(&server)
        .await;
    let client = Client::new(server.uri());
    let error = client
        .login_verify("pending", SecondFactor::RecoveryCode("abcde-fghjk"))
        .await
        .expect_err("refused");
    assert!(matches!(
        error,
        ApiError::Api { code: ErrorCode::BadRequest, ref message } if message.starts_with("The code is wrong")
    ));
}
