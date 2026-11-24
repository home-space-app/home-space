use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method},
};

use api_client::{Client, user_agent};

#[test]
fn the_user_agent_names_the_version_and_the_system() {
    assert_eq!(
        user_agent(),
        format!(
            "home-space/{} ({}; {})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    );
}

#[tokio::test]
async fn requests_send_the_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("user-agent", user_agent()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "ok" })))
        .expect(1)
        .mount(&server)
        .await;

    Client::new(server.uri()).health().await.expect("health");
}
