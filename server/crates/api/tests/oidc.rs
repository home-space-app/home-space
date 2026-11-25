mod common;

use std::collections::HashMap;

use axum::http::{Method, StatusCode};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;

use auth::{OidcConfig, OidcService};
use common::{
    PASSWORD, Response, TestApp,
    idp::{CLIENT_ID, FakeIdp, IdToken},
    setup, setup_with,
};

const PUBLIC_URL: &str = "https://files.example.com";
const WEB_REDIRECT: &str = "https://files.example.com/login/callback";
const LOOPBACK_REDIRECT: &str = "http://127.0.0.1:54321/callback";
const VERIFIER: &str = "0123456789abcdef0123456789abcdef0123456789abc";
const PROVIDER_CODE: &str = "provider-authorization-code";

fn challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))
}

#[derive(Clone, Copy)]
struct Options {
    required_group: Option<&'static str>,
    password_login: bool,
}

const DEFAULTS: Options = Options {
    required_group: None,
    password_login: true,
};

async fn setup_oidc(pool: PgPool, idp: &FakeIdp, options: Options) -> TestApp {
    let oidc = OidcService::discover(OidcConfig {
        issuer: idp.issuer(),
        client_id: CLIENT_ID.to_owned(),
        client_secret: Some("client-secret".to_owned()),
        scopes: ["openid", "profile", "email"].map(str::to_owned).to_vec(),
        public_url: PUBLIC_URL.to_owned(),
        required_group: options.required_group.map(str::to_owned),
        groups_claim: "groups".to_owned(),
        display_name: "Test IdP".to_owned(),
    })
    .await
    .expect("discover provider");
    setup_with(pool, |auth| auth.with_oidc(oidc, options.password_login)).await
}

struct Started {
    state: String,
    nonce: String,
}

async fn request_start(app: &TestApp, redirect: &str, challenge: &str) -> Response {
    let url = Url::parse_with_params(
        "http://localhost/api/v1/auth/oidc/start",
        [("redirect_uri", redirect), ("code_challenge", challenge)],
    )
    .expect("url");
    let uri = format!("{}?{}", url.path(), url.query().expect("query"));
    app.send(Method::GET, &uri, None, Default::default(), None)
        .await
}

/// Starts a sign-in and returns what the provider was told.
async fn start(app: &TestApp, redirect: &str) -> Started {
    let response = request_start(app, redirect, &challenge()).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let location = Url::parse(&response.location()).expect("provider url");
    assert!(location.as_str().contains("/authorize"));
    let query: HashMap<String, String> = location.query_pairs().into_owned().collect();
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], CLIENT_ID);
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(
        query["redirect_uri"],
        format!("{PUBLIC_URL}/api/v1/auth/oidc/callback")
    );
    let scopes: Vec<&str> = query["scope"].split(' ').collect();
    assert_eq!(scopes, ["openid", "profile", "email"]);
    Started {
        state: query["state"].clone(),
        nonce: query["nonce"].clone(),
    }
}

async fn callback(app: &TestApp, params: &[(&str, &str)]) -> Response {
    let url =
        Url::parse_with_params("http://localhost/api/v1/auth/oidc/callback", params).expect("url");
    let uri = format!("{}?{}", url.path(), url.query().expect("query"));
    app.send(Method::GET, &uri, None, Default::default(), None)
        .await
}

async fn exchange(app: &TestApp, code: &str, verifier: &str) -> Response {
    app.json(
        Method::POST,
        "/api/v1/auth/oidc/exchange",
        None,
        json!({ "code": code, "code_verifier": verifier }),
    )
    .await
}

/// The client redirect a callback answered with: its base URL and query parameters.
fn client_redirect(response: &Response) -> (String, HashMap<String, String>) {
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let mut url = Url::parse(&response.location()).expect("client url");
    let params = url.query_pairs().into_owned().collect();
    url.set_query(None);
    (url.to_string(), params)
}

/// Runs a whole sign-in at the provider and returns the callback's response.
async fn sign_in(
    app: &TestApp,
    idp: &FakeIdp,
    redirect: &str,
    token: impl FnOnce(&str) -> IdToken,
) -> Response {
    let started = start(app, redirect).await;
    idp.respond_with(token(&started.nonce)).await;
    callback(app, &[("state", &started.state), ("code", PROVIDER_CODE)]).await
}

/// Signs in and exchanges the login code, returning the `/me` document.
async fn sign_in_and_get_me(
    app: &TestApp,
    idp: &FakeIdp,
    token: impl FnOnce(&str) -> IdToken,
) -> Value {
    let response = sign_in(app, idp, WEB_REDIRECT, token).await;
    let (_, params) = client_redirect(&response);
    let code = params.get("code").expect("login code");
    let tokens = exchange(app, code, VERIFIER).await;
    assert_eq!(tokens.status, StatusCode::OK);
    let access = tokens.json()["access_token"]
        .as_str()
        .expect("token")
        .to_owned();
    let me = app.get("/api/v1/me", &access).await;
    assert_eq!(me.status, StatusCode::OK);
    me.json()
}

#[sqlx::test(migrations = "../../migrations")]
async fn providers_lists_password_login_without_oidc(pool: PgPool) {
    let app = setup(pool).await;
    let response = app
        .send(
            Method::GET,
            "/api/v1/auth/providers",
            None,
            Default::default(),
            None,
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.json(),
        json!({ "password": true, "oidc": null, "browser_login": false })
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn providers_lists_the_configured_provider(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(
        pool,
        &idp,
        Options {
            password_login: false,
            ..DEFAULTS
        },
    )
    .await;
    let response = app
        .send(
            Method::GET,
            "/api/v1/auth/providers",
            None,
            Default::default(),
            None,
        )
        .await;
    assert_eq!(
        response.json(),
        json!({
            "password": false,
            "oidc": { "name": "Test IdP", "browser": false },
            "browser_login": false,
        })
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn oidc_endpoints_are_not_found_when_disabled(pool: PgPool) {
    let app = setup(pool).await;
    let response = request_start(&app, WEB_REDIRECT, &challenge()).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let response = exchange(&app, "code", VERIFIER).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn first_sign_in_creates_a_user_and_later_ones_reuse_it(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let token = |nonce: &str| {
        IdToken::new("subject-1", nonce).with(json!({
            "preferred_username": "alice",
            "email": "alice@example.com",
        }))
    };

    let first = sign_in_and_get_me(&app, &idp, token).await;
    let second = sign_in_and_get_me(&app, &idp, token).await;

    assert_eq!(first["username"], "alice");
    assert_eq!(first["id"], second["id"]);
    assert_eq!(
        app.db
            .list_users(db::TenantId::DEFAULT)
            .await
            .expect("users")
            .len(),
        1
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn desktop_clients_can_use_a_loopback_redirect(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, LOOPBACK_REDIRECT, |nonce| {
        IdToken::new("subject-1", nonce)
    })
    .await;
    let (base, params) = client_redirect(&response);
    assert_eq!(base, LOOPBACK_REDIRECT);
    assert!(params.contains_key("code"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn mobile_apps_can_use_their_redirect(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, protocol::paths::MOBILE_OIDC_REDIRECT, |nonce| {
        IdToken::new("subject-1", nonce)
    })
    .await;
    let (base, params) = client_redirect(&response);
    assert_eq!(base, protocol::paths::MOBILE_OIDC_REDIRECT);

    let exchanged = exchange(&app, &params["code"], VERIFIER).await;
    assert_eq!(exchanged.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn callback_keeps_the_code_out_of_caches_and_referrers(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, WEB_REDIRECT, |nonce| IdToken::new("s", nonce)).await;
    assert_eq!(response.headers["referrer-policy"], "no-referrer");
    assert_eq!(response.headers["cache-control"], "no-store");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_taken_username_gets_a_numbered_suffix(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    app.auth
        .create_user("alice", PASSWORD)
        .await
        .expect("existing user");

    let me = sign_in_and_get_me(&app, &idp, |nonce| {
        IdToken::new("subject-1", nonce).with(json!({ "preferred_username": "alice" }))
    })
    .await;

    assert_eq!(me["username"], "alice-2");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_username_falls_back_to_the_email_then_the_subject(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;

    let by_email = sign_in_and_get_me(&app, &idp, |nonce| {
        IdToken::new("subject-1", nonce).with(json!({ "email": "bob@example.com" }))
    })
    .await;
    let by_subject = sign_in_and_get_me(&app, &idp, |nonce| IdToken::new("subject-2", nonce)).await;

    assert_eq!(by_email["username"], "bob");
    assert_eq!(by_subject["username"], "subject-2");
}

#[sqlx::test(migrations = "../../migrations")]
async fn users_are_matched_on_the_subject_not_the_email(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let alice = sign_in_and_get_me(&app, &idp, |nonce| {
        IdToken::new("subject-1", nonce)
            .with(json!({ "preferred_username": "alice", "email": "shared@example.com" }))
    })
    .await;
    let other = sign_in_and_get_me(&app, &idp, |nonce| {
        IdToken::new("subject-2", nonce)
            .with(json!({ "preferred_username": "mallory", "email": "shared@example.com" }))
    })
    .await;
    assert_ne!(alice["id"], other["id"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_wrong_nonce_is_rejected(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, WEB_REDIRECT, |_| {
        IdToken::new("s", "someone-elses")
    })
    .await;
    let (_, params) = client_redirect(&response);
    assert_eq!(params["error"], "login_failed");
    assert!(!params.contains_key("code"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_token_for_another_audience_is_rejected(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, WEB_REDIRECT, |nonce| IdToken {
        audience: "another-client",
        ..IdToken::new("s", nonce)
    })
    .await;
    let (_, params) = client_redirect(&response);
    assert_eq!(params["error"], "login_failed");
    assert!(
        app.db
            .list_users(db::TenantId::DEFAULT)
            .await
            .expect("users")
            .is_empty()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_token_is_rejected(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, WEB_REDIRECT, |nonce| IdToken {
        expires_in: -3600,
        ..IdToken::new("s", nonce)
    })
    .await;
    let (_, params) = client_redirect(&response);
    assert_eq!(params["error"], "login_failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_rejected_provider_code_fails_the_sign_in(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let started = start(&app, WEB_REDIRECT).await;
    idp.reject_codes().await;
    let response = callback(&app, &[("state", &started.state), ("code", "bad")]).await;
    let (_, params) = client_redirect(&response);
    assert_eq!(params["error"], "login_failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_provider_error_is_passed_to_the_client(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let started = start(&app, WEB_REDIRECT).await;
    let response = callback(
        &app,
        &[("state", &started.state), ("error", "access_denied")],
    )
    .await;
    let (base, params) = client_redirect(&response);
    assert_eq!(base, WEB_REDIRECT);
    assert_eq!(params["error"], "access_denied");
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_members_of_the_required_group_may_sign_in(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let options = Options {
        required_group: Some("family"),
        ..DEFAULTS
    };
    let app = setup_oidc(pool, &idp, options).await;

    let outsider = sign_in(&app, &idp, WEB_REDIRECT, |nonce| {
        IdToken::new("outsider", nonce).with(json!({ "groups": ["friends"] }))
    })
    .await;
    let nobody = sign_in(&app, &idp, WEB_REDIRECT, |nonce| {
        IdToken::new("nobody", nonce)
    })
    .await;
    assert_eq!(client_redirect(&outsider).1["error"], "access_denied");
    assert_eq!(client_redirect(&nobody).1["error"], "access_denied");
    assert!(
        app.db
            .list_users(db::TenantId::DEFAULT)
            .await
            .expect("users")
            .is_empty()
    );

    // Keycloak reports group paths with a leading slash.
    let member = sign_in_and_get_me(&app, &idp, |nonce| {
        IdToken::new("member", nonce).with(json!({ "groups": ["/family"] }))
    })
    .await;
    assert_eq!(member["username"], "member");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_login_code_works_only_once(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, WEB_REDIRECT, |nonce| IdToken::new("s", nonce)).await;
    let code = client_redirect(&response).1["code"].clone();

    assert_eq!(exchange(&app, &code, VERIFIER).await.status, StatusCode::OK);
    assert_eq!(
        exchange(&app, &code, VERIFIER).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_login_code_needs_the_matching_verifier(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, WEB_REDIRECT, |nonce| IdToken::new("s", nonce)).await;
    let code = client_redirect(&response).1["code"].clone();

    let wrong = "z".repeat(50);
    assert_eq!(
        exchange(&app, &code, &wrong).await.status,
        StatusCode::UNAUTHORIZED
    );
    // A failed attempt burns the code, so an intercepted code cannot be guessed against.
    assert_eq!(
        exchange(&app, &code, VERIFIER).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_login_code_is_unauthorized(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    assert_eq!(
        exchange(&app, "made-up", VERIFIER).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_state_works_only_once(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let started = start(&app, WEB_REDIRECT).await;
    idp.respond_with(IdToken::new("s", &started.nonce)).await;
    let params = [("state", started.state.as_str()), ("code", PROVIDER_CODE)];

    assert!(
        client_redirect(&callback(&app, &params).await)
            .1
            .contains_key("code")
    );
    // A provider resuming the same sign-in again, e.g. after the back button, lands on the web
    // client's error page instead of a bare API error, and no code is handed out.
    let (base, again) = client_redirect(&callback(&app, &params).await);
    assert_eq!(base, WEB_REDIRECT);
    assert_eq!(
        again.get("error").map(String::as_str),
        Some("login_expired")
    );
    assert!(!again.contains_key("code"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_state_sends_the_browser_to_the_web_client(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let forged = callback(&app, &[("state", "forged"), ("code", PROVIDER_CODE)]).await;
    let missing = callback(&app, &[("error", "access_denied")]).await;
    for response in [forged, missing] {
        let (base, params) = client_redirect(&response);
        assert_eq!(base, WEB_REDIRECT);
        assert_eq!(params.len(), 1);
        assert_eq!(params["error"], "login_expired");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn redirects_to_other_sites_are_rejected(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    for redirect in [
        "https://evil.example.com/login/callback",
        "https://files.example.com/somewhere-else",
        "http://localhost:1/callback",
        "javascript:alert(1)",
    ] {
        let response = request_start(&app, redirect, &challenge()).await;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{redirect}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_malformed_challenge_is_rejected(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = request_start(&app, WEB_REDIRECT, "too-short").await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../../migrations")]
async fn password_login_can_be_turned_off(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let options = Options {
        password_login: false,
        ..DEFAULTS
    };
    let app = setup_oidc(pool, &idp, options).await;
    app.auth.create_user("alice", PASSWORD).await.expect("user");
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({ "username": "alice", "password": PASSWORD }),
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn password_login_still_works_alongside_oidc(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    assert!(!app.user_token("alice").await.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn oidc_users_cannot_log_in_with_a_password(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    sign_in_and_get_me(&app, &idp, |nonce| {
        IdToken::new("subject-1", nonce).with(json!({ "preferred_username": "alice" }))
    })
    .await;
    for password in [PASSWORD, ""] {
        let response = app
            .json(
                Method::POST,
                "/api/v1/auth/login",
                None,
                json!({ "username": "alice", "password": password }),
            )
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn oidc_sessions_can_be_refreshed_and_logged_out(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let app = setup_oidc(pool, &idp, DEFAULTS).await;
    let response = sign_in(&app, &idp, WEB_REDIRECT, |nonce| IdToken::new("s", nonce)).await;
    let code = client_redirect(&response).1["code"].clone();
    let tokens = exchange(&app, &code, VERIFIER).await.json();
    let refresh = tokens["refresh_token"].as_str().expect("refresh token");

    let refreshed = app
        .json(
            Method::POST,
            "/api/v1/auth/refresh",
            None,
            json!({ "refresh_token": refresh }),
        )
        .await;
    assert_eq!(refreshed.status, StatusCode::OK);
    let next = refreshed.json()["refresh_token"]
        .as_str()
        .expect("refresh token")
        .to_owned();

    let logout = app
        .json(
            Method::POST,
            "/api/v1/auth/logout",
            None,
            json!({ "refresh_token": next }),
        )
        .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_provider_only_tenant_sends_app_browser_sign_ins_to_the_provider(pool: PgPool) {
    let idp = FakeIdp::start().await;
    let web = tempfile::tempdir().expect("tempdir");
    let options = Options {
        password_login: false,
        ..DEFAULTS
    };
    let app = setup_oidc(pool, &idp, options).await.with_web(&web);
    let providers = app
        .send(
            Method::GET,
            "/api/v1/auth/providers",
            None,
            Default::default(),
            None,
        )
        .await;
    assert_eq!(providers.json()["oidc"]["name"], "Test IdP");
    assert_eq!(providers.json()["browser_login"], true);

    let url = Url::parse_with_params(
        "http://localhost/api/v1/auth/browser/start",
        [
            ("redirect_uri", "http://127.0.0.1:54321/callback"),
            ("code_challenge", challenge().as_str()),
        ],
    )
    .expect("url");
    let uri = format!("{}?{}", url.path(), url.query().expect("query"));
    let response = app
        .send(Method::GET, &uri, None, Default::default(), None)
        .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let location = Url::parse(&response.location()).expect("provider url");
    let query: HashMap<String, String> = location.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], CLIENT_ID);

    // The provider's answer comes back to the app, which trades it where it started.
    idp.respond_with(IdToken::new("subject-1", &query["nonce"]))
        .await;
    let back = callback(&app, &[("state", &query["state"]), ("code", PROVIDER_CODE)]).await;
    let (base, params) = client_redirect(&back);
    assert_eq!(base, "http://127.0.0.1:54321/callback");
    let tokens = app
        .json(
            Method::POST,
            "/api/v1/auth/browser/exchange",
            None,
            json!({ "code": params["code"], "code_verifier": VERIFIER }),
        )
        .await;
    assert_eq!(tokens.status, StatusCode::OK, "{:?}", tokens.json());
}
