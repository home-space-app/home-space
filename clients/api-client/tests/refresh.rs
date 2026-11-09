use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

use api_client::{ApiError, Client, TokenListener};
use protocol::ErrorCode;

const USER_ID: &str = "00000000-0000-0000-0000-000000000001";

fn pair(access: &str, refresh: &str) -> Value {
    json!({ "access_token": access, "refresh_token": refresh, "expires_in": 900 })
}

fn me() -> Value {
    json!({ "id": USER_ID, "username": "alice", "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0" })
}

async fn logged_in(server: &MockServer, client: &Client) {
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("old", "refresh-1")))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("authorization", "Bearer old"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me()))
        .mount(server)
        .await;
    client.login("alice", "pw").await.expect("login");
}

#[tokio::test]
async fn expired_access_token_is_refreshed_and_the_call_retried() {
    let server = MockServer::start().await;
    let client = Client::new(server.uri());
    logged_in(&server, &client).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes"))
        .and(header("authorization", "Bearer old"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(body_json(json!({ "refresh_token": "refresh-1" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("new", "refresh-2")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes"))
        .and(header("authorization", "Bearer new"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let nodes = client.list(None).await.expect("list");

    assert!(nodes.is_empty());
}

#[tokio::test]
async fn concurrent_expired_calls_refresh_only_once() {
    let server = MockServer::start().await;
    let client = Client::new(server.uri());
    logged_in(&server, &client).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes"))
        .and(header("authorization", "Bearer old"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("new", "refresh-2")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes"))
        .and(header("authorization", "Bearer new"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let (first, second, third) =
        tokio::join!(client.list(None), client.list(None), client.list(None));

    assert!(first.is_ok() && second.is_ok() && third.is_ok());
}

#[tokio::test]
async fn failed_refresh_ends_the_session_and_notifies_the_listener() {
    let server = MockServer::start().await;
    let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let listener: TokenListener = Arc::new(move |token| {
        sink.lock().expect("lock").push(token.map(str::to_owned));
    });
    let client = Client::with_listener(server.uri(), listener);
    logged_in(&server, &client).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let result = client.list(None).await;

    assert_eq!(result.expect_err("unauthorized"), ApiError::Unauthorized);
    assert_eq!(client.access_token(), None);
    assert_eq!(
        *seen.lock().expect("lock"),
        [Some("refresh-1".to_owned()), None]
    );
}

#[tokio::test]
async fn restore_rotates_the_saved_refresh_token() {
    let server = MockServer::start().await;
    let client = Client::new(server.uri());
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(body_json(json!({ "refresh_token": "saved" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("fresh", "refresh-2")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("authorization", "Bearer fresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me()))
        .mount(&server)
        .await;

    let user = client.restore("saved").await.expect("restore");

    assert_eq!(user.username, "alice");
    assert_eq!(
        user.lost_and_found_id.0.to_string(),
        "00000000-0000-0000-0000-0000000000f0"
    );
}

/// A browser's refresh token stays in its cookie: the client asks for that at sign-in, never
/// receives the token, and sends none when it refreshes or signs out.
#[tokio::test]
async fn a_cookie_session_never_sends_or_holds_the_refresh_token() {
    let server = MockServer::start().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener: TokenListener = {
        let seen = seen.clone();
        Arc::new(move |token| {
            seen.lock().expect("seen").push(token.map(str::to_owned));
        })
    };
    let client = Client::with_listener(server.uri(), listener).with_cookie_session();
    let access_only = |access: &str| json!({ "access_token": access, "expires_in": 900 });
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_json(
            json!({ "username": "alice", "password": "pw", "cookie": true }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(access_only("old")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(body_json(json!({ "cookie": true })))
        .respond_with(ResponseTemplate::new(200).set_body_json(access_only("new")))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .and(body_json(json!({ "cookie": true })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    client.login("alice", "pw").await.expect("login");
    client.refresh_session("old").await.expect("refresh");
    assert_eq!(client.access_token().as_deref(), Some("new"));
    client.resume(None).await.expect("resume");
    client.logout().await.expect("logout");

    let seen = seen.lock().expect("seen");
    let signed_in = Some(String::new());
    assert_eq!(
        *seen,
        [signed_in.clone(), signed_in.clone(), signed_in, None]
    );
}

/// A token saved before the cookie existed is handed to the server once.
#[tokio::test]
async fn a_cookie_session_hands_over_a_token_saved_earlier() {
    let server = MockServer::start().await;
    let client = Client::new(server.uri()).with_cookie_session();
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(body_json(
            json!({ "refresh_token": "saved", "cookie": true }),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "new", "expires_in": 900 })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me()))
        .mount(&server)
        .await;

    client.resume(Some("saved")).await.expect("resume");
}

#[tokio::test]
async fn error_bodies_are_decoded_into_codes() {
    let server = MockServer::start().await;
    let client = Client::new(server.uri());
    logged_in(&server, &client).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/folders"))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({ "error": "conflict", "message": "name already exists" })),
        )
        .mount(&server)
        .await;

    let result = client.create_folder(None, "docs").await;

    assert_eq!(
        result.expect_err("conflict"),
        ApiError::Api {
            code: ErrorCode::Conflict,
            message: "name already exists".to_owned()
        }
    );
}

#[tokio::test]
async fn a_proxy_error_page_is_a_status_error() {
    let server = MockServer::start().await;
    let client = Client::new(server.uri());
    logged_in(&server, &client).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/folders"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"))
        .mount(&server)
        .await;

    let err = client
        .create_folder(None, "docs")
        .await
        .expect_err("bad gateway");

    assert_eq!(
        err,
        ApiError::Status {
            status: 502,
            reason: "Bad Gateway".to_owned()
        }
    );
    assert!(err.is_unreachable());
    assert_eq!(err.to_string(), "server unreachable (502 Bad Gateway)");
}

#[tokio::test]
async fn calls_without_a_session_are_unauthorized() {
    let server = MockServer::start().await;
    let client = Client::new(server.uri());

    let result = client.list(None).await;

    assert_eq!(result.expect_err("unauthorized"), ApiError::Unauthorized);
}

#[tokio::test]
async fn move_to_root_sends_an_explicit_null_parent() {
    let server = MockServer::start().await;
    let client = Client::new(server.uri());
    logged_in(&server, &client).await;
    let id = protocol::NodeId(uuid::Uuid::nil());
    Mock::given(method("PATCH"))
        .and(path(protocol::paths::node(id)))
        .and(body_json(json!({ "parent_id": null })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id.to_string(), "parent_id": null, "kind": "folder", "name": "x",
            "size": 0, "sha256": null, "mime": "inode/directory", "version": 1,
            "created_at": "1970-01-01T00:00:00Z", "updated_at": "1970-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    client.move_to(id, None).await.expect("move");
}
