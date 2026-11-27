use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::*;

const OLD: &str = "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK";
const NEW: &str = "Xw7pR2nLq9TzVb4KsYc1MdHg6JfAe3Uo8NiZ5tQrWmEx";
const OTHER: &str = "Lm4QzT8vR1nXc6WbPj3KsH9yGd2FaE7oUi5NtVrBwCqZ";
const TENANT: TenantId = TenantId::DEFAULT;

fn key(secret: &str) -> SecretBox {
    SecretBox::new(secret)
}

async fn user(db: &Db, name: &str) -> UserId {
    db.create_user(TENANT, name, "hash").await.expect("user").id
}

/// Gives `user` an authenticator app whose secret is `plain`, sealed with `with`.
async fn app(db: &Db, user: UserId, with: &SecretBox, plain: &[u8]) {
    let sealed = with.seal(TENANT, user, plain).expect("seal");
    db.start_totp(TENANT, user, &sealed).await.expect("totp");
}

async fn sign_in(db: &Db, user: UserId) {
    let id = Uuid::now_v7();
    db.start_session(
        id,
        user,
        None,
        &crate::sessions::signed_in_now(crate::sessions::PASSWORD_METHOD),
        id.as_bytes(),
        OffsetDateTime::now_utc() + Duration::days(1),
    )
    .await
    .expect("session");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_first_check_is_recorded_only_for_the_key_that_opens_everything(pool: PgPool) {
    let db = Db::from_pool(pool);
    assert_eq!(
        check(&db, TENANT, None).await.expect("check"),
        KeyCheck::Unused
    );
    let alice = user(&db, "alice").await;
    app(&db, alice, &key(OLD), b"app").await;

    assert!(matches!(
        check(&db, TENANT, None).await,
        Err(SecretsKeyError::Missing)
    ));
    assert!(matches!(
        check(&db, TENANT, Some(&key(NEW))).await,
        Err(SecretsKeyError::Wrong)
    ));
    assert_eq!(db.secrets_key_check(TENANT).await.expect("db"), None);
    assert_eq!(
        check(&db, TENANT, Some(&key(OLD))).await.expect("check"),
        KeyCheck::Recorded
    );
    assert_eq!(
        check(&db, TENANT, Some(&key(OLD))).await.expect("check"),
        KeyCheck::Matches
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_recorded_check_refuses_another_key_or_none(pool: PgPool) {
    let db = Db::from_pool(pool);
    // Nothing is sealed yet, so any key would open everything: the first one is recorded.
    assert_eq!(
        check(&db, TENANT, Some(&key(OLD))).await.expect("check"),
        KeyCheck::Recorded
    );
    assert!(matches!(
        check(&db, TENANT, Some(&key(NEW))).await,
        Err(SecretsKeyError::Wrong)
    ));
    assert!(matches!(
        check(&db, TENANT, None).await,
        Err(SecretsKeyError::Missing)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rotation_seals_everything_with_the_new_key_and_signs_everyone_out(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let (old, new) = (key(OLD), key(NEW));
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    app(&db, alice, &old, b"alice's app").await;
    app(&db, bob, &old, b"bob's app").await;
    sign_in(&db, alice).await;
    sign_in(&db, bob).await;
    check(&db, TENANT, Some(&old)).await.expect("check");

    let done = rotate(&db, TENANT, std::slice::from_ref(&old), &new)
        .await
        .expect("rotate");
    assert_eq!(
        done,
        Rotation::Rotated {
            authenticator_apps: 2,
            sessions_ended: 2
        }
    );

    let sealed = db.sealed_secrets(TENANT).await.expect("db");
    assert_eq!(
        new.open(TENANT, alice, &sealed.totp[0].sealed)
            .expect("open"),
        b"alice's app"
    );
    assert_eq!(
        new.open(TENANT, bob, &sealed.totp[1].sealed).expect("open"),
        b"bob's app"
    );
    assert!(old.open(TENANT, alice, &sealed.totp[0].sealed).is_err());
    assert_eq!(
        check(&db, TENANT, Some(&new)).await.expect("check"),
        KeyCheck::Matches
    );
    assert!(matches!(
        check(&db, TENANT, Some(&old)).await,
        Err(SecretsKeyError::Wrong)
    ));
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(sessions, 0);

    // Run again after a crash: the database already has the new key.
    assert_eq!(
        rotate(&db, TENANT, std::slice::from_ref(&old), &new)
            .await
            .expect("rotate"),
        Rotation::AlreadyDone
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn rotation_opens_with_the_old_previous_key_too(pool: PgPool) {
    let db = Db::from_pool(pool);
    let alice = user(&db, "alice").await;
    let bob = user(&db, "bob").await;
    // Bob's app was sealed before an earlier rotation and never sealed again.
    app(&db, alice, &key(OLD), b"alice's app").await;
    app(&db, bob, &key(OTHER), b"bob's app").await;
    let new = key(NEW);

    assert!(matches!(
        rotate(&db, TENANT, &[key(OLD)], &new).await,
        Err(SecretsKeyError::Unopened(_))
    ));
    assert_eq!(db.secrets_key_check(TENANT).await.expect("db"), None);

    rotate(&db, TENANT, &[key(OLD), key(OTHER)], &new)
        .await
        .expect("rotate");
    let sealed = db.sealed_secrets(TENANT).await.expect("db");
    let bobs = sealed.totp.iter().find(|row| row.user == bob).expect("bob");
    assert_eq!(
        new.open(TENANT, bob, &bobs.sealed).expect("open"),
        b"bob's app"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn rotation_refuses_a_current_key_that_is_not_the_checked_one(pool: PgPool) {
    let db = Db::from_pool(pool);
    check(&db, TENANT, Some(&key(OLD))).await.expect("check");
    assert!(matches!(
        rotate(&db, TENANT, &[key(OTHER)], &key(NEW)).await,
        Err(SecretsKeyError::Wrong)
    ));
    assert!(matches!(
        rotate(&db, TENANT, &[], &key(NEW)).await,
        Err(SecretsKeyError::Missing)
    ));
    assert_eq!(
        check(&db, TENANT, Some(&key(OLD))).await.expect("check"),
        KeyCheck::Matches
    );
}
