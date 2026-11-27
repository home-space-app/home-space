//! Checking the passwords and recovery keys of password folders, and how guessing is held up.

use super::*;
use crate::{
    FolderSecretKind,
    throttle::{FREE_SECRET_FAILURES, MAX_SECRET_BLOCK, secret_block},
};

const RIGHT: &str = "right-folder-password";

async fn check(
    auth: &AuthService,
    folder: NodeId,
    kind: FolderSecretKind,
    hash: &str,
    typed: &str,
    from: &str,
) -> Result<(), AuthError> {
    auth.check_folder_password(folder, kind, hash.to_owned(), typed, address(from))
        .await
}

/// Wrong passwords from as many addresses as there are free failures, one each.
async fn guess_from_everywhere(auth: &AuthService, folder: NodeId, hash: &str) {
    for n in 0..FREE_SECRET_FAILURES {
        let from = format!("192.0.2.{n}");
        let wrong = check(
            auth,
            folder,
            FolderSecretKind::Password,
            hash,
            "guess",
            &from,
        )
        .await;
        assert!(
            matches!(wrong, Err(AuthError::InvalidCredentials)),
            "{wrong:?}"
        );
    }
}

#[test]
fn a_folder_waits_at_most_an_hour_over_all_addresses() {
    assert_eq!(secret_block(FREE_SECRET_FAILURES - 1), None);
    assert_eq!(secret_block(FREE_SECRET_FAILURES), Some(FIRST_LOGIN_BLOCK));
    assert_eq!(secret_block(u32::MAX), Some(MAX_SECRET_BLOCK));
    assert_eq!(MAX_SECRET_BLOCK, Duration::hours(1));
}

/// Security: spreading guesses over many addresses gains nothing, since a folder's wrong
/// passwords are counted over all of them together too.
#[sqlx::test(migrations = "../../migrations")]
async fn guessing_a_folder_password_from_many_addresses_has_to_wait(pool: PgPool) {
    let auth = service(pool);
    let folder = NodeId::new();
    let hash = auth.hash_folder_password(RIGHT).await.expect("hash");
    guess_from_everywhere(&auth, folder, &hash).await;

    let blocked = check(
        &auth,
        folder,
        FolderSecretKind::Password,
        &hash,
        RIGHT,
        "198.51.100.7",
    )
    .await;
    assert!(
        matches!(blocked, Err(AuthError::TooManyAttempts { .. })),
        "{blocked:?}"
    );
    let other = NodeId::new();
    check(
        &auth,
        other,
        FolderSecretKind::Password,
        &hash,
        RIGHT,
        "198.51.100.7",
    )
    .await
    .expect("another folder is not held up");
}

/// Guessing the password never keeps the owner from recovering with the recovery key, which
/// is counted on its own.
#[sqlx::test(migrations = "../../migrations")]
async fn the_recovery_key_is_counted_apart_from_the_password(pool: PgPool) {
    let auth = service(pool);
    let folder = NodeId::new();
    let hash = auth.hash_folder_password(RIGHT).await.expect("hash");
    let recovery = auth
        .hash_folder_password("the-recovery-key")
        .await
        .expect("hash");
    guess_from_everywhere(&auth, folder, &hash).await;

    check(
        &auth,
        folder,
        FolderSecretKind::RecoveryKey,
        &recovery,
        "the-recovery-key",
        "192.0.2.1",
    )
    .await
    .expect("recovered");
}

/// A wrong folder password counts against its address as a wrong sign-in does.
#[sqlx::test(migrations = "../../migrations")]
async fn wrong_folder_passwords_count_against_the_address(pool: PgPool) {
    let auth = service(pool.clone());
    let hash = auth.hash_folder_password(RIGHT).await.expect("hash");
    let wrong = check(
        &auth,
        NodeId::new(),
        FolderSecretKind::Password,
        &hash,
        "guess",
        "192.0.2.1",
    )
    .await;
    assert!(matches!(wrong, Err(AuthError::InvalidCredentials)));
    assert_eq!(address_failures(&pool, "192.0.2.1").await, 1);
}
