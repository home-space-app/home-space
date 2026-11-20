//! A server in host mode keeps its tenants apart: each host signs in only its own users, and
//! nothing one tenant issued works on another's host.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::{HOST_UNAVAILABLE, PASSWORD, TestApp, setup_two_tenants};

/// Signs `username` in with the app's password, returning the token pair.
async fn sign_in(app: &TestApp, username: &str) -> Value {
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({ "username": username, "password": PASSWORD }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{username}");
    response.json()
}

async fn refresh(app: &TestApp, refresh_token: &str) -> StatusCode {
    app.json(
        Method::POST,
        "/api/v1/auth/refresh",
        None,
        json!({ "refresh_token": refresh_token }),
    )
    .await
    .status
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_host_no_tenant_answers_on_is_misdirected(pool: PgPool) {
    let (a, _) = setup_two_tenants(pool).await;
    let unknown = a
        .send_with(
            Method::GET,
            "/api/v1/auth/providers",
            None,
            Body::empty(),
            None,
            &[("host", "unknown.example")],
        )
        .await;
    assert_eq!(unknown.status, StatusCode::MISDIRECTED_REQUEST);

    let closed = a
        .send_with(
            Method::GET,
            "/api/v1/auth/providers",
            None,
            Body::empty(),
            None,
            &[("host", HOST_UNAVAILABLE)],
        )
        .await;
    assert_eq!(closed.status, StatusCode::SERVICE_UNAVAILABLE);
    // Neither names the tenants the server does answer for.
    for body in [&unknown.body, &closed.body] {
        let text = String::from_utf8_lossy(body);
        assert!(
            !text.contains("Tenant") && !text.contains("example.com"),
            "{text}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn health_answers_on_any_host(pool: PgPool) {
    let (a, _) = setup_two_tenants(pool).await;
    let response = a
        .send_with(
            Method::GET,
            "/api/v1/health",
            None,
            Body::empty(),
            None,
            &[("host", "10.0.0.5:8080")],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn each_host_signs_in_only_its_own_users(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    a.auth.create_user("alice", PASSWORD).await.expect("alice");
    b.auth.create_user("bob", PASSWORD).await.expect("bob");

    sign_in(&a, "alice").await;
    sign_in(&b, "bob").await;
    for (app, username) in [(&b, "alice"), (&a, "bob")] {
        let response = app
            .json(
                Method::POST,
                "/api/v1/auth/login",
                None,
                json!({ "username": username, "password": PASSWORD }),
            )
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{username}");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_same_username_is_a_different_user_on_each_host(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let on_a = a.user_token("alice").await;
    let on_b = b.user_token("alice").await;

    let me_a = a.get("/api/v1/me", &on_a).await.json();
    let me_b = b.get("/api/v1/me", &on_b).await.json();
    assert_eq!(me_a["username"], "alice");
    assert_eq!(me_b["username"], "alice");
    assert_ne!(me_a["id"], me_b["id"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_access_token_works_only_on_its_tenants_host(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let token = a.user_token("alice").await;
    // An alice of the other tenant exists too, so only the token's tenant can tell them apart.
    b.user_token("alice").await;

    assert_eq!(a.get("/api/v1/me", &token).await.status, StatusCode::OK);
    assert_eq!(
        b.get("/api/v1/me", &token).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_refresh_token_sent_to_another_host_is_refused_and_the_session_lives_on(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    a.auth.create_user("alice", PASSWORD).await.expect("alice");
    let pair = sign_in(&a, "alice").await;
    let refresh_token = pair["refresh_token"].as_str().expect("refresh token");

    assert_eq!(refresh(&b, refresh_token).await, StatusCode::UNAUTHORIZED);
    // Had the other host counted it as a reused token, the whole session would have ended.
    assert_eq!(refresh(&a, refresh_token).await, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn signing_out_on_another_host_ends_nothing(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    a.auth.create_user("alice", PASSWORD).await.expect("alice");
    let pair = sign_in(&a, "alice").await;
    let refresh_token = pair["refresh_token"].as_str().expect("refresh token");
    let access = pair["access_token"].as_str().expect("access token");

    let response = b
        .json(
            Method::POST,
            "/api/v1/auth/logout",
            None,
            json!({ "refresh_token": refresh_token }),
        )
        .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(a.get("/api/v1/me", access).await.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_ticket_works_only_on_its_tenants_host(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let token = a.user_token("alice").await;
    let id = a.upload(&token, "name=a.txt", "hello").await.json()["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let ticket = a
        .json(
            Method::POST,
            "/api/v1/tickets",
            Some(&token),
            json!({ "scope": "download", "node_id": id }),
        )
        .await
        .json()["ticket"]
        .as_str()
        .expect("ticket")
        .to_owned();
    let uri = format!("/api/v1/files/{id}/content?ticket={ticket}");

    let elsewhere = b.send(Method::GET, &uri, None, Body::empty(), None).await;
    assert_eq!(elsewhere.status, StatusCode::UNAUTHORIZED);
    // Refused before it was counted as used, so it still works where it was issued.
    let at_home = a.send(Method::GET, &uri, None, Body::empty(), None).await;
    assert_eq!(
        (at_home.status, at_home.body.as_slice()),
        (StatusCode::OK, &b"hello"[..])
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_browser_on_one_tenants_page_cannot_use_another_host(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    a.auth.create_user("alice", PASSWORD).await.expect("alice");
    b.auth.create_user("alice", PASSWORD).await.expect("alice");
    let from_page_of_a = |app: &TestApp| {
        let app = app.clone();
        async move {
            app.send_with(
                Method::POST,
                "/api/v1/auth/login",
                None,
                Body::from(
                    json!({ "username": "alice", "password": PASSWORD, "cookie": true })
                        .to_string(),
                ),
                Some("application/json"),
                &[("origin", "https://files.example.com")],
            )
            .await
            .status
        }
    };
    assert_eq!(from_page_of_a(&a).await, StatusCode::OK);
    assert_eq!(from_page_of_a(&b).await, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_user_picker_lists_only_users_of_the_same_tenant(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let alice = a.user_token("alice").await;
    a.user_token("bob").await;
    b.user_token("mallory").await;

    let listed = a.get("/api/v1/users", &alice).await.json();
    let names = listed
        .as_array()
        .expect("users")
        .iter()
        .map(|user| user["username"].as_str().expect("name").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["bob"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_shown_to_everyone_is_hidden_from_other_tenants(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let laptop = Uuid::now_v7();
    let alice = a.sign_in_from("alice", laptop, "Laptop").await;
    let mallory = b.sign_in_from("mallory", Uuid::now_v7(), "Phone").await;
    let shown = a
        .json(
            Method::PUT,
            &format!("/api/v1/devices/{laptop}/visibility"),
            Some(&alice),
            json!({ "visibility": "everyone" }),
        )
        .await;
    assert_eq!(shown.status, StatusCode::NO_CONTENT);

    let targets = b.get("/api/v1/devices/targets", &mallory).await.json();
    assert!(
        targets
            .as_array()
            .expect("targets")
            .iter()
            .all(|target| target["user"]["username"] == "mallory"),
        "{targets}"
    );
    let sent = b
        .json(
            Method::POST,
            "/api/v1/transfers",
            Some(&mallory),
            json!({
                "recipient_id": a.user_id("alice").await,
                "recipient_device": laptop,
                "items": [{ "path": "a.txt", "kind": "file", "size": 1 }],
            }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_cannot_be_shown_to_a_user_of_another_tenant(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let laptop = Uuid::now_v7();
    let alice = a.sign_in_from("alice", laptop, "Laptop").await;
    b.user_token("mallory").await;

    let shown = a
        .json(
            Method::PUT,
            &format!("/api/v1/devices/{laptop}/visibility"),
            Some(&alice),
            json!({ "visibility": "users", "users": [b.user_id("mallory").await] }),
        )
        .await;
    assert_eq!(shown.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../../migrations")]
async fn files_cannot_be_shared_with_a_user_of_another_tenant(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let alice = a.user_token("alice").await;
    let mallory = b.user_token("mallory").await;
    let report = a.upload(&alice, "name=report.pdf", "pdf").await.json()["id"].clone();

    let shared = a
        .json(
            Method::POST,
            "/api/v1/shares",
            Some(&alice),
            json!({ "node_ids": [report], "recipients": [b.user_id("mallory").await] }),
        )
        .await;
    assert_eq!(shared.status, StatusCode::NOT_FOUND);
    assert_eq!(
        b.get("/api/v1/shared", &mallory).await.json(),
        json!([]),
        "nothing was shared with mallory"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_share_link_opens_only_on_its_tenants_host(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let alice = a.user_token("alice").await;
    let report = a.upload(&alice, "name=report.pdf", "pdf").await.json()["id"].clone();
    let link = a
        .json(
            Method::POST,
            "/api/v1/shares",
            Some(&alice),
            json!({ "node_ids": [report] }),
        )
        .await;
    assert_eq!(link.status, StatusCode::CREATED);
    let token = link.json()["token"].as_str().expect("token").to_owned();
    let uri = format!("/api/v1/public/shares/{token}");

    let at_home = a.send(Method::GET, &uri, None, Body::empty(), None).await;
    assert_eq!(at_home.status, StatusCode::OK);
    let elsewhere = b.send(Method::GET, &uri, None, Body::empty(), None).await;
    assert_eq!(elsewhere.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_administrator_manages_only_the_accounts_of_their_own_tenant(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    a.user_token("alice").await;
    let alice = a.user_id("alice").await;
    let admin = b.user_token("bob").await;
    let bob = b.user_id("bob").await;
    b.db.set_admin(bob, true).await.expect("admin");

    let listed = b.get("/api/v1/admin/users", &admin).await.json();
    assert!(only_bob(&listed), "{listed}");

    let path = format!("/api/v1/admin/users/{alice}");
    assert_eq!(b.get(&path, &admin).await.status, StatusCode::NOT_FOUND);
    let disable = b
        .json(
            Method::PATCH,
            &path,
            Some(&admin),
            json!({ "disabled": true }),
        )
        .await;
    assert_eq!(disable.status, StatusCode::NOT_FOUND);
    let delete = b
        .send(Method::DELETE, &path, Some(&admin), Body::empty(), None)
        .await;
    assert_eq!(delete.status, StatusCode::NOT_FOUND);
    // Alice was left alone: she still signs in.
    sign_in(&a, "alice").await;

    let status = b.get("/api/v1/admin/status", &admin).await.json();
    assert!(only_bob(&status["users"]), "{status}");
}

/// Whether a list of accounts holds tenant B's administrator alone.
fn only_bob(accounts: &Value) -> bool {
    let names: Vec<&str> = accounts
        .as_array()
        .expect("accounts")
        .iter()
        .filter_map(|account| account["username"].as_str())
        .collect();
    names == ["bob"]
}
