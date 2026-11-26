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
    assert_eq!(password.json()["errors"][0]["pointer"], "/password");
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

const FOLDER_PASSWORD: &str = "correct horse battery";
const AGE_HEADER: &[u8] = b"age-encryption.org/v1\n";

async fn protect_with_password(app: &TestApp, token: &str, node: &str) -> common::Response {
    send(
        app,
        Method::POST,
        "/api/v1/protected",
        token,
        json!({ "node_id": node, "lock": "password", "password": FOLDER_PASSWORD }),
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

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_folder_is_encrypted_on_disk_and_opens_with_its_password(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let alice = app.user_id("alice").await.to_string();
    let notes = folder(&app, &token, "Notes").await;
    let file = upload_into(&app, &token, &notes, "plan.txt", "first draft").await;
    upload_into(&app, &token, &notes, "plan.txt", "second draft").await;

    let protecting = protect_with_password(&app, &token, &notes).await;
    assert_eq!(
        protecting.status,
        StatusCode::CREATED,
        "{}",
        protecting.json()
    );
    assert_eq!(protecting.json()["state"], "protecting");
    assert!(protecting.json().get("wrapped_secret").is_none());
    let mut state = String::new();
    for _ in 0..100 {
        let listed = app.get("/api/v1/protected", &token).await.json();
        state = listed[0]["state"].as_str().unwrap_or_default().to_owned();
        if state == "protected" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(state, "protected");
    // The current content and the older version are both age files now.
    let blobs = stored_blobs(&app, &alice);
    assert_eq!(blobs.len(), 2);
    assert!(blobs.iter().all(|blob| blob.starts_with(AGE_HEADER)));
    assert!(
        !blobs
            .iter()
            .any(|blob| blob.windows(5).any(|w| w == b"draft"))
    );

    let content = format!("/api/v1/files/{file}/content");
    let old = format!("/api/v1/files/{file}/versions/1/content");
    let unlock_uri = format!("/api/v1/protected/{notes}/unlock");
    // Setting the password left the folder unlocked for this session; lock it again.
    assert_eq!(app.get(&content, &token).await.body, b"second draft");
    let locked = delete(&app, &unlock_uri, &token).await;
    assert_eq!(locked.status, StatusCode::NO_CONTENT);
    assert_eq!(problem(&app.get(&content, &token).await), "locked");
    let unlock = |password: &str| {
        send(
            &app,
            Method::POST,
            &unlock_uri,
            &token,
            json!({ "password": password }),
        )
    };
    assert_eq!(problem(&unlock("not the password").await), "wrong-password");
    let unlocked = unlock(FOLDER_PASSWORD).await;
    assert_eq!(unlocked.status, StatusCode::OK, "{}", unlocked.json());

    let current = app.get(&content, &token).await;
    assert_eq!(current.status, StatusCode::OK);
    assert_eq!(current.body, b"second draft");
    assert_eq!(app.get(&old, &token).await.body, b"first draft");
    let range = app
        .send_with(
            Method::GET,
            &content,
            Some(&token),
            Body::empty(),
            None,
            &[("range", "bytes=7-11")],
        )
        .await;
    assert_eq!(range.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(range.body, b"draft");

    // A new file is encrypted as it is stored, and reads back plain.
    let new = upload_into(&app, &token, &notes, "todo.txt", "buy milk").await;
    assert_eq!(stored_blobs(&app, &alice).len(), 3);
    assert!(
        stored_blobs(&app, &alice)
            .iter()
            .all(|blob| blob.starts_with(AGE_HEADER))
    );
    assert_eq!(
        app.get(&format!("/api/v1/files/{new}/content"), &token)
            .await
            .body,
        b"buy milk"
    );

    // Nothing moves in or out: what is inside is encrypted and what is outside is not.
    let moved = send(
        &app,
        Method::POST,
        "/api/v1/nodes/move",
        &token,
        json!({ "ids": [new], "parent_id": null }),
    )
    .await;
    assert_eq!(problem(&moved), "protected");

    let locked = delete(&app, &format!("/api/v1/protected/{notes}/unlock"), &token).await;
    assert_eq!(locked.status, StatusCode::NO_CONTENT);
    assert_eq!(problem(&app.get(&content, &token).await), "locked");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unlock_is_for_one_session_only(pool: PgPool) {
    let app = setup(pool).await;
    let laptop = app.user_token("alice").await;
    let phone = app
        .sign_in_from("alice", uuid::Uuid::now_v7(), "Phone")
        .await;
    let notes = folder(&app, &laptop, "Notes").await;
    let protected = protect_with_password(&app, &laptop, &notes).await;
    assert_eq!(protected.json()["state"], "protected");
    let file = upload_into(&app, &laptop, &notes, "a.txt", "secret").await;
    let content = format!("/api/v1/files/{file}/content");
    send(
        &app,
        Method::POST,
        &format!("/api/v1/protected/{notes}/unlock"),
        &laptop,
        json!({ "password": FOLDER_PASSWORD }),
    )
    .await;
    assert_eq!(app.get(&content, &laptop).await.body, b"secret");
    assert_eq!(problem(&app.get(&content, &phone).await), "locked");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_folder_needs_a_good_password_and_finishes_on_the_server(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    let weak = send(
        &app,
        Method::POST,
        "/api/v1/protected",
        &token,
        json!({ "node_id": notes, "lock": "password", "password": "short" }),
    )
    .await;
    assert_eq!(weak.json()["errors"][0]["pointer"], "/password");
    assert_eq!(
        protect_with_password(&app, &token, &notes).await.status,
        StatusCode::CREATED
    );
    let id = |action: &str| format!("/api/v1/protected/{notes}{action}");
    let finish = send(&app, Method::POST, &id("/finish"), &token, json!({})).await;
    assert_eq!(problem(&finish), "protected");
    // The server drops a password folder's protection itself, once it decrypted it.
    let remove = delete(&app, &id(""), &token).await;
    assert_eq!(problem(&remove), "protected");
}

/// Waits for the server to finish protecting the user's only password folder.
async fn wait_protected(app: &TestApp, token: &str) {
    for _ in 0..100 {
        let listed = app.get("/api/v1/protected", token).await.json();
        if listed[0]["state"] == "protected" {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the folder was not protected in time");
}

/// The names the database holds for the user's nodes.
async fn stored_names(pool: &PgPool, user: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT n.name FROM nodes n JOIN users u ON u.id = n.owner_id WHERE u.username = $1",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .expect("names")
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
        .expect("listing")
        .iter()
        .map(|node| node["name"].as_str().expect("name"))
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn names_in_a_password_folder_are_sealed_and_need_an_unlock(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    upload_into(&app, &token, &notes, "plan.txt", "draft").await;
    let year = make_folder_in(&app, &token, &notes, "2025").await.json()["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let taxes = upload_into(&app, &token, &year, "taxes.pdf", "numbers").await;

    assert_eq!(
        protect_with_password(&app, &token, &notes).await.status,
        StatusCode::CREATED
    );
    wait_protected(&app, &token).await;
    // The folder keeps its name; what is inside is sealed in the database.
    let stored = stored_names(&pool, "alice").await;
    assert!(stored.contains(&"Notes".to_owned()));
    for plain in ["plan.txt", "2025", "taxes.pdf"] {
        assert!(
            !stored.iter().any(|name| name == plain),
            "{plain} is stored in plain"
        );
    }

    // Protecting left the folder unlocked for this session, which sees plain names.
    let listing = app
        .get(&format!("/api/v1/nodes?parent_id={notes}"), &token)
        .await;
    assert_eq!(listing.status, StatusCode::OK, "{}", listing.json());
    assert_eq!(names_of(&listing.json()), ["plan.txt", "2025"]);
    let node = app.get(&format!("/api/v1/nodes/{taxes}"), &token).await;
    assert_eq!(node.json()["name"], "taxes.pdf");
    let ancestors = app
        .get(&format!("/api/v1/nodes/{taxes}/ancestors"), &token)
        .await;
    assert_eq!(names_of(&ancestors.json()), ["Notes", "2025"]);
    let download = app
        .get(&format!("/api/v1/files/{taxes}/content"), &token)
        .await;
    let disposition = download.headers["content-disposition"]
        .to_str()
        .expect("header");
    assert!(disposition.contains("taxes.pdf"), "{disposition}");

    // New names are sealed too, and come back plain.
    let made = make_folder_in(&app, &token, &notes, "Receipts").await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.json());
    assert_eq!(made.json()["name"], "Receipts");
    let renamed = app
        .json(
            Method::PATCH,
            &format!("/api/v1/nodes/{taxes}"),
            Some(&token),
            json!({ "name": "taxes 2025.pdf" }),
        )
        .await;
    assert_eq!(renamed.json()["name"], "taxes 2025.pdf");
    // Moving within the folder keeps the sealed name readable.
    let moved = app
        .json(
            Method::PATCH,
            &format!("/api/v1/nodes/{taxes}"),
            Some(&token),
            json!({ "parent_id": notes }),
        )
        .await;
    assert_eq!(moved.json()["name"], "taxes 2025.pdf");
    let again = upload_into(&app, &token, &notes, "plan.txt", "draft 2").await;
    let listing = app
        .get(&format!("/api/v1/nodes?parent_id={notes}"), &token)
        .await;
    assert_eq!(
        names_of(&listing.json()),
        ["plan.txt", "taxes 2025.pdf", "2025", "Receipts"]
    );
    let stored = stored_names(&pool, "alice").await;
    for plain in ["Receipts", "taxes 2025.pdf", "plan.txt"] {
        assert!(
            !stored.iter().any(|name| name == plain),
            "{plain} is stored in plain"
        );
    }
    let long = "a".repeat(200);
    let too_long = make_folder_in(&app, &token, &notes, &long).await;
    assert_eq!(problem(&too_long), "invalid-name");

    // Locked, nothing inside can be read or written; the folder itself still shows.
    let unlock_uri = format!("/api/v1/protected/{notes}/unlock");
    delete(&app, &unlock_uri, &token).await;
    let root = app.get("/api/v1/nodes", &token).await;
    assert!(names_of(&root.json()).contains(&"Notes"));
    assert_eq!(
        app.get(&format!("/api/v1/nodes/{notes}"), &token)
            .await
            .status,
        StatusCode::OK
    );
    for uri in [
        format!("/api/v1/nodes?parent_id={notes}"),
        format!("/api/v1/nodes/{again}"),
        format!("/api/v1/nodes/{again}/ancestors"),
        format!("/api/v1/nodes/{again}/versions"),
    ] {
        assert_eq!(problem(&app.get(&uri, &token).await), "locked", "{uri}");
    }
    assert_eq!(
        problem(&make_folder_in(&app, &token, &notes, "More").await),
        "locked"
    );
    let upload = app
        .upload(&token, &format!("parent_id={notes}&name=new.txt"), "x")
        .await;
    assert_eq!(problem(&upload), "locked");
    let rename = app
        .json(
            Method::PATCH,
            &format!("/api/v1/nodes/{again}"),
            Some(&token),
            json!({ "name": "other.txt" }),
        )
        .await;
    assert_eq!(problem(&rename), "locked");
    assert_eq!(
        problem(&delete(&app, &format!("/api/v1/nodes/{again}"), &token).await),
        "locked"
    );
    assert_eq!(
        problem(&delete(&app, &format!("/api/v1/nodes/{notes}"), &token).await),
        "locked"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_folder_with_a_name_too_long_to_seal_is_not_protected(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    make_folder_in(&app, &token, &notes, &"é".repeat(100)).await;
    let refused = protect_with_password(&app, &token, &notes).await;
    assert_eq!(
        refused.json()["errors"][0]["pointer"],
        "/node_id",
        "{}",
        refused.json()
    );
    assert_eq!(app.get("/api/v1/protected", &token).await.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn changes_leave_out_what_a_session_has_not_unlocked(pool: PgPool) {
    let app = setup(pool).await;
    let laptop = app.user_token("alice").await;
    let phone = app
        .sign_in_from("alice", uuid::Uuid::now_v7(), "Phone")
        .await;
    let notes = folder(&app, &laptop, "Notes").await;
    protect_with_password(&app, &laptop, &notes).await;
    let cursor = app.get("/api/v1/changes", &phone).await.json()["cursor"].clone();
    let since = format!("/api/v1/changes?since={cursor}");

    upload_into(&app, &laptop, &notes, "secret.txt", "x").await;
    folder(&app, &laptop, "Public").await;

    let upserted = |response: &common::Response| -> Vec<String> {
        response.json()["changes"]
            .as_array()
            .expect("changes")
            .iter()
            .filter(|change| change["type"] == "upserted")
            .map(|change| change["node"]["name"].as_str().expect("name").to_owned())
            .collect()
    };
    // The laptop unlocked the folder by protecting it; the phone did not.
    let seen = upserted(&app.get(&since, &laptop).await);
    assert!(seen.contains(&"secret.txt".to_owned()), "{seen:?}");
    assert!(seen.contains(&"Public".to_owned()));
    assert_eq!(upserted(&app.get(&since, &phone).await), ["Public"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_resumable_upload_into_a_password_folder_is_sealed(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let notes = folder(&app, &token, "Notes").await;
    protect_with_password(&app, &token, &notes).await;
    let created = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(&token),
            json!({ "parent_id": notes, "name": "scan.pdf", "size": 4 }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json());
    let upload = created.json()["id"].as_str().expect("id").to_owned();
    let appended = app
        .send_with(
            Method::PATCH,
            &format!("/api/v1/uploads/{upload}"),
            Some(&token),
            Body::from("scan"),
            Some("application/offset+octet-stream"),
            &[("upload-offset", "0")],
        )
        .await;
    assert!(appended.status.is_success(), "{}", appended.json());
    let node = &appended.json()["node"];
    assert_eq!(node["name"], "scan.pdf");
    assert_eq!(node["mime"], "application/pdf");
    assert!(
        !stored_names(&pool, "alice")
            .await
            .contains(&"scan.pdf".to_owned())
    );

    // A locked folder takes no upload.
    delete(&app, &format!("/api/v1/protected/{notes}/unlock"), &token).await;
    let refused = app
        .json(
            Method::POST,
            "/api/v1/uploads",
            Some(&token),
            json!({ "parent_id": notes, "name": "other.pdf", "size": 4 }),
        )
        .await;
    assert_eq!(problem(&refused), "locked");
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
    // A shared folder that holds a password folder does not show it.
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
    let secret = upload_into(&app, &token, &notes, "secret.txt", "inside").await;
    upload_into(&app, &token, &notes, "other.txt", "inside too").await;

    let deleted = delete(&app, &format!("/api/v1/nodes/{secret}"), &token).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.json());
    assert_eq!(app.get("/api/v1/trash", &token).await.json(), json!([]));
    assert_eq!(stored_blobs(&app, &alice).len(), 2);

    // A folder holding a locked password folder is deleted only once it is unlocked.
    let unlock_uri = format!("/api/v1/protected/{notes}/unlock");
    delete(&app, &unlock_uri, &token).await;
    let refused = delete(&app, &format!("/api/v1/nodes/{docs}"), &token).await;
    assert_eq!(problem(&refused), "locked");
    send(
        &app,
        Method::POST,
        &unlock_uri,
        &token,
        json!({ "password": FOLDER_PASSWORD }),
    )
    .await;
    let deleted = delete(&app, &format!("/api/v1/nodes/{docs}"), &token).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.json());
    let binned = app.get("/api/v1/trash", &token).await.json();
    assert_eq!(binned.as_array().map(Vec::len), Some(1));
    assert_eq!(binned[0]["node"]["name"], "plain.txt");
    assert_eq!(stored_blobs(&app, &alice).len(), 1);
    assert_eq!(app.get("/api/v1/protected", &token).await.json(), json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_password_folders_password_can_be_changed(pool: PgPool) {
    let app = setup(pool).await;
    let laptop = app.user_token("alice").await;
    let phone = app
        .sign_in_from("alice", uuid::Uuid::now_v7(), "Phone")
        .await;
    let notes = folder(&app, &laptop, "Notes").await;
    protect_with_password(&app, &laptop, &notes).await;
    let unlock_uri = format!("/api/v1/protected/{notes}/unlock");
    let unlock = |token: String, password: &'static str| {
        let uri = unlock_uri.clone();
        let app = &app;
        async move {
            send(
                app,
                Method::POST,
                &uri,
                &token,
                json!({ "password": password }),
            )
            .await
        }
    };
    assert_eq!(
        unlock(phone.clone(), FOLDER_PASSWORD).await.status,
        StatusCode::OK
    );
    let listing = format!("/api/v1/nodes?parent_id={notes}");
    assert_eq!(app.get(&listing, &phone).await.status, StatusCode::OK);

    let change = format!("/api/v1/protected/{notes}/password");
    let weak = send(
        &app,
        Method::POST,
        &change,
        &laptop,
        json!({ "password": "short" }),
    )
    .await;
    assert_eq!(weak.json()["errors"][0]["pointer"], "/password");
    let changed = send(
        &app,
        Method::POST,
        &change,
        &laptop,
        json!({ "password": "a brand new folder password" }),
    )
    .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.json());
    // The phone is locked out and needs the new password; the laptop keeps the folder open.
    assert_eq!(problem(&app.get(&listing, &phone).await), "locked");
    assert_eq!(app.get(&listing, &laptop).await.status, StatusCode::OK);
    assert_eq!(
        problem(&unlock(phone.clone(), FOLDER_PASSWORD).await),
        "wrong-password"
    );
    assert_eq!(
        unlock(phone.clone(), "a brand new folder password")
            .await
            .status,
        StatusCode::OK
    );

    // Changing it needs the folder unlocked.
    delete(&app, &unlock_uri, &phone).await;
    let locked = send(
        &app,
        Method::POST,
        &change,
        &phone,
        json!({ "password": FOLDER_PASSWORD }),
    )
    .await;
    assert_eq!(problem(&locked), "locked");
}

#[sqlx::test(migrations = "../../migrations")]
async fn unprotecting_a_password_folder_decrypts_it_in_place(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let alice = app.user_id("alice").await.to_string();
    let notes = folder(&app, &token, "Notes").await;
    let file = upload_into(&app, &token, &notes, "plan.txt", "first draft").await;
    upload_into(&app, &token, &notes, "plan.txt", "second draft").await;
    id_in(&app, &token, &notes, "2025").await;
    protect_with_password(&app, &token, &notes).await;
    wait_protected(&app, &token).await;
    assert!(
        stored_blobs(&app, &alice)
            .iter()
            .all(|blob| blob.starts_with(AGE_HEADER))
    );

    let unlock_uri = format!("/api/v1/protected/{notes}/unlock");
    let unprotect = format!("/api/v1/protected/{notes}/unprotect");
    delete(&app, &unlock_uri, &token).await;
    assert_eq!(
        problem(&send(&app, Method::POST, &unprotect, &token, json!({})).await),
        "locked"
    );
    send(
        &app,
        Method::POST,
        &unlock_uri,
        &token,
        json!({ "password": FOLDER_PASSWORD }),
    )
    .await;
    let started = send(&app, Method::POST, &unprotect, &token, json!({})).await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.json());
    assert_eq!(started.json()["state"], "unprotecting");

    let mut gone = false;
    for _ in 0..100 {
        if app.get("/api/v1/protected", &token).await.json() == json!([]) {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(gone, "the protection was not dropped");
    let blobs = stored_blobs(&app, &alice);
    assert_eq!(blobs.len(), 2);
    assert!(blobs.iter().all(|blob| !blob.starts_with(AGE_HEADER)));
    let stored = stored_names(&pool, "alice").await;
    for plain in ["plan.txt", "2025"] {
        assert!(
            stored.contains(&plain.to_owned()),
            "{plain} is not plain again"
        );
    }
    // An ordinary folder again: no unlock needed.
    delete(&app, &unlock_uri, &token).await;
    let listing = app
        .get(&format!("/api/v1/nodes?parent_id={notes}"), &token)
        .await;
    assert_eq!(names_of(&listing.json()), ["plan.txt", "2025"]);
    let old = app
        .get(&format!("/api/v1/files/{file}/versions/1/content"), &token)
        .await;
    assert_eq!(old.body, b"first draft");
}
