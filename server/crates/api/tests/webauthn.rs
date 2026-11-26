//! Passkeys and security keys: adding them, signing in with them after the password or on
//! their own, and confirming it is the user with one.

#[path = "webauthn/authenticator.rs"]
mod authenticator;
mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use serde_json::{Value, json};
use sqlx::PgPool;

use authenticator::Authenticator;
use common::{HOST_A, HOST_B, PASSWORD, TestApp, setup, setup_two_tenants};

const ORIGIN: &str = "https://files.example.com";

/// Sends `payload` from a page of `origin` on `host`.
async fn from_page(
    app: &TestApp,
    (host, origin): (&str, &str),
    method: Method,
    uri: &str,
    token: Option<&str>,
    payload: &Value,
) -> common::Response {
    app.send_with(
        method,
        uri,
        token,
        Body::from(payload.to_string()),
        Some("application/json"),
        &[("host", host), ("origin", origin)],
    )
    .await
}

async fn post(app: &TestApp, uri: &str, token: Option<&str>, payload: Value) -> common::Response {
    from_page(app, (HOST_A, ORIGIN), Method::POST, uri, token, &payload).await
}

fn problem(response: &common::Response) -> String {
    response.json()["type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Adds a key of `kind` made by `device` for the user of `token`: returns what the server
/// answered.
async fn add_key(app: &TestApp, token: &str, device: &mut Authenticator, kind: &str) -> Value {
    let started = post(
        app,
        "/api/v1/me/webauthn/start",
        Some(token),
        json!({ "kind": kind }),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.json());
    let challenge = started.json();
    let credential = device.register(ORIGIN, &challenge["options"]);
    let finished = post(
        app,
        "/api/v1/me/webauthn/finish",
        Some(token),
        json!({ "challenge_id": challenge["challenge_id"], "credential": credential }),
    )
    .await;
    assert_eq!(finished.status, StatusCode::OK, "{}", finished.json());
    finished.json()
}

/// Signs in with the password asking for two steps: returns the login token and the methods.
async fn first_step(app: &TestApp, username: &str) -> (String, Value) {
    let response = post(
        app,
        "/api/v1/auth/login",
        None,
        json!({ "username": username, "password": PASSWORD, "two_step": true }),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "{}",
        response.json()
    );
    let body = response.json();
    (
        body["login_token"].as_str().expect("token").to_owned(),
        body["methods"].clone(),
    )
}

/// Answers the second step with `device`.
async fn second_step(
    app: &TestApp,
    login_token: &str,
    device: &mut Authenticator,
) -> common::Response {
    let options = post(
        app,
        "/api/v1/auth/login/webauthn",
        None,
        json!({ "login_token": login_token }),
    )
    .await;
    assert_eq!(options.status, StatusCode::OK, "{}", options.json());
    let answer = device
        .sign(ORIGIN, &options.json()["options"])
        .expect("the device has a credential");
    post(
        app,
        "/api/v1/auth/login/verify",
        None,
        json!({ "login_token": login_token, "webauthn": answer }),
    )
    .await
}

async fn passkey_sign_in(app: &TestApp, device: &mut Authenticator) -> common::Response {
    let started = post(app, "/api/v1/auth/passkey/start", None, json!({})).await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.json());
    let challenge = started.json();
    let answer = device
        .sign(ORIGIN, &challenge["options"])
        .expect("the device has a passkey");
    post(
        app,
        "/api/v1/auth/passkey/finish",
        None,
        json!({ "challenge_id": challenge["challenge_id"], "credential": answer }),
    )
    .await
}

async fn session_methods(pool: &PgPool) -> Vec<Vec<String>> {
    sqlx::query_scalar("SELECT auth_methods FROM sessions ORDER BY created_at")
        .fetch_all(pool)
        .await
        .expect("sessions")
}

async fn sign_in_long_ago(pool: &PgPool) {
    sqlx::query("UPDATE sessions SET authenticated_at = now() - interval '1 hour'")
        .execute(pool)
        .await
        .expect("backdate");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_first_key_brings_recovery_codes_and_each_key_is_listed(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let mut phone = Authenticator::new();
    let mut yubikey = Authenticator::new();

    let first = add_key(&app, &token, &mut phone, "passkey").await;
    assert_eq!(first["key"]["name"], "Passkey");
    assert_eq!(first["key"]["kind"], "passkey");
    assert_eq!(first["key"]["rp_id"], HOST_A);
    assert_eq!(first["recovery_codes"].as_array().map(Vec::len), Some(10));
    let second = add_key(&app, &token, &mut yubikey, "security_key").await;
    assert_eq!(second["key"]["name"], "Security key");
    assert!(second.get("recovery_codes").is_none());

    let listed = app.get("/api/v1/me/webauthn", &token).await.json();
    assert_eq!(listed.as_array().map(Vec::len), Some(2));
    let me = app.get("/api/v1/me", &token).await.json();
    assert_eq!(me["second_factors"]["webauthn"], 2);
    assert_eq!(me["second_factors"]["recovery_codes_left"], 10);

    let id = first["key"]["id"].as_str().expect("id");
    let renamed = from_page(
        &app,
        (HOST_A, ORIGIN),
        Method::PATCH,
        &format!("/api/v1/me/webauthn/{id}"),
        Some(&token),
        &json!({ "name": "  Phone " }),
    )
    .await;
    assert_eq!(renamed.json()["name"], "Phone");

    sign_in_long_ago(&pool).await;
    let later = post(
        &app,
        "/api/v1/me/webauthn/start",
        Some(&token),
        json!({ "kind": "passkey" }),
    )
    .await;
    assert_eq!(problem(&later), "reauthentication-required");
    assert_eq!(later.json()["methods"], json!(["password", "webauthn"]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_list_says_which_keys_can_protect_folders(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let mut yubikey = Authenticator::new();
    yubikey.hmac_secret = true;
    let mut old_yubikey = Authenticator::new();
    let mut phone = Authenticator::new();
    phone.synced = true;
    phone.hmac_secret = true;

    let added = add_key(&app, &token, &mut yubikey, "security_key").await;
    assert_eq!(added["key"]["prf"], true);
    assert!(added["key"].get("synced").is_none());
    add_key(&app, &token, &mut old_yubikey, "passkey").await;
    add_key(&app, &token, &mut phone, "passkey").await;

    let listed = app.get("/api/v1/me/webauthn", &token).await.json();
    let keys: Vec<protocol::WebAuthnKey> = serde_json::from_value(listed).expect("keys");
    let can: Vec<bool> = keys
        .iter()
        .map(protocol::WebAuthnKey::can_protect_folders)
        .collect();
    assert_eq!(can, [true, false, false], "{keys:?}");
    assert!(!keys[1].prf && !keys[1].synced);
    assert!(keys[2].prf && keys[2].synced);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_key_is_the_second_step_after_the_password_and_answers_once(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let mut yubikey = Authenticator::new();
    yubikey.verifies_user = false;
    add_key(&app, &token, &mut yubikey, "security_key").await;

    let (login_token, methods) = first_step(&app, "alice").await;
    assert_eq!(methods, json!(["webauthn", "recovery"]));
    let signed_in = second_step(&app, &login_token, &mut yubikey).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.json());

    // An answer to one sign-in's challenge does not finish another.
    let (old_token, _) = first_step(&app, "alice").await;
    let options = post(
        &app,
        "/api/v1/auth/login/webauthn",
        None,
        json!({ "login_token": old_token }),
    )
    .await;
    let stale = yubikey
        .sign(ORIGIN, &options.json()["options"])
        .expect("answer");
    let (login_token, _) = first_step(&app, "alice").await;
    post(
        &app,
        "/api/v1/auth/login/webauthn",
        None,
        json!({ "login_token": login_token }),
    )
    .await;
    let replayed = post(
        &app,
        "/api/v1/auth/login/verify",
        None,
        json!({ "login_token": login_token, "webauthn": stale }),
    )
    .await;
    assert_eq!(problem(&replayed), "webauthn-failed");

    assert_eq!(
        session_methods(&pool).await,
        [vec!["pwd"], vec!["pwd", "hwk"]]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_passkey_signs_in_on_its_own_and_a_security_key_does_not(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let mut phone = Authenticator::new();
    phone.synced = true;
    add_key(&app, &token, &mut phone, "passkey").await;

    let signed_in = passkey_sign_in(&app, &mut phone).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.json());
    let fresh = signed_in.json()["access_token"]
        .as_str()
        .expect("token")
        .to_owned();
    assert_eq!(
        app.get("/api/v1/me", &fresh).await.json()["username"],
        "alice"
    );

    let bob = app.user_token("bob").await;
    let mut yubikey = Authenticator::new();
    add_key(&app, &bob, &mut yubikey, "security_key").await;
    let refused = passkey_sign_in(&app, &mut yubikey).await;
    assert_eq!(problem(&refused), "webauthn-failed");

    // Each challenge is answered once.
    let started = post(&app, "/api/v1/auth/passkey/start", None, json!({}))
        .await
        .json();
    let answer = phone.sign(ORIGIN, &started["options"]).expect("answer");
    let finish = json!({ "challenge_id": started["challenge_id"], "credential": answer });
    let first = post(&app, "/api/v1/auth/passkey/finish", None, finish.clone()).await;
    assert_eq!(first.status, StatusCode::OK);
    let again = post(&app, "/api/v1/auth/passkey/finish", None, finish).await;
    assert_eq!(problem(&again), "login-expired");

    // Alice with the password, with the passkey, Bob with the password, Alice again.
    let methods = session_methods(&pool).await;
    assert_eq!(methods[1], ["swk", "user"]);
    assert_eq!(methods[3], ["swk", "user"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn passkeys_need_the_servers_own_https_or_localhost_page_and_a_name(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let body = json!({ "kind": "passkey" });
    for page in [
        (HOST_A, "http://files.example.com"),
        (HOST_A, "https://evil.example.net"),
        ("192.168.1.4", "https://192.168.1.4"),
    ] {
        let refused = from_page(
            &app,
            page,
            Method::POST,
            "/api/v1/me/webauthn/start",
            Some(&token),
            &body,
        )
        .await;
        assert_eq!(problem(&refused), "webauthn-unavailable", "{page:?}");
    }
    let local = from_page(
        &app,
        ("localhost:8080", "http://localhost:8080"),
        Method::POST,
        "/api/v1/me/webauthn/start",
        Some(&token),
        &body,
    )
    .await;
    assert_eq!(local.status, StatusCode::OK, "{}", local.json());
    assert_eq!(
        local.json()["options"]["publicKey"]["rp"]["id"],
        "localhost"
    );
    assert_eq!(
        local.json()["options"]["publicKey"]["authenticatorSelection"]["residentKey"],
        "required"
    );

    let providers = |host: &'static str| {
        let app = app.clone();
        async move {
            app.send_with(
                Method::GET,
                "/api/v1/auth/providers",
                None,
                Body::empty(),
                None,
                &[("host", host)],
            )
            .await
            .json()["passkeys"]
                .clone()
        }
    };
    assert_eq!(providers(HOST_A).await, json!(true));
    assert_eq!(providers("127.0.0.1:8080").await, json!(false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_key_confirms_it_is_the_user(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let mut phone = Authenticator::new();
    add_key(&app, &token, &mut phone, "passkey").await;
    sign_in_long_ago(&pool).await;
    let blocked = post(&app, "/api/v1/me/recovery-codes", Some(&token), json!({})).await;
    assert_eq!(problem(&blocked), "reauthentication-required");

    let challenge = post(
        &app,
        "/api/v1/me/reauthenticate/webauthn",
        Some(&token),
        json!({}),
    )
    .await;
    assert_eq!(challenge.status, StatusCode::OK, "{}", challenge.json());
    let challenge = challenge.json();
    let answer = phone.sign(ORIGIN, &challenge["options"]).expect("answer");
    let confirmed = post(
        &app,
        "/api/v1/me/reauthenticate",
        Some(&token),
        json!({ "webauthn": { "challenge_id": challenge["challenge_id"], "credential": answer } }),
    )
    .await;
    assert_eq!(
        confirmed.status,
        StatusCode::NO_CONTENT,
        "{}",
        confirmed.json()
    );
    let codes = post(&app, "/api/v1/me/recovery-codes", Some(&token), json!({})).await;
    assert_eq!(codes.status, StatusCode::OK, "{}", codes.json());
}

#[sqlx::test(migrations = "../../migrations")]
async fn removing_the_last_key_or_a_reset_signs_in_with_the_password_alone(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let mut phone = Authenticator::new();
    let added = add_key(&app, &token, &mut phone, "passkey").await;
    let id = added["key"]["id"].as_str().expect("id");
    let removed = app
        .send(
            Method::DELETE,
            &format!("/api/v1/me/webauthn/{id}"),
            Some(&token),
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    let me = app.get("/api/v1/me", &token).await.json();
    assert_eq!(
        me["second_factors"],
        json!({ "totp": false, "webauthn": 0, "recovery_codes_left": 0 })
    );
    let plain = post(
        &app,
        "/api/v1/auth/login",
        None,
        json!({ "username": "alice", "password": PASSWORD, "two_step": true }),
    )
    .await;
    assert_eq!(plain.status, StatusCode::OK);

    let bob = app.user_token("bob").await;
    add_key(&app, &bob, &mut phone, "passkey").await;
    let admin = app.user_token("admin").await;
    app.db
        .set_admin(app.user_id("admin").await, true)
        .await
        .expect("admin");
    let accounts = app.get("/api/v1/admin/users", &admin).await.json();
    let bob_account = accounts
        .as_array()
        .expect("accounts")
        .iter()
        .find(|account| account["username"] == "bob")
        .expect("bob")
        .clone();
    assert_eq!(bob_account["factors"], json!(["webauthn"]));
    let bob_id = app.user_id("bob").await;
    let reset = app
        .send(
            Method::DELETE,
            &format!("/api/v1/admin/users/{bob_id}/second-factors"),
            Some(&admin),
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT);
    assert_eq!(
        problem(&passkey_sign_in(&app, &mut phone).await),
        "webauthn-failed"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_passkey_works_only_on_its_own_site(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let token = a.user_token("alice").await;
    let mut phone = Authenticator::new();
    add_key(&a, &token, &mut phone, "passkey").await;

    let other_origin = format!("https://{HOST_B}");
    let started = from_page(
        &b,
        (HOST_B, &other_origin),
        Method::POST,
        "/api/v1/auth/passkey/start",
        None,
        &json!({}),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.json());
    let challenge = started.json();
    assert!(
        phone.sign(&other_origin, &challenge["options"]).is_none(),
        "made for the other site"
    );

    // Even an answer made as if for this site's challenge names a credential it does not have.
    let mut options = challenge["options"].clone();
    options["publicKey"]["rpId"] = json!(HOST_A);
    let forged = phone.sign(ORIGIN, &options).expect("answer");
    let refused = from_page(
        &b,
        (HOST_B, &other_origin),
        Method::POST,
        "/api/v1/auth/passkey/finish",
        None,
        &json!({ "challenge_id": challenge["challenge_id"], "credential": forged }),
    )
    .await;
    assert_eq!(problem(&refused), "webauthn-failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn of_two_answers_raced_with_one_key_only_the_higher_counter_signs_in(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let mut yubikey = Authenticator::new();
    add_key(&app, &token, &mut yubikey, "passkey").await;

    // Both challenges were made while the stored counter was 0, so each answer is newer than
    // what its challenge knew; only the database still knows which came first.
    let start = || post(&app, "/api/v1/auth/passkey/start", None, json!({}));
    let earlier = start().await.json();
    let later = start().await.json();
    let first = yubikey.sign(ORIGIN, &earlier["options"]).expect("answer");
    let second = yubikey.sign(ORIGIN, &later["options"]).expect("answer");
    let finish = |challenge: &Value, answer: Value| {
        post(
            &app,
            "/api/v1/auth/passkey/finish",
            None,
            json!({ "challenge_id": challenge["challenge_id"], "credential": answer }),
        )
    };
    let newer = finish(&later, second).await;
    assert_eq!(newer.status, StatusCode::OK, "{}", newer.json());
    let older = finish(&earlier, first).await;
    assert_eq!(problem(&older), "webauthn-failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn one_address_may_start_only_so_many_passkey_sign_ins(pool: PgPool) {
    let app = setup(pool).await;
    let start_from = |address: &'static str| {
        let app = &app;
        async move {
            app.send_with(
                Method::POST,
                "/api/v1/auth/passkey/start",
                None,
                Body::from("{}"),
                Some("application/json"),
                &[
                    ("host", HOST_A),
                    ("origin", ORIGIN),
                    ("x-forwarded-for", address),
                ],
            )
            .await
        }
    };
    for _ in 0..30 {
        assert_eq!(start_from("198.51.100.7").await.status, StatusCode::OK);
    }
    let refused = start_from("198.51.100.7").await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(problem(&refused), "too-many-attempts");
    assert_eq!(start_from("203.0.113.9").await.status, StatusCode::OK);
}
