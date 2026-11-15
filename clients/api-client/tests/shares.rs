use serde_json::{Value, json};
use time::macros::datetime;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path, query_param},
};

use api_client::Client;
use protocol::{CreateShare, NodeId, UpdateShare, paths};

const SHARE_ID: &str = "00000000-0000-0000-0000-0000000000a1";
const NODE_ID: &str = "00000000-0000-0000-0000-000000000002";
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn share_body(has_password: bool, expires_at: Value) -> Value {
    json!({
        "id": SHARE_ID,
        "token": TOKEN,
        "items": [],
        "has_password": has_password,
        "permission": "read",
        "expires_at": expires_at,
        "created_at": "2026-11-15T10:00:00Z",
    })
}

async fn signed_in(server: &MockServer) -> Client {
    Mock::given(method("POST"))
        .and(path(paths::LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "access_token": "access", "refresh_token": "refresh", "expires_in": 900 }),
        ))
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
    client.login("alice", "pw").await.expect("login");
    client
}

fn share_id() -> Uuid {
    SHARE_ID.parse().expect("id")
}

fn node_id() -> NodeId {
    NODE_ID.parse().expect("id")
}

#[tokio::test]
async fn create_share_sends_the_items_password_and_expiry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::SHARES))
        .and(body_json(json!({
            "node_ids": [NODE_ID],
            "password": "secret-pw",
            "expires_at": "2026-11-22T10:00:00Z",
        })))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(share_body(true, json!("2026-11-22T10:00:00Z"))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server).await;

    let share = client
        .create_share(&CreateShare {
            node_ids: vec![node_id()],
            password: Some("secret-pw".to_owned()),
            expires_at: Some(datetime!(2026-11-22 10:00 UTC)),
        })
        .await
        .expect("create");

    assert_eq!(share.token, TOKEN);
    assert!(share.has_password);
}

#[tokio::test]
async fn shares_asks_for_those_of_one_node() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::SHARES))
        .and(query_param("node_id", NODE_ID))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([share_body(false, Value::Null)])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server).await;

    let shares = client.shares(Some(node_id())).await.expect("shares");

    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].expires_at, None);
}

#[tokio::test]
async fn update_share_sends_only_what_changes() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(paths::share(share_id())))
        .and(body_json(json!({ "password": null })))
        .respond_with(ResponseTemplate::new(200).set_body_json(share_body(false, Value::Null)))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server).await;

    let share = client
        .update_share(
            share_id(),
            &UpdateShare {
                password: Some(None),
                expires_at: None,
            },
        )
        .await
        .expect("update");

    assert!(!share.has_password);
}

#[tokio::test]
async fn delete_share_stops_sharing() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(paths::share(share_id())))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server).await;

    client.delete_share(share_id()).await.expect("delete");
}

#[tokio::test]
async fn share_url_points_at_the_web_page_of_the_link() {
    let client = Client::new("https://files.example");

    assert_eq!(
        client.share_url(TOKEN),
        format!("https://files.example/s/{TOKEN}")
    );
}
