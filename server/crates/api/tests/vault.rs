//! The vault and protected folders: enabling security keys, the recovery key, rotation, and
//! protecting folders with a security key or a password.

#[path = "webauthn/authenticator.rs"]
#[allow(dead_code)]
mod authenticator;
mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sqlx::PgPool;

use authenticator::Authenticator;
use common::{HOST_A, TestApp, setup};

const ORIGIN: &str = "https://files.example.com";
const RECIPIENT: &str = "age1fauk43xvke3myu0vg0syaflnwhlslwxd2504jfdjwejkjc50pcysgcf5gf";

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn wrapped(fill: u8) -> String {
    b64(&[fill; 73])
}

async fn send(
    app: &TestApp,
    method: Method,
    uri: &str,
    token: &str,
    payload: Value,
) -> common::Response {
    app.send_with(
        method,
        uri,
        Some(token),
        Body::from(payload.to_string()),
        Some("application/json"),
        &[("host", HOST_A), ("origin", ORIGIN)],
    )
    .await
}

async fn delete(app: &TestApp, uri: &str, token: &str) -> common::Response {
    app.send(Method::DELETE, uri, Some(token), Body::empty(), None)
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

/// Adds a security key made by `device`; returns its id.
async fn add_key(app: &TestApp, token: &str, device: &mut Authenticator) -> String {
    let started = send(
        app,
        Method::POST,
        "/api/v1/me/webauthn/start",
        token,
        json!({ "kind": "passkey" }),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.json());
    let challenge = started.json();
    let credential = device.register(ORIGIN, &challenge["options"]);
    let finished = send(
        app,
        Method::POST,
        "/api/v1/me/webauthn/finish",
        token,
        json!({ "challenge_id": challenge["challenge_id"], "credential": credential }),
    )
    .await;
    assert_eq!(finished.status, StatusCode::OK, "{}", finished.json());
    finished.json()["key"]["id"]
        .as_str()
        .expect("id")
        .to_owned()
}

fn yubikey() -> Authenticator {
    let mut key = Authenticator::new();
    key.hmac_secret = true;
    key
}

async fn enable(app: &TestApp, token: &str, credential: &str, epoch: i32) -> common::Response {
    send(
        app,
        Method::POST,
        "/api/v1/vault/unlockers",
        token,
        json!({
            "kind": "security_key",
            "credential": credential,
            "prf_salt": b64(&[3; 32]),
            "wrapped_vault_key": wrapped(1),
            "epoch": epoch,
        }),
    )
    .await
}

async fn folder(app: &TestApp, token: &str, name: &str) -> String {
    let made = app
        .json(
            Method::POST,
            "/api/v1/folders",
            Some(token),
            json!({ "name": name }),
        )
        .await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.json());
    made.json()["id"].as_str().expect("id").to_owned()
}

/// Protects `node` with the vault at `epoch`.
async fn protect(app: &TestApp, token: &str, node: &str, epoch: i32) -> common::Response {
    send(
        app,
        Method::POST,
        "/api/v1/protected",
        token,
        json!({
            "node_id": node, "lock": "security_key", "wrapped_secret": wrapped(2),
            "recipient": RECIPIENT, "epoch": epoch,
        }),
    )
    .await
}

/// Gives the user of `token` a vault at epoch 1, unlocked by a recovery key.
async fn recovery_vault(app: &TestApp, token: &str) {
    let made = send(
        app,
        Method::POST,
        "/api/v1/vault/unlockers",
        token,
        json!({ "kind": "recovery", "wrapped_vault_key": wrapped(4), "epoch": 1 }),
    )
    .await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.json());
}

async fn sign_in_long_ago(pool: &PgPool) {
    sqlx::query("UPDATE sessions SET authenticated_at = now() - interval '1 hour'")
        .execute(pool)
        .await
        .expect("backdate");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_security_key_with_prf_unlocks_the_vault_and_locks_folders(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    assert_eq!(
        app.get("/api/v1/vault", &token).await.status,
        StatusCode::NOT_FOUND
    );
    let capabilities = app.get("/api/v1/capabilities", &token).await.json();
    assert!(
        capabilities.to_string().contains("protected_folders"),
        "{capabilities}"
    );

    let mut old = Authenticator::new();
    let old_key = add_key(&app, &token, &mut old).await;
    let refused = enable(&app, &token, &old_key, 1).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.json()["errors"][0]["pointer"], "/credential");

    let mut yubikey = yubikey();
    let key = add_key(&app, &token, &mut yubikey).await;
    let enabled = enable(&app, &token, &key, 1).await;
    assert_eq!(enabled.status, StatusCode::CREATED, "{}", enabled.json());
    assert_eq!(enabled.json()["rp_id"], "files.example.com");
    assert_eq!(enabled.json()["prf_salt"], b64(&[3; 32]));

    let vault = app.get("/api/v1/vault", &token).await.json();
    assert_eq!(vault["epoch"], 1);
    assert_eq!(vault["unlockers"][0]["credential"], key);

    let papers = folder(&app, &token, "Papers").await;
    let protected = send(
        &app,
        Method::POST,
        "/api/v1/protected",
        &token,
        json!({
            "node_id": papers,
            "lock": "security_key",
            "wrapped_secret": wrapped(2),
            "recipient": RECIPIENT,
            "epoch": 1,
        }),
    )
    .await;
    assert_eq!(
        protected.status,
        StatusCode::CREATED,
        "{}",
        protected.json()
    );
    assert_eq!(protected.json()["state"], "protected");
    let listed = app.get("/api/v1/protected", &token).await.json();
    assert_eq!(listed[0]["node_id"], papers);
    assert_eq!(listed[0]["recipient"], RECIPIENT);

    // The only key of a locked folder stays until another unlocks it too.
    let removing = delete(&app, &format!("/api/v1/me/webauthn/{key}"), &token).await;
    assert_eq!(removing.status, StatusCode::CONFLICT);
    assert_eq!(problem(&removing), "last-unlocker");
    let unlocker = vault["unlockers"][0]["id"].as_str().expect("id");
    let removing = delete(&app, &format!("/api/v1/vault/unlockers/{unlocker}"), &token).await;
    assert_eq!(problem(&removing), "last-unlocker");

    let recovery = send(
        &app,
        Method::POST,
        "/api/v1/vault/unlockers",
        &token,
        json!({ "kind": "recovery", "wrapped_vault_key": wrapped(4), "epoch": 1 }),
    )
    .await;
    assert_eq!(recovery.status, StatusCode::CREATED, "{}", recovery.json());
    let removed = delete(&app, &format!("/api/v1/me/webauthn/{key}"), &token).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.json());
    let vault = app.get("/api/v1/vault", &token).await.json();
    assert_eq!(vault["unlockers"].as_array().map(Vec::len), Some(1));
    assert_eq!(vault["unlockers"][0]["kind"], "recovery");
}

#[sqlx::test(migrations = "../../migrations")]
async fn rotation_needs_the_current_epoch_and_every_folder(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let mut one = yubikey();
    let mut two = yubikey();
    let first = add_key(&app, &token, &mut one).await;
    let second = add_key(&app, &token, &mut two).await;
    let kept = enable(&app, &token, &first, 1).await.json()["id"].clone();
    let lost = enable(&app, &token, &second, 1).await.json()["id"].clone();
    let papers = folder(&app, &token, "Papers").await;
    send(
        &app,
        Method::POST,
        "/api/v1/protected",
        &token,
        json!({
            "node_id": papers, "lock": "security_key", "wrapped_secret": wrapped(2),
            "recipient": RECIPIENT, "epoch": 1,
        }),
    )
    .await;

    let missing = send(
        &app,
        Method::POST,
        "/api/v1/vault/rotate",
        &token,
        json!({
            "epoch": 1,
            "unlockers": [{ "id": kept, "wrapped_vault_key": wrapped(8) }],
            "remove": [lost],
        }),
    )
    .await;
    assert_eq!(
        missing.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        missing.json()
    );
    let rotation = json!({
        "epoch": 1,
        "unlockers": [{ "id": kept, "wrapped_vault_key": wrapped(8) }],
        "folders": [{ "node_id": papers, "wrapped_secret": wrapped(9) }],
        "remove": [lost],
    });
    let rotated = send(
        &app,
        Method::POST,
        "/api/v1/vault/rotate",
        &token,
        rotation.clone(),
    )
    .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.json());
    assert_eq!(rotated.json()["epoch"], 2);
    let again = send(&app, Method::POST, "/api/v1/vault/rotate", &token, rotation).await;
    assert_eq!(problem(&again), "vault-changed");

    // A key enabled now must be wrapped under the new epoch.
    let stale = enable(&app, &token, &second, 1).await;
    assert_eq!(problem(&stale), "vault-changed");
    assert_eq!(
        enable(&app, &token, &second, 2).await.status,
        StatusCode::CREATED
    );
    let listed = app.get("/api/v1/protected", &token).await.json();
    assert_eq!(listed[0]["epoch"], 2);
    assert_eq!(listed[0]["wrapped_secret"], wrapped(9));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_folder_needs_a_whole_lock_and_is_protected_once(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    let protect_with = |body: Value| send(&app, Method::POST, "/api/v1/protected", &token, body);

    let password = protect_with(json!({ "node_id": notes, "lock": "password" })).await;
    assert_eq!(password.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(password.json()["errors"][0]["pointer"], "/lock");
    let bad_recipient = protect_with(json!({
        "node_id": notes, "lock": "security_key", "wrapped_secret": wrapped(2),
        "recipient": "age1nope", "epoch": 1,
    }))
    .await;
    assert_eq!(bad_recipient.json()["errors"][0]["pointer"], "/recipient");
    let short = protect_with(json!({
        "node_id": notes, "lock": "security_key", "wrapped_secret": b64(&[1; 40]),
        "recipient": RECIPIENT, "epoch": 1,
    }))
    .await;
    assert_eq!(short.json()["errors"][0]["pointer"], "/wrapped_secret");
    assert_eq!(
        problem(&protect(&app, &token, &notes, 1).await),
        "vault-changed"
    );

    recovery_vault(&app, &token).await;
    let protected = protect(&app, &token, &notes, 1).await;
    assert_eq!(
        protected.status,
        StatusCode::CREATED,
        "{}",
        protected.json()
    );
    assert_eq!(protected.json()["state"], "protected");
    assert_eq!(
        problem(&protect(&app, &token, &notes, 1).await),
        "protected"
    );

    let bob = app.user_token("bob").await;
    assert_eq!(
        protect(&app, &bob, &notes, 1).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.get("/api/v1/protected", &bob).await.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_folder_with_content_is_protecting_until_the_client_has_replaced_it(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    recovery_vault(&app, &token).await;
    let papers = folder(&app, &token, "Papers").await;
    let uploaded = app
        .upload(&token, &format!("parent_id={papers}&name=tax.pdf"), "plain")
        .await;
    assert_eq!(uploaded.status, StatusCode::CREATED, "{}", uploaded.json());

    let protecting = protect(&app, &token, &papers, 1).await;
    assert_eq!(protecting.json()["state"], "protecting");
    let id = |action: &str| format!("/api/v1/protected/{papers}{action}");
    let early = send(&app, Method::POST, &id("/unprotect"), &token, json!({})).await;
    assert_eq!(problem(&early), "protected");
    let finished = send(&app, Method::POST, &id("/finish"), &token, json!({})).await;
    assert_eq!(finished.status, StatusCode::OK, "{}", finished.json());
    assert_eq!(finished.json()["state"], "protected");

    // A protected folder's record is never dropped while its files need its secret.
    let removing = delete(&app, &id(""), &token).await;
    assert_eq!(problem(&removing), "protected");
    let unprotecting = send(&app, Method::POST, &id("/unprotect"), &token, json!({})).await;
    assert_eq!(unprotecting.json()["state"], "unprotecting");
    let removed = delete(&app, &id(""), &token).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.json());
    assert_eq!(app.get("/api/v1/protected", &token).await.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_what_unlocks_needs_a_recent_sign_in(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let mut key = yubikey();
    let credential = add_key(&app, &token, &mut key).await;
    let notes = folder(&app, &token, "Notes").await;
    sign_in_long_ago(&pool).await;

    assert_eq!(
        problem(&enable(&app, &token, &credential, 1).await),
        "reauthentication-required"
    );
    assert_eq!(
        problem(&protect(&app, &token, &notes, 1).await),
        "reauthentication-required"
    );
}
