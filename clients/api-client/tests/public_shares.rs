use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

use api_client::{ApiError, Client};
use protocol::{NodeId, paths};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const ACCESS: &str = "share-access";
const FOLDER_ID: &str = "00000000-0000-0000-0000-000000000003";

fn folder_id() -> NodeId {
    FOLDER_ID.parse().expect("id")
}

fn shared_folder() -> serde_json::Value {
    json!({
        "id": FOLDER_ID,
        "kind": "folder",
        "name": "Photos",
        "size": 0,
        "mime": "",
        "updated_at": "2026-11-15T10:00:00Z",
    })
}

#[tokio::test]
async fn public_share_needs_no_sign_in() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::public_share(TOKEN)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "owner": "sam",
            "item_count": 3,
            "expires_at": null,
            "needs_password": true,
        })))
        .mount(&server)
        .await;

    let share = Client::new(server.uri())
        .public_share(TOKEN, None)
        .await
        .expect("share");

    assert!(share.needs_password);
    assert_eq!(share.items, None);
}

#[tokio::test]
async fn public_share_sends_the_access_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::public_share(TOKEN)))
        .and(header(paths::SHARE_ACCESS, ACCESS))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "owner": "sam",
            "item_count": 1,
            "expires_at": null,
            "needs_password": false,
            "items": [shared_folder()],
        })))
        .mount(&server)
        .await;

    let share = Client::new(server.uri())
        .public_share(TOKEN, Some(ACCESS))
        .await
        .expect("share");

    assert_eq!(share.items.map(|items| items.len()), Some(1));
}

#[tokio::test]
async fn an_expired_link_is_gone() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::public_share(TOKEN)))
        .respond_with(
            ResponseTemplate::new(410).set_body_json(json!({ "error": "gone", "message": "gone" })),
        )
        .mount(&server)
        .await;

    let error = Client::new(server.uri())
        .public_share(TOKEN, None)
        .await
        .expect_err("gone");

    assert!(matches!(
        error,
        ApiError::Api {
            code: protocol::ErrorCode::Gone,
            ..
        }
    ));
}

#[tokio::test]
async fn unlock_share_sends_the_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::public_share_unlock(TOKEN)))
        .and(body_json(json!({ "password": "secret" })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access": ACCESS, "expires_in": 43_200 })),
        )
        .mount(&server)
        .await;

    let access = Client::new(server.uri())
        .unlock_share(TOKEN, "secret")
        .await
        .expect("unlocked");

    assert_eq!(access.access, ACCESS);
}

#[tokio::test]
async fn a_wrong_password_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::public_share_unlock(TOKEN)))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(
                json!({ "error": "unauthorized", "message": "invalid credentials" }),
            ),
        )
        .mount(&server)
        .await;

    let error = Client::new(server.uri())
        .unlock_share(TOKEN, "wrong")
        .await
        .expect_err("refused");

    assert!(matches!(error, ApiError::Unauthorized));
}

#[tokio::test]
async fn shared_folder_lists_the_folder_with_the_access_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::public_share_nodes(TOKEN)))
        .and(query_param("parent_id", FOLDER_ID))
        .and(header(paths::SHARE_ACCESS, ACCESS))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([shared_folder()])))
        .mount(&server)
        .await;

    let children = Client::new(server.uri())
        .shared_folder(TOKEN, folder_id(), Some(ACCESS))
        .await
        .expect("children");

    assert_eq!(children[0].name, "Photos");
}

#[test]
fn shared_file_url_carries_the_access_token() {
    let client = Client::new("https://home.example");

    let url = client.shared_file_url(TOKEN, folder_id(), Some(ACCESS));

    assert_eq!(
        url,
        format!(
            "https://home.example{}?{}={ACCESS}",
            paths::public_share_file_content(TOKEN, folder_id()),
            paths::SHARE_ACCESS_PARAM
        )
    );
}

#[test]
fn shared_file_url_of_an_open_link_has_no_query() {
    let client = Client::new("https://home.example");

    let url = client.shared_file_url(TOKEN, folder_id(), None);

    assert!(!url.contains('?'));
}

#[test]
fn shared_archive_url_of_the_whole_link_has_no_query() {
    let client = Client::new("https://home.example");

    let url = client.shared_archive_url(TOKEN, None, None);

    assert_eq!(
        url,
        format!("https://home.example{}", paths::public_share_archive(TOKEN))
    );
}

#[test]
fn shared_archive_url_names_the_folder_and_the_access_token() {
    let client = Client::new("https://home.example");

    let url = client.shared_archive_url(TOKEN, Some(folder_id()), Some(ACCESS));

    assert_eq!(
        url,
        format!(
            "https://home.example{}?{}={}&{}={ACCESS}",
            paths::public_share_archive(TOKEN),
            paths::SHARE_FOLDER_PARAM,
            folder_id(),
            paths::SHARE_ACCESS_PARAM
        )
    );
}
