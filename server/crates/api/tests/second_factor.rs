//! Two-factor sign-in with an authenticator app and recovery codes.

mod common;

use axum::http::{Method, StatusCode};
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha1::Sha1;
use sqlx::PgPool;

use common::{PASSWORD, TestApp, setup, setup_two_tenants, setup_with};

const STEP_SECONDS: i64 = 30;

/// The current time step, waiting for the next one when this one is about to end, so that a
/// test using the steps around it does not straddle two.
async fn fresh_step() -> i64 {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if now.rem_euclid(STEP_SECONDS) > STEP_SECONDS - 8 {
        let wait = STEP_SECONDS - now.rem_euclid(STEP_SECONDS) + 1;
        tokio::time::sleep(std::time::Duration::from_secs(wait.unsigned_abs())).await;
    }
    time::OffsetDateTime::now_utc().unix_timestamp() / STEP_SECONDS
}

/// The code an authenticator app shows for `secret` (base32) at `step`.
fn code(secret: &str, step: i64) -> String {
    let key = BASE32_NOPAD.decode(secret.as_bytes()).expect("base32");
    let mut mac = Hmac::<Sha1>::new_from_slice(&key).expect("key");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let value = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:06}", value % 1_000_000)
}

/// Adds an authenticator app for the user of `token`, confirmed with the code of the step
/// before `step`: returns its secret and the recovery codes.
async fn add_app(app: &TestApp, token: &str, step: i64) -> (String, Vec<String>) {
    let started = app
        .json(Method::POST, "/api/v1/me/totp", Some(token), json!({}))
        .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.json());
    let secret = started.json()["secret"]
        .as_str()
        .expect("secret")
        .to_owned();
    let uri = started.json()["uri"].as_str().expect("uri").to_owned();
    assert!(uri.starts_with("otpauth://totp/Home%20Space:"), "{uri}");
    let confirmed = app
        .json(
            Method::POST,
            "/api/v1/me/totp/confirm",
            Some(token),
            json!({ "code": code(&secret, step - 1) }),
        )
        .await;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.json());
    let codes = confirmed.json()["codes"]
        .as_array()
        .expect("codes")
        .iter()
        .map(|code| code.as_str().expect("code").to_owned())
        .collect();
    (secret, codes)
}

async fn sign_in(app: &TestApp, username: &str, two_step: bool) -> common::Response {
    app.json(
        Method::POST,
        "/api/v1/auth/login",
        None,
        json!({ "username": username, "password": PASSWORD, "two_step": two_step }),
    )
    .await
}

/// Signs in with the password and asks for the second step: returns the login token.
async fn first_step(app: &TestApp, username: &str) -> String {
    let response = sign_in(app, username, true).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    response.json()["login_token"]
        .as_str()
        .expect("login token")
        .to_owned()
}

async fn verify(app: &TestApp, token: &str, proof: Value) -> common::Response {
    let mut body = json!({ "login_token": token });
    body.as_object_mut()
        .expect("object")
        .extend(proof.as_object().expect("proof").clone());
    app.json(Method::POST, "/api/v1/auth/login/verify", None, body)
        .await
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

#[sqlx::test(migrations = "../../migrations")]
async fn with_an_app_signing_in_takes_a_code_or_a_recovery_code(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let step = fresh_step().await;

    app.json(Method::POST, "/api/v1/me/totp", Some(&token), json!({}))
        .await;
    let refused = app
        .json(
            Method::POST,
            "/api/v1/me/totp/confirm",
            Some(&token),
            json!({ "code": "000000" }),
        )
        .await;
    assert_eq!(problem(&refused), "invalid-code");
    let (secret, recovery) = add_app(&app, &token, step).await;
    assert_eq!(recovery.len(), 10);
    let me = app.get("/api/v1/me", &token).await.json();
    assert_eq!(
        me["second_factors"],
        json!({ "totp": true, "webauthn": 0, "recovery_codes_left": 10 })
    );

    // An app that cannot ask for the code is told why, and given nothing to go on with.
    let old_app = sign_in(&app, "alice", false).await;
    assert_eq!(old_app.status, StatusCode::FORBIDDEN);
    assert_eq!(problem(&old_app), "second-factor-required");
    assert!(old_app.json().get("login_token").is_none());
    assert!(
        old_app.json()["detail"]
            .as_str()
            .expect("detail")
            .contains("browser")
    );

    let login_token = first_step(&app, "alice").await;
    let signed_in = verify(&app, &login_token, json!({ "code": code(&secret, step) })).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.json());
    assert!(signed_in.json()["access_token"].is_string());
    let used = verify(&app, &login_token, json!({ "code": code(&secret, step) })).await;
    assert_eq!(problem(&used), "login-expired");

    let login_token = first_step(&app, "alice").await;
    let replayed = verify(&app, &login_token, json!({ "code": code(&secret, step) })).await;
    assert_eq!(problem(&replayed), "invalid-code");
    let typed = recovery[0].to_uppercase().replace('-', " ");
    let recovered = verify(&app, &login_token, json!({ "recovery_code": typed })).await;
    assert_eq!(recovered.status, StatusCode::OK, "{}", recovered.json());
    let fresh = recovered.json()["access_token"]
        .as_str()
        .expect("token")
        .to_owned();
    let me = app.get("/api/v1/me", &fresh).await.json();
    assert_eq!(me["second_factors"]["recovery_codes_left"], 9);

    let login_token = first_step(&app, "alice").await;
    let again = verify(&app, &login_token, json!({ "recovery_code": recovery[0] })).await;
    assert_eq!(problem(&again), "invalid-code");

    // The sessions say what proved the user.
    let methods: Vec<Vec<String>> =
        sqlx::query_scalar("SELECT auth_methods FROM sessions ORDER BY created_at")
            .fetch_all(&pool)
            .await
            .expect("sessions");
    assert_eq!(
        methods,
        [vec!["pwd"], vec!["pwd", "otp"], vec!["pwd", "recovery"]]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn wrong_codes_are_throttled_like_wrong_passwords(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    add_app(&app, &token, fresh_step().await).await;
    let login_token = first_step(&app, "alice").await;
    for _ in 0..3 {
        let wrong = verify(&app, &login_token, json!({ "code": "000000" })).await;
        assert_eq!(problem(&wrong), "invalid-code");
    }
    let blocked = verify(&app, &login_token, json!({ "code": "000000" })).await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test(migrations = "../../migrations")]
async fn adding_an_app_needs_a_recent_sign_in_and_the_servers_key(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    sqlx::query("UPDATE sessions SET authenticated_at = now() - interval '11 minutes'")
        .execute(&pool)
        .await
        .expect("age");
    let refused = app
        .json(Method::POST, "/api/v1/me/totp", Some(&token), json!({}))
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(problem(&refused), "reauthentication-required");
    assert_eq!(refused.json()["methods"], json!(["password"]));

    let confirmed = app
        .json(
            Method::POST,
            "/api/v1/me/reauthenticate",
            Some(&token),
            json!({ "password": PASSWORD }),
        )
        .await;
    assert_eq!(confirmed.status, StatusCode::NO_CONTENT);
    let step = fresh_step().await;
    let (secret, _) = add_app(&app, &token, step).await;

    // With an app, a code from it confirms too; for new recovery codes only it does.
    sqlx::query("UPDATE sessions SET authenticated_at = now() - interval '11 minutes'")
        .execute(&pool)
        .await
        .expect("age");
    let new_codes = || {
        app.json(
            Method::POST,
            "/api/v1/me/recovery-codes",
            Some(&token),
            json!({}),
        )
    };
    let refused = new_codes().await;
    assert_eq!(refused.json()["methods"], json!(["totp"]));
    let by_password = app
        .json(
            Method::POST,
            "/api/v1/me/reauthenticate",
            Some(&token),
            json!({ "password": PASSWORD }),
        )
        .await;
    assert_eq!(by_password.status, StatusCode::NO_CONTENT);
    let refused = new_codes().await;
    assert_eq!(problem(&refused), "reauthentication-required");
    assert_eq!(refused.json()["methods"], json!(["totp"]));
    let by_code = app
        .json(
            Method::POST,
            "/api/v1/me/reauthenticate",
            Some(&token),
            json!({ "code": code(&secret, step) }),
        )
        .await;
    assert_eq!(by_code.status, StatusCode::NO_CONTENT, "{}", by_code.json());
    let codes = app
        .json(
            Method::POST,
            "/api/v1/me/recovery-codes",
            Some(&token),
            json!({}),
        )
        .await;
    assert_eq!(codes.json()["codes"].as_array().expect("codes").len(), 10);
}

/// Confirms it is the user of `token` with a code from their authenticator app.
async fn confirm_with_code(app: &TestApp, token: &str, code: &str) {
    let confirmed = app
        .json(
            Method::POST,
            "/api/v1/me/reauthenticate",
            Some(token),
            json!({ "code": code }),
        )
        .await;
    assert_eq!(
        confirmed.status,
        StatusCode::NO_CONTENT,
        "{}",
        confirmed.json()
    );
}

async fn change_password(app: &TestApp, token: &str) -> common::Response {
    app.json(
        Method::POST,
        "/api/v1/me/password",
        Some(token),
        json!({ "current_password": PASSWORD, "new_password": "a-new-password-1" }),
    )
    .await
}

async fn remove_app(app: &TestApp, token: &str) -> common::Response {
    app.send(
        Method::DELETE,
        "/api/v1/me/totp",
        Some(token),
        axum::body::Body::empty(),
        None,
    )
    .await
}

/// Security: a stolen session and password cannot take the second factor away. Removing it
/// needs a recent proof with a second factor, which a password confirmation is not, and then
/// ends the other sessions, so none signed in with it outlives it.
#[sqlx::test(migrations = "../../migrations")]
async fn removing_the_app_needs_a_second_factor_and_ends_the_other_sessions(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let step = fresh_step().await;
    let (secret, recovery) = add_app(&app, &token, step).await;
    let login_token = first_step(&app, "alice").await;
    let other = verify(&app, &login_token, json!({ "recovery_code": recovery[0] })).await;
    let other = other.json()["access_token"]
        .as_str()
        .expect("token")
        .to_owned();

    // This session signed in with the password alone, before the app was added.
    let refused = remove_app(&app, &token).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(problem(&refused), "reauthentication-required");
    assert_eq!(refused.json()["methods"], json!(["totp"]));
    let by_password = app
        .json(
            Method::POST,
            "/api/v1/me/reauthenticate",
            Some(&token),
            json!({ "password": PASSWORD }),
        )
        .await;
    assert_eq!(by_password.status, StatusCode::NO_CONTENT);
    assert_eq!(
        problem(&remove_app(&app, &token).await),
        "reauthentication-required"
    );

    confirm_with_code(&app, &token, &code(&secret, step)).await;
    assert_eq!(
        remove_app(&app, &token).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.get("/api/v1/me", &token).await.status, StatusCode::OK);
    assert_eq!(
        app.get("/api/v1/me", &other).await.status,
        StatusCode::UNAUTHORIZED
    );
}

/// A session that signed in with its second factor may change sign-in factors for a while
/// without confirming again, and so may one that confirmed with it; the account password
/// changes only with one of them too.
#[sqlx::test(migrations = "../../migrations")]
async fn a_sign_in_with_the_second_factor_counts_for_changing_the_password(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let step = fresh_step().await;
    let (secret, recovery) = add_app(&app, &token, step).await;
    let refused = change_password(&app, &token).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.json());
    assert_eq!(refused.json()["methods"], json!(["totp"]));

    let login_token = first_step(&app, "alice").await;
    let signed_in = verify(&app, &login_token, json!({ "recovery_code": recovery[0] })).await;
    let fresh = signed_in.json()["access_token"]
        .as_str()
        .expect("token")
        .to_owned();
    let new_codes = app
        .json(
            Method::POST,
            "/api/v1/me/recovery-codes",
            Some(&fresh),
            json!({}),
        )
        .await;
    assert_eq!(new_codes.status, StatusCode::OK, "{}", new_codes.json());

    sqlx::query("UPDATE sessions SET second_factor_at = now() - interval '11 minutes'")
        .execute(&pool)
        .await
        .expect("age");
    assert_eq!(
        problem(&change_password(&app, &fresh).await),
        "reauthentication-required"
    );
    confirm_with_code(&app, &fresh, &code(&secret, step)).await;
    let changed = change_password(&app, &fresh).await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.json());
}

#[sqlx::test(migrations = "../../migrations")]
async fn without_the_key_no_app_can_be_added(pool: PgPool) {
    let app = setup_with(pool, |auth| auth.with_secrets_key(None)).await;
    let token = app.user_token("alice").await;
    let refused = app
        .json(Method::POST, "/api/v1/me/totp", Some(&token), json!({}))
        .await;
    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem(&refused), "secrets-key-missing");
}

#[sqlx::test(migrations = "../../migrations")]
async fn removing_the_app_or_an_administrators_reset_signs_in_with_the_password_again(
    pool: PgPool,
) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let step = fresh_step().await;
    let (secret, _) = add_app(&app, &token, step).await;
    confirm_with_code(&app, &token, &code(&secret, step)).await;
    let again = app
        .json(Method::POST, "/api/v1/me/totp", Some(&token), json!({}))
        .await;
    assert_eq!(problem(&again), "totp-exists");
    let removed = app
        .send(
            Method::DELETE,
            "/api/v1/me/totp",
            Some(&token),
            axum::body::Body::empty(),
            None,
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    assert_eq!(sign_in(&app, "alice", true).await.status, StatusCode::OK);

    let bob = app.user_token("bob").await;
    add_app(&app, &bob, fresh_step().await).await;
    let admin = app.user_token("admin").await;
    let admin_id = app.user_id("admin").await;
    app.db.set_admin(admin_id, true).await.expect("admin");
    let bob_id = app.user_id("bob").await;
    let accounts = app.get("/api/v1/admin/users", &admin).await.json();
    let listed = |accounts: &Value| {
        accounts
            .as_array()
            .expect("accounts")
            .iter()
            .find(|account| account["username"] == "bob")
            .expect("bob")["two_factor"]
            .clone()
    };
    assert_eq!(listed(&accounts), json!(true));
    let reset = app
        .send(
            Method::DELETE,
            &format!("/api/v1/admin/users/{bob_id}/second-factors"),
            Some(&admin),
            axum::body::Body::empty(),
            None,
        )
        .await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.get("/api/v1/me", &bob).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(sign_in(&app, "bob", true).await.status, StatusCode::OK);
    let accounts = app.get("/api/v1/admin/users", &admin).await.json();
    assert_eq!(listed(&accounts), json!(false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_waiting_sign_in_finishes_only_on_its_own_site(pool: PgPool) {
    let (a, b) = setup_two_tenants(pool).await;
    let token = a.user_token("alice").await;
    let step = fresh_step().await;
    let (secret, _) = add_app(&a, &token, step).await;
    let login_token = first_step(&a, "alice").await;
    let elsewhere = verify(&b, &login_token, json!({ "code": code(&secret, step) })).await;
    assert_eq!(problem(&elsewhere), "login-expired");
    let here = verify(&a, &login_token, json!({ "code": code(&secret, step) })).await;
    assert_eq!(here.status, StatusCode::OK);
}
