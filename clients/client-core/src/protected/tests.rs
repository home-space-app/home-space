use std::time::Duration;

use protocol::{FolderLock, Node, NodeId, NodeKind, ProtectedFolder, ProtectionState};
use time::macros::datetime;
use uuid::Uuid;
use vault_core::FolderSecret;

use super::*;

const FOLDER: NodeId = NodeId(Uuid::from_bytes([1; 16]));
const OTHER: NodeId = NodeId(Uuid::from_bytes([2; 16]));
const MINUTE: Duration = Duration::from_secs(60);

fn listed(new: &NewPasswordFolder) -> ProtectedFolder {
    ProtectedFolder {
        node_id: new.request.node_id,
        lock: FolderLock::Password,
        wrapped_secret: new.request.wrapped_secret.clone(),
        epoch: None,
        kdf: new.request.kdf.clone(),
        recovery_wrapped_secret: new.request.recovery_wrapped_secret.clone(),
        recipient: new.request.recipient.clone(),
        format: 1,
        state: ProtectionState::Protected,
        created_at: datetime!(2026-11-27 10:00 UTC),
    }
}

fn node(parent: NodeId, kind: NodeKind, name: &str) -> Node {
    Node {
        id: NodeId(Uuid::from_bytes([9; 16])),
        parent_id: Some(parent),
        kind,
        name: name.to_owned(),
        size: 0,
        sha256: None,
        mime: String::new(),
        version: 1,
        created_at: datetime!(2026-11-27 10:00 UTC),
        updated_at: datetime!(2026-11-27 10:00 UTC),
        edited_on: None,
    }
}

fn keys() -> vault_core::FolderKeys {
    FolderSecret::generate().keys().expect("keys")
}

#[test]
fn the_enclosing_folder_is_the_protected_one_on_the_path() {
    let new = protect_with_password(FOLDER, "correct horse").expect("protected");
    let folders = [listed(&new)];
    assert_eq!(
        enclosing(&folders, [OTHER, FOLDER]).map(|folder| folder.node_id),
        Some(FOLDER)
    );
    assert!(enclosing(&folders, [OTHER]).is_none());
}

#[test]
fn a_password_folder_opens_with_its_password_or_its_recovery_key_only() {
    let new = protect_with_password(FOLDER, "correct horse").expect("protected");
    let folder = listed(&new);
    let recovery = new.recovery_key.to_display();

    let keys = unlock_password_folder(&folder, PasswordProof::Password("correct horse"))
        .expect("password opens");
    assert_eq!(keys.recipient().to_string(), folder.recipient);
    unlock_password_folder(&folder, PasswordProof::RecoveryKey(&recovery)).expect("recovery opens");
    assert!(matches!(
        unlock_password_folder(&folder, PasswordProof::Password("wrong horse")),
        Err(ProtectedError::WrongPassword)
    ));
    assert!(matches!(
        unlock_password_folder(&folder, PasswordProof::RecoveryKey("ABCDE")),
        Err(ProtectedError::WrongRecoveryKey)
    ));
}

#[test]
fn a_forgotten_password_is_replaced_with_the_recovery_key_which_is_replaced_too() {
    let new = protect_with_password(FOLDER, "correct horse").expect("protected");
    let mut folder = listed(&new);
    let recovery = new.recovery_key.to_display();

    let change = change_folder_password(&folder, PasswordProof::RecoveryKey(&recovery), "battery")
        .expect("changed");
    let new_recovery = change
        .recovery_key
        .expect("a new recovery key")
        .to_display();
    folder.wrapped_secret = change.request.wrapped_secret;
    folder.kdf = Some(change.request.kdf);
    folder.recovery_wrapped_secret = change.request.recovery_wrapped_secret;

    unlock_password_folder(&folder, PasswordProof::Password("battery")).expect("new password");
    unlock_password_folder(&folder, PasswordProof::RecoveryKey(&new_recovery))
        .expect("new recovery key");
    assert!(matches!(
        unlock_password_folder(&folder, PasswordProof::RecoveryKey(&recovery)),
        Err(ProtectedError::WrongRecoveryKey)
    ));
}

#[test]
fn a_server_listing_other_password_settings_is_refused_before_deriving() {
    let new = protect_with_password(FOLDER, "correct horse").expect("protected");
    let mut folder = listed(&new);
    if let Some(kdf) = folder.kdf.as_mut() {
        kdf.iterations = 1;
    }
    assert!(matches!(
        unlock_password_folder(&folder, PasswordProof::Password("correct horse")),
        Err(ProtectedError::UnacceptedParameters)
    ));
}

#[test]
fn a_security_key_folder_takes_no_password() {
    let new = protect_with_password(FOLDER, "correct horse").expect("protected");
    let folder = ProtectedFolder {
        lock: FolderLock::SecurityKey,
        ..listed(&new)
    };
    assert!(matches!(
        unlock_password_folder(&folder, PasswordProof::Password("correct horse")),
        Err(ProtectedError::NotPassword)
    ));
}

#[test]
fn an_unlocked_folder_locks_after_fifteen_minutes_without_activity() {
    let mut unlocked = UnlockedFolders::default();
    let keys = keys();
    let recipient = keys.recipient().to_string();
    unlocked
        .unlock(FOLDER, keys, &recipient, Duration::ZERO)
        .expect("unlocked");

    assert!(unlocked.use_keys(FOLDER, 9 * MINUTE).is_some());
    assert_eq!(AUTO_LOCK_AFTER, 15 * MINUTE);
    assert_eq!(unlocked.next_lock_in(9 * MINUTE), Some(15 * MINUTE));
    assert!(unlocked.lock_idle(23 * MINUTE).is_empty());
    assert_eq!(unlocked.lock_idle(24 * MINUTE), [FOLDER]);
    assert!(!unlocked.is_unlocked(FOLDER, 24 * MINUTE));
}

#[test]
fn keys_past_their_idle_limit_are_not_handed_out_even_before_the_timer() {
    let mut unlocked = UnlockedFolders::default();
    let keys = keys();
    let recipient = keys.recipient().to_string();
    unlocked
        .unlock(FOLDER, keys, &recipient, Duration::ZERO)
        .expect("unlocked");
    assert!(unlocked.use_keys(FOLDER, 15 * MINUTE).is_none());
    assert!(unlocked.recipient(FOLDER, Duration::ZERO).is_none());
}

#[test]
fn lock_all_wipes_every_folder() {
    let mut unlocked = UnlockedFolders::default();
    for folder in [FOLDER, OTHER] {
        let keys = keys();
        let recipient = keys.recipient().to_string();
        unlocked
            .unlock(folder, keys, &recipient, Duration::ZERO)
            .expect("unlocked");
    }
    let mut locked = unlocked.lock_all();
    locked.sort_by_key(|folder| folder.0);
    assert_eq!(locked, [FOLDER, OTHER]);
    assert_eq!(unlocked.next_lock_in(Duration::ZERO), None);
}

#[test]
fn keys_whose_recipient_the_server_changed_are_not_kept() {
    let mut unlocked = UnlockedFolders::default();
    let other = keys().recipient().to_string();
    assert!(matches!(
        unlocked.unlock(FOLDER, keys(), &other, Duration::ZERO),
        Err(ProtectedError::RecipientMismatch)
    ));
    assert!(!unlocked.is_unlocked(FOLDER, Duration::ZERO));
}

#[test]
fn a_listing_shows_sealed_names_opened_and_the_rest_as_unreadable() {
    let keys = keys();
    let sealed = seal_name(&keys, FOLDER, NodeKind::File, "taxes.pdf").expect("sealed");
    let listing = open_listing(
        &keys,
        vec![
            node(FOLDER, NodeKind::File, &sealed),
            node(OTHER, NodeKind::File, &sealed),
            node(FOLDER, NodeKind::Folder, "plain"),
        ],
        ProtectionState::Protected,
    );
    let names: Vec<&str> = listing.iter().map(|node| node.name.as_str()).collect();
    assert_eq!(names, ["taxes.pdf", UNREADABLE_NAME, UNREADABLE_NAME]);
    assert_eq!(listing[0].mime, "application/pdf");
}

#[test]
fn a_folder_being_converted_shows_its_plain_names_as_they_are() {
    let keys = keys();
    let sealed = seal_name(&keys, FOLDER, NodeKind::File, "taxes.pdf").expect("sealed");
    for state in [ProtectionState::Protecting, ProtectionState::Unprotecting] {
        let listing = open_listing(
            &keys,
            vec![
                node(FOLDER, NodeKind::File, &sealed),
                Node {
                    mime: "image/png".to_owned(),
                    ..node(FOLDER, NodeKind::File, "plain.png")
                },
            ],
            state,
        );
        let shown: Vec<(&str, &str)> = listing
            .iter()
            .map(|node| (node.name.as_str(), node.mime.as_str()))
            .collect();
        assert_eq!(
            shown,
            [("taxes.pdf", "application/pdf"), ("plain.png", "image/png")]
        );
    }
}

#[test]
fn a_name_too_long_to_seal_says_so() {
    let long = "a".repeat(vault_core::names::MAX_NAME_CHARS + 1);
    assert!(matches!(
        seal_name(&keys(), FOLDER, NodeKind::File, &long),
        Err(ProtectedError::Name(_))
    ));
}

#[test]
fn the_server_takes_the_longest_sealed_name_inside_a_protected_folder() {
    assert_eq!(
        vault_core::names::MAX_SEALED_CHARS,
        protocol::rules::MAX_SEALED_NODE_NAME_CHARS
    );
}

#[test]
fn a_decrypted_file_is_shown_by_its_extension_and_never_as_a_page() {
    assert_eq!(shown_mime("Scan.PDF"), "application/pdf");
    assert_eq!(shown_mime("photo.jpeg"), "image/jpeg");
    assert_eq!(shown_mime("index.html"), "text/plain");
    assert_eq!(shown_mime("drawing.svg"), "text/plain");
    assert_eq!(shown_mime("archive.tar.gz"), ENCRYPTED_MIME);
    assert_eq!(shown_mime("README"), ENCRYPTED_MIME);
}

#[test]
fn a_path_opens_only_the_names_below_the_protected_folder() {
    let keys = keys();
    let sub = NodeId(Uuid::from_bytes([5; 16]));
    let sealed = seal_name(&keys, FOLDER, NodeKind::Folder, "Taxes").expect("sealed");
    let path = vec![
        Node {
            id: OTHER,
            ..node(OTHER, NodeKind::Folder, "Home stuff")
        },
        Node {
            id: FOLDER,
            ..node(OTHER, NodeKind::Folder, "Private")
        },
        Node {
            id: sub,
            ..node(FOLDER, NodeKind::Folder, &sealed)
        },
    ];
    let names: Vec<String> = open_path(&keys, path, FOLDER, ProtectionState::Protected)
        .into_iter()
        .map(|node| node.name)
        .collect();
    assert_eq!(names, ["Home stuff", "Private", "Taxes"]);
}
