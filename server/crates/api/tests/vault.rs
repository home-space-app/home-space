//! The vault and protected folders: enabling folder keys, the recovery key, rotation, and
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

/// Adds a sign-in passkey made by `device`; returns its id.
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

/// A folder key made on [`HOST_A`], whose credential has the id `credential_id`.
fn folder_key(credential_id: &[u8], epoch: i32) -> Value {
    json!({
        "kind": "security_key",
        "name": "YubiKey",
        "credential_id": b64(credential_id),
        "rp_id": HOST_A,
        "prf_salt": b64(&[3; 32]),
        "wrapped_vault_key": wrapped(1),
        "epoch": epoch,
    })
}

async fn add_unlocker(app: &TestApp, token: &str, unlocker: Value) -> common::Response {
    send(
        app,
        Method::POST,
        "/api/v1/vault/unlockers",
        token,
        unlocker,
    )
    .await
}

async fn enable(app: &TestApp, token: &str, credential_id: &[u8], epoch: i32) -> common::Response {
    add_unlocker(app, token, folder_key(credential_id, epoch)).await
}

/// Where `response`, a refusal, says the fault is.
fn pointer(response: &common::Response) -> Value {
    response.json()["errors"][0]["pointer"].clone()
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
async fn a_folder_key_unlocks_the_vault_and_locks_folders(pool: PgPool) {
    let app = setup(pool.clone()).await;
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

    let mut nameless = folder_key(b"yubikey", 1);
    nameless["name"] = Value::Null;
    let mut elsewhere = folder_key(b"yubikey", 1);
    elsewhere["rp_id"] = json!("files.attacker.example");
    let mut short_salt = folder_key(b"yubikey", 1);
    short_salt["prf_salt"] = json!(b64(&[3; 16]));
    let mut long_name = folder_key(b"yubikey", 1);
    long_name["name"] = json!("k".repeat(65));
    for (refused, at) in [
        (nameless, "/name"),
        (elsewhere, "/rp_id"),
        (short_salt, "/prf_salt"),
        (long_name, "/name"),
        (folder_key(b"", 1), "/credential_id"),
        (folder_key(&[1; 1024], 1), "/credential_id"),
    ] {
        let response = add_unlocker(&app, &token, refused).await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(pointer(&response), at, "{}", response.json());
    }
    let named_recovery = json!({
        "kind": "recovery", "name": "Paper", "wrapped_vault_key": wrapped(4), "epoch": 1,
    });
    assert_eq!(
        pointer(&add_unlocker(&app, &token, named_recovery).await),
        "/kind"
    );

    let enabled = enable(&app, &token, b"yubikey", 1).await;
    assert_eq!(enabled.status, StatusCode::CREATED, "{}", enabled.json());
    assert_eq!(enabled.json()["name"], "YubiKey");
    assert_eq!(enabled.json()["credential_id"], b64(b"yubikey"));
    assert_eq!(enabled.json()["rp_id"], HOST_A);
    assert_eq!(enabled.json()["prf_salt"], b64(&[3; 32]));
    assert_eq!(
        pointer(&enable(&app, &token, b"yubikey", 1).await),
        "/credential_id"
    );

    let vault = app.get("/api/v1/vault", &token).await.json();
    assert_eq!(vault["epoch"], 1);
    assert_eq!(vault["unlockers"][0]["name"], "YubiKey");
    assert!(vault["unlockers"][0].get("credential").is_none(), "{vault}");

    let papers = folder(&app, &token, "Papers").await;
    let protected = protect(&app, &token, &papers, 1).await;
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

    // Sign-in keys come and go without the vault.
    let mut passkey = Authenticator::new();
    let sign_in = add_key(&app, &token, &mut passkey).await;
    // Removing a second factor takes a recent proof with one, as if confirmed with the key.
    sqlx::query("UPDATE sessions SET second_factor_at = now()")
        .execute(&pool)
        .await
        .expect("confirmed with the key");
    let removed = delete(&app, &format!("/api/v1/me/webauthn/{sign_in}"), &token).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.json());
    assert_eq!(app.get("/api/v1/vault", &token).await.json(), vault);

    // The only key of a locked folder stays until another unlocks it too.
    let unlocker = vault["unlockers"][0]["id"].as_str().expect("id");
    let removing = delete(&app, &format!("/api/v1/vault/unlockers/{unlocker}"), &token).await;
    assert_eq!(removing.status, StatusCode::CONFLICT);
    assert_eq!(problem(&removing), "last-unlocker");

    recovery_vault(&app, &token).await;
    let removed = delete(&app, &format!("/api/v1/vault/unlockers/{unlocker}"), &token).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.json());
    let vault = app.get("/api/v1/vault", &token).await.json();
    assert_eq!(vault["unlockers"].as_array().map(Vec::len), Some(1));
    assert_eq!(vault["unlockers"][0]["kind"], "recovery");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_administrators_reset_of_second_factors_keeps_the_vault(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let mut yubikey = Authenticator::new();
    add_key(&app, &token, &mut yubikey).await;
    let enabled = enable(&app, &token, b"yubikey", 1).await;
    assert_eq!(enabled.status, StatusCode::CREATED, "{}", enabled.json());
    let papers = folder(&app, &token, "Papers").await;
    assert_eq!(
        protect(&app, &token, &papers, 1).await.status,
        StatusCode::CREATED
    );

    let admin = app.user_token("admin").await;
    let admin_id = app.user_id("admin").await;
    app.db.set_admin(admin_id, true).await.expect("admin");
    let alice = app.user_id("alice").await;
    let reset = app
        .send(
            Method::DELETE,
            &format!("/api/v1/admin/users/{alice}/second-factors"),
            Some(&admin),
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT, "{}", reset.json());
    let vault = app.db.vault(alice).await.expect("vault").expect("kept");
    assert_eq!(vault.unlockers.len(), 1);
    assert_eq!(
        vault.unlockers[0].credential_id.as_deref(),
        Some(&b"yubikey"[..])
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn rotation_needs_the_current_epoch_and_every_folder(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let kept = enable(&app, &token, b"one", 1).await.json()["id"].clone();
    let lost = enable(&app, &token, b"two", 1).await.json()["id"].clone();
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
    let stale = enable(&app, &token, b"two", 1).await;
    assert_eq!(problem(&stale), "vault-changed");
    assert_eq!(
        enable(&app, &token, b"two", 2).await.status,
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

    let password = protect_with(json!({
        "node_id": notes, "lock": "password", "wrapped_secret": wrapped(2), "recipient": RECIPIENT,
    }))
    .await;
    assert_eq!(password.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(password.json()["errors"][0]["pointer"], "/kdf");
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
    let notes = folder(&app, &token, "Notes").await;
    sign_in_long_ago(&pool).await;

    assert_eq!(
        problem(&enable(&app, &token, b"one", 1).await),
        "reauthentication-required"
    );
    assert_eq!(
        problem(&protect(&app, &token, &notes, 1).await),
        "reauthentication-required"
    );
}

/// The Argon2id parameters a client sends for a password, with a salt of `fill`.
fn kdf(fill: u8) -> Value {
    json!({ "salt": b64(&[fill; 16]), "memory_kib": 65536, "iterations": 3, "parallelism": 1 })
}

/// Protects `node` with a password: the secret wrapped as a client would, for the password
/// and for the folder's recovery key.
async fn protect_with_password(app: &TestApp, token: &str, node: &str) -> common::Response {
    send(
        app,
        Method::POST,
        "/api/v1/protected",
        token,
        json!({
            "node_id": node, "lock": "password", "wrapped_secret": wrapped(5),
            "recipient": RECIPIENT, "kdf": kdf(1), "recovery_wrapped_secret": wrapped(6),
        }),
    )
    .await
}

async fn upload_into(
    app: &TestApp,
    token: &str,
    folder: &str,
    name: &str,
    content: &'static str,
) -> String {
    let uploaded = app
        .upload(token, &format!("parent_id={folder}&name={name}"), content)
        .await;
    assert!(uploaded.status.is_success(), "{}", uploaded.json());
    uploaded.json()["id"].as_str().expect("id").to_owned()
}

/// Every blob stored for `user`, current content and versions.
fn stored_blobs(app: &TestApp, user: &str) -> Vec<Vec<u8>> {
    ["blobs", "versions"]
        .iter()
        .filter_map(|dir| std::fs::read_dir(app.storage().join(dir).join(user)).ok())
        .flatten()
        .map(|entry| std::fs::read(entry.expect("entry").path()).expect("blob"))
        .collect()
}

async fn make_folder_in(app: &TestApp, token: &str, parent: &str, name: &str) -> common::Response {
    app.json(
        Method::POST,
        "/api/v1/folders",
        Some(token),
        json!({ "name": name, "parent_id": parent }),
    )
    .await
}

fn names_of(listing: &Value) -> Vec<&str> {
    listing
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| node["name"].as_str())
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_folder_keeps_only_what_the_client_wrapped(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    let protected = protect_with_password(&app, &token, &notes).await;
    assert_eq!(
        protected.status,
        StatusCode::CREATED,
        "{}",
        protected.json()
    );
    assert_eq!(protected.json()["state"], "protected");

    let listed = app.get("/api/v1/protected", &token).await.json();
    assert_eq!(listed[0]["lock"], "password");
    assert_eq!(listed[0]["wrapped_secret"], wrapped(5));
    assert_eq!(listed[0]["recovery_wrapped_secret"], wrapped(6));
    assert_eq!(listed[0]["kdf"], kdf(1));
    assert_eq!(listed[0]["recipient"], RECIPIENT);
    assert!(listed[0].get("epoch").is_none());

    // The server stores what the client sends, plain or not, and reads nothing.
    let file = upload_into(&app, &token, &notes, "tj7VHBiK2oF", "age bytes").await;
    let content = app
        .get(&format!("/api/v1/files/{file}/content"), &token)
        .await;
    assert_eq!(content.status, StatusCode::OK);
    assert_eq!(content.body, b"age bytes");
    let unlock = send(
        &app,
        Method::POST,
        &format!("/api/v1/protected/{notes}/unlock"),
        &token,
        json!({ "password": "anything" }),
    )
    .await;
    assert_eq!(unlock.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_lock_needs_its_parameters_within_bounds(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    let lock = |kdf: Value, recovery: Value| {
        json!({
            "node_id": notes, "lock": "password", "wrapped_secret": wrapped(5),
            "recipient": RECIPIENT, "kdf": kdf, "recovery_wrapped_secret": recovery,
        })
    };
    let weak =
        json!({ "salt": b64(&[1; 16]), "memory_kib": 1024, "iterations": 3, "parallelism": 1 });
    let short_salt =
        json!({ "salt": b64(&[1; 8]), "memory_kib": 65536, "iterations": 3, "parallelism": 1 });
    for (body, field) in [
        (lock(weak, json!(wrapped(6))), "/kdf"),
        (lock(short_salt, json!(wrapped(6))), "/kdf"),
        (lock(Value::Null, json!(wrapped(6))), "/kdf"),
        (lock(kdf(1), Value::Null), "/recovery_wrapped_secret"),
        (
            lock(kdf(1), json!(b64(&[6; 10]))),
            "/recovery_wrapped_secret",
        ),
    ] {
        let refused = send(&app, Method::POST, "/api/v1/protected", &token, body).await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            refused.json()
        );
        assert_eq!(pointer(&refused), field);
    }
    assert_eq!(app.get("/api/v1/protected", &token).await.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_folder_takes_a_new_wrap_and_keeps_its_recovery_key(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    protect_with_password(&app, &token, &notes).await;
    let uri = format!("/api/v1/protected/{notes}/password");

    let changed = send(
        &app,
        Method::POST,
        &uri,
        &token,
        json!({ "wrapped_secret": wrapped(7), "kdf": kdf(2) }),
    )
    .await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.json());
    assert_eq!(changed.json()["wrapped_secret"], wrapped(7));
    assert_eq!(changed.json()["kdf"], kdf(2));
    assert_eq!(changed.json()["recovery_wrapped_secret"], wrapped(6));

    // With the recovery key, the client replaces it too.
    let recovered = send(
        &app,
        Method::POST,
        &uri,
        &token,
        json!({ "wrapped_secret": wrapped(8), "kdf": kdf(3), "recovery_wrapped_secret": wrapped(9) }),
    )
    .await;
    assert_eq!(recovered.json()["recovery_wrapped_secret"], wrapped(9));
    assert_eq!(recovered.json()["recipient"], RECIPIENT);

    // A security-key folder has no password.
    recovery_vault(&app, &token).await;
    let keys = folder(&app, &token, "Keys").await;
    protect(&app, &token, &keys, 1).await;
    let refused = send(
        &app,
        Method::POST,
        &format!("/api/v1/protected/{keys}/password"),
        &token,
        json!({ "wrapped_secret": wrapped(7), "kdf": kdf(2) }),
    )
    .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);

    sign_in_long_ago(&pool).await;
    let stale = send(
        &app,
        Method::POST,
        &uri,
        &token,
        json!({ "wrapped_secret": wrapped(7), "kdf": kdf(2) }),
    )
    .await;
    assert_eq!(problem(&stale), "reauthentication-required");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_folder_with_content_is_protecting_until_the_client_has_replaced_it(
    pool: PgPool,
) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    upload_into(&app, &token, &notes, "plan.txt", "plain").await;
    let protecting = protect_with_password(&app, &token, &notes).await;
    assert_eq!(protecting.json()["state"], "protecting");
    let id = |action: &str| format!("/api/v1/protected/{notes}{action}");
    let finished = send(&app, Method::POST, &id("/finish"), &token, json!({})).await;
    assert_eq!(finished.json()["state"], "protected", "{}", finished.json());
    let unprotecting = send(&app, Method::POST, &id("/unprotect"), &token, json!({})).await;
    assert_eq!(unprotecting.json()["state"], "unprotecting");
    let removed = delete(&app, &id(""), &token).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.json());
    assert_eq!(app.get("/api/v1/protected", &token).await.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn nothing_moves_across_the_edge_of_a_protected_folder(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    recovery_vault(&app, &token).await;
    let docs = folder(&app, &token, "Docs").await;
    let notes = id_in(&app, &token, &docs, "Notes").await;
    let keys = id_in(&app, &token, &docs, "Keys").await;
    protect_with_password(&app, &token, &notes).await;
    protect(&app, &token, &keys, 1).await;
    let plain = upload_into(&app, &token, &docs, "plain.txt", "outside").await;
    let sealed = upload_into(&app, &token, &notes, "tj7VHBiK2oF", "inside").await;
    let inner = id_in(&app, &token, &notes, "eDnpgp214PuY").await;

    for (id, parent) in [
        (&plain, &notes),
        (&plain, &keys),
        (&sealed, &docs),
        (&sealed, &keys),
    ] {
        let refused = move_node(&app, &token, id, parent).await;
        assert_eq!(problem(&refused), "protected", "{}", refused.json());
    }
    let batch = app
        .json(
            Method::POST,
            "/api/v1/nodes/move",
            Some(&token),
            json!({ "ids": [plain], "parent_id": notes }),
        )
        .await;
    assert_eq!(problem(&batch), "protected", "{}", batch.json());

    // Within the folder, and the folder itself, move as usual.
    let within = move_node(&app, &token, &sealed, &inner).await;
    assert_eq!(within.status, StatusCode::OK, "{}", within.json());
    let elsewhere = folder(&app, &token, "Elsewhere").await;
    let whole = move_node(&app, &token, &notes, &elsewhere).await;
    assert_eq!(whole.status, StatusCode::OK, "{}", whole.json());
}

#[sqlx::test(migrations = "../../migrations")]
async fn nothing_lands_unencrypted_or_renumbered_in_a_protected_folder(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    protect_with_password(&app, &token, &notes).await;
    let keep_both = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(&token),
            json!({ "parent_id": notes, "name": "tj7VHBiK2oF", "size": 3, "keep_both": true }),
        )
        .await;
    assert_eq!(problem(&keep_both), "protected", "{}", keep_both.json());
    let saved = app
        .json(
            Method::POST,
            &format!("/api/v1/transfers/{}/save", uuid::Uuid::now_v7()),
            Some(&token),
            json!({ "parent_id": notes }),
        )
        .await;
    assert_eq!(problem(&saved), "protected", "{}", saved.json());
    let copied = app
        .json(
            Method::POST,
            &format!("/api/v1/shared/{}/copy", uuid::Uuid::now_v7()),
            Some(&token),
            json!({ "node_ids": [uuid::Uuid::now_v7()], "parent_id": notes }),
        )
        .await;
    assert_eq!(problem(&copied), "protected", "{}", copied.json());
}

async fn move_node(app: &TestApp, token: &str, id: &str, parent: &str) -> common::Response {
    app.json(
        Method::PATCH,
        &format!("/api/v1/nodes/{id}"),
        Some(token),
        json!({ "parent_id": parent }),
    )
    .await
}

async fn id_in(app: &TestApp, token: &str, parent: &str, name: &str) -> String {
    let made = make_folder_in(app, token, parent, name).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.json());
    made.json()["id"].as_str().expect("id").to_owned()
}

#[sqlx::test(migrations = "../../migrations")]
async fn password_folders_stay_out_of_search_duplicates_shares_and_transfers(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let alice = app.user_id("alice").await;
    let docs = folder(&app, &token, "Docs").await;
    let notes = id_in(&app, &token, &docs, "Notes").await;
    upload_into(&app, &token, &docs, "copy.txt", "same words").await;
    protect_with_password(&app, &token, &notes).await;
    let inside = upload_into(&app, &token, &notes, "receipt.txt", "same words").await;

    let hits = app.get("/api/v1/search?q=receipt", &token).await.json();
    assert_eq!(hits, json!([]));
    let report = app.get("/api/v1/duplicates", &token).await.json();
    assert_eq!(report["groups"], json!([]));

    for nodes in [json!([notes]), json!([inside])] {
        let refused = app
            .json(
                Method::POST,
                "/api/v1/shares",
                Some(&token),
                json!({ "node_ids": nodes }),
            )
            .await;
        assert_eq!(problem(&refused), "protected", "{}", refused.json());
    }
    // A shared folder that holds a protected folder does not show it.
    let shared = app
        .json(
            Method::POST,
            "/api/v1/shares",
            Some(&token),
            json!({ "node_ids": [docs] }),
        )
        .await;
    assert_eq!(shared.status, StatusCode::CREATED, "{}", shared.json());
    let link = shared.json()["token"].as_str().expect("token").to_owned();
    let listed = app
        .send(
            Method::GET,
            &format!("/api/v1/public/shares/{link}/nodes?parent_id={docs}"),
            None,
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(names_of(&listed.json()), ["copy.txt"]);
    let into = app
        .send(
            Method::GET,
            &format!("/api/v1/public/shares/{link}/nodes?parent_id={notes}"),
            None,
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(into.status, StatusCode::NOT_FOUND);

    let sent = app
        .json(
            Method::POST,
            "/api/v1/transfers",
            Some(&token),
            json!({
                "recipient_id": alice,
                "recipient_device": uuid::Uuid::now_v7(),
                "items": [{ "path": "receipt.txt", "kind": "file", "size": 10, "node_id": inside }],
            }),
        )
        .await;
    assert_eq!(problem(&sent), "protected", "{}", sent.json());
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_in_a_password_folder_skips_the_recycle_bin(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let alice = app.user_id("alice").await.to_string();
    let docs = folder(&app, &token, "Docs").await;
    let notes = id_in(&app, &token, &docs, "Notes").await;
    upload_into(&app, &token, &docs, "plain.txt", "outside").await;
    protect_with_password(&app, &token, &notes).await;
    let sealed = upload_into(&app, &token, &notes, "tj7VHBiK2oF", "age bytes").await;
    upload_into(&app, &token, &notes, "eDnpgp214PuY", "more age bytes").await;

    let deleted = delete(&app, &format!("/api/v1/nodes/{sealed}"), &token).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.json());
    assert_eq!(app.get("/api/v1/trash", &token).await.json(), json!([]));
    assert_eq!(stored_blobs(&app, &alice).len(), 2);

    let deleted = delete(&app, &format!("/api/v1/nodes/{docs}"), &token).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.json());
    let binned = app.get("/api/v1/trash", &token).await.json();
    assert_eq!(binned.as_array().map(Vec::len), Some(1));
    assert_eq!(binned[0]["node"]["name"], "plain.txt");
    assert_eq!(stored_blobs(&app, &alice).len(), 1);
    assert_eq!(app.get("/api/v1/protected", &token).await.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_in_a_security_key_folder_skips_the_recycle_bin(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let alice = app.user_id("alice").await.to_string();
    recovery_vault(&app, &token).await;
    let docs = folder(&app, &token, "Docs").await;
    let keys = id_in(&app, &token, &docs, "Keys").await;
    upload_into(&app, &token, &docs, "plain.txt", "outside").await;
    let protected = protect(&app, &token, &keys, 1).await;
    assert_eq!(
        protected.json()["state"],
        "protected",
        "{}",
        protected.json()
    );
    // The client uploads ciphertext under sealed names; the server stores it like any file.
    let sealed = upload_into(&app, &token, &keys, "tj7VHBiK2oF", "age bytes").await;
    upload_into(&app, &token, &keys, "eDnpgp214PuY", "more age bytes").await;

    let deleted = delete(&app, &format!("/api/v1/nodes/{sealed}"), &token).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.json());
    assert_eq!(app.get("/api/v1/trash", &token).await.json(), json!([]));
    assert_eq!(stored_blobs(&app, &alice).len(), 2);

    let deleted = delete(&app, &format!("/api/v1/nodes/{docs}"), &token).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.json());
    let binned = app.get("/api/v1/trash", &token).await.json();
    assert_eq!(binned.as_array().map(Vec::len), Some(1));
    assert_eq!(binned[0]["node"]["name"], "plain.txt");
    assert_eq!(stored_blobs(&app, &alice).len(), 1);
}
