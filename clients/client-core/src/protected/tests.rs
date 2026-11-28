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
fn shared_keys_are_the_folder_keys_and_count_as_activity() {
    let mut unlocked = UnlockedFolders::default();
    let keys = keys();
    let recipient = keys.recipient().to_string();
    unlocked
        .unlock(FOLDER, keys, &recipient, Duration::ZERO)
        .expect("unlocked");
    let shared = unlocked
        .shared_keys(FOLDER, 10 * MINUTE)
        .expect("unlocked keys");
    assert_eq!(shared.recipient().to_string(), recipient);
    assert!(unlocked.is_unlocked(FOLDER, 20 * MINUTE));
    unlocked.lock(FOLDER);
    assert!(unlocked.shared_keys(FOLDER, 20 * MINUTE).is_none());
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

mod security_keys {
    use protocol::{AddUnlocker, UserId, Vault, VaultUnlocker};
    use time::macros::datetime;
    use uuid::Uuid;
    use vault_core::{RecoveryKey, SoftAuthenticator};

    use super::*;

    const USER: UserId = UserId(Uuid::from_bytes([7; 16]));
    const RP: &str = "files.example.org";

    fn listed(request: AddUnlocker, id: u8) -> VaultUnlocker {
        VaultUnlocker {
            id: Uuid::from_bytes([id; 16]),
            kind: request.kind,
            name: request.name,
            credential_id: request.credential_id,
            rp_id: request.rp_id,
            prf_salt: request.prf_salt,
            wrapped_vault_key: request.wrapped_vault_key,
            created_at: datetime!(2026-11-28 10:00 UTC),
        }
    }

    /// A vault with one folder key on `auth`.
    fn vault_with_key(auth: &mut SoftAuthenticator) -> Vault {
        let credential = auth.register(RP);
        let request =
            enable_folder_key(auth, None, USER, " YubiKey ", &credential, RP).expect("enabled");
        assert_eq!(request.epoch, 1);
        assert_eq!(request.name.as_deref(), Some("YubiKey"));
        Vault {
            epoch: 1,
            unlockers: vec![listed(request, 1)],
        }
    }

    fn folder(request: &protocol::ProtectFolder) -> ProtectedFolder {
        ProtectedFolder {
            node_id: request.node_id,
            lock: FolderLock::SecurityKey,
            wrapped_secret: request.wrapped_secret.clone(),
            epoch: request.epoch,
            kdf: None,
            recovery_wrapped_secret: None,
            recipient: request.recipient.clone(),
            format: 1,
            state: ProtectionState::Protected,
            created_at: datetime!(2026-11-28 10:00 UTC),
        }
    }

    #[test]
    fn a_folder_protected_with_the_vault_opens_with_one_touch() {
        let mut auth = SoftAuthenticator::new();
        let vault = vault_with_key(&mut auth);
        let [key] = folder_keys(&vault).try_into().expect("one key");
        let touches = auth.touches;
        let (request, keys) = with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut auth,
                key: &key,
            },
            |vault_key| protect_with_vault(&vault, vault_key, FOLDER),
        )
        .expect("protected");
        assert_eq!(auth.touches, touches + 1);
        assert_eq!(request.lock, FolderLock::SecurityKey);
        assert_eq!(request.epoch, Some(1));
        let sealed = keys
            .seal_name(&FOLDER.0, vault_core::NodeKind::File, "tax.pdf")
            .expect("sealed");

        let listed = folder(&request);
        let opened = with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut auth,
                key: &key,
            },
            |vault_key| open_with_vault(&vault, vault_key, &listed),
        )
        .expect("opened");
        assert_eq!(
            opened
                .open_name(&FOLDER.0, vault_core::NodeKind::File, &sealed)
                .expect("name"),
            "tax.pdf"
        );
        assert_eq!(auth.touches, touches + 2);
    }

    #[test]
    fn another_users_vault_and_a_password_folder_do_not_open() {
        let mut auth = SoftAuthenticator::new();
        let vault = vault_with_key(&mut auth);
        let [key] = folder_keys(&vault).try_into().expect("one key");
        let other = UserId(Uuid::from_bytes([8; 16]));
        let refused = with_vault(
            &vault,
            other,
            VaultProof::Key {
                auth: &mut auth,
                key: &key,
            },
            |_| Ok(()),
        );
        assert!(matches!(refused, Err(ProtectedError::KeyDoesNotOpen)));

        let new = protect_with_password(FOLDER, "correct horse").expect("protected");
        let password_folder = super::listed(&new);
        let refused = with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut auth,
                key: &key,
            },
            |vault_key| open_with_vault(&vault, vault_key, &password_folder),
        );
        assert!(matches!(refused, Err(ProtectedError::NotSecurityKey)));
    }

    #[test]
    fn the_vault_recovery_key_opens_it_and_enables_another_key() {
        let mut auth = SoftAuthenticator::new();
        let mut vault = vault_with_key(&mut auth);
        let [key] = folder_keys(&vault).try_into().expect("one key");
        assert!(recovery_unlocker(&vault).is_none());
        let refused = with_vault(
            &vault,
            USER,
            VaultProof::<SoftAuthenticator>::RecoveryKey("AAAAA"),
            |_| Ok(()),
        );
        assert!(matches!(refused, Err(ProtectedError::NoRecoveryKey)));

        let (request, recovery) = with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut auth,
                key: &key,
            },
            |vault_key| new_vault_recovery_key(&vault, vault_key, USER),
        )
        .expect("recovery key");
        assert_eq!(request.epoch, 1);
        vault.unlockers.push(listed(request, 2));
        let typed = recovery.to_display().to_lowercase();

        let mut second = SoftAuthenticator::new();
        let credential = second.register(RP);
        let added = with_vault(
            &vault,
            USER,
            VaultProof::<SoftAuthenticator>::RecoveryKey(&typed),
            |vault_key| {
                enable_folder_key(
                    &mut second,
                    Some((&vault, vault_key)),
                    USER,
                    "Spare",
                    &credential,
                    RP,
                )
            },
        )
        .expect("added");
        vault.unlockers.push(listed(added, 3));
        let keys = folder_keys(&vault);
        assert_eq!(keys.len(), 2);
        let spare = &keys[1];
        assert_eq!(spare.name, "Spare");
        with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut second,
                key: spare,
            },
            |_| Ok(()),
        )
        .expect("the spare opens the same vault");

        let wrong = RecoveryKey::generate().to_display();
        let refused = with_vault(
            &vault,
            USER,
            VaultProof::<SoftAuthenticator>::RecoveryKey(&wrong),
            |_| Ok(()),
        );
        assert!(matches!(refused, Err(ProtectedError::WrongRecoveryKey)));
    }

    #[test]
    fn a_key_that_does_not_hold_the_credential_says_so() {
        let mut auth = SoftAuthenticator::new();
        let vault = vault_with_key(&mut auth);
        let [key] = folder_keys(&vault).try_into().expect("one key");
        let mut other = SoftAuthenticator::new();
        let refused = with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut other,
                key: &key,
            },
            |_| Ok(()),
        );
        assert!(
            matches!(refused, Err(ProtectedError::SecurityKey(text)) if text == "unknown credential")
        );
    }

    #[test]
    fn a_folder_wrapped_under_another_epoch_is_refused() {
        let mut auth = SoftAuthenticator::new();
        let vault = vault_with_key(&mut auth);
        let [key] = folder_keys(&vault).try_into().expect("one key");
        let (request, _) = with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut auth,
                key: &key,
            },
            |vault_key| protect_with_vault(&vault, vault_key, FOLDER),
        )
        .expect("protected");
        let mut listed = folder(&request);
        listed.epoch = Some(2);
        let refused = with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut auth,
                key: &key,
            },
            |vault_key| open_with_vault(&vault, vault_key, &listed),
        );
        assert!(matches!(refused, Err(ProtectedError::VaultChanged)));
    }

    /// Answers for the credentials of either key.
    struct Both<'a>(&'a mut SoftAuthenticator, &'a mut SoftAuthenticator);

    impl vault_core::Authenticator for Both<'_> {
        fn prf(
            &mut self,
            credential: vault_core::CredentialRef<'_>,
            input: &[u8; 32],
        ) -> vault_core::Result<vault_core::PrfOutput> {
            self.0
                .prf(credential, input)
                .or_else(|_| self.1.prf(credential, input))
        }
    }

    /// The vault as the server keeps it after `rotation`.
    fn rotated(vault: &Vault, rotation: &Rotation) -> Vault {
        let mut unlockers: Vec<VaultUnlocker> = vault
            .unlockers
            .iter()
            .filter(|unlocker| !rotation.request.remove.contains(&unlocker.id))
            .map(|unlocker| {
                let new = rotation
                    .request
                    .unlockers
                    .iter()
                    .find(|new| new.id == unlocker.id)
                    .expect("every unlocker that stays is wrapped anew");
                VaultUnlocker {
                    prf_salt: new.prf_salt.clone().or_else(|| unlocker.prf_salt.clone()),
                    wrapped_vault_key: new.wrapped_vault_key.clone(),
                    ..unlocker.clone()
                }
            })
            .collect();
        if let Some((request, _)) = &rotation.recovery {
            assert_eq!(request.epoch, vault.epoch + 1);
            unlockers.push(listed(request.clone(), 9));
        }
        Vault {
            epoch: vault.epoch + 1,
            unlockers,
        }
    }

    /// The folder as `GET /protected` lists it after `rotation`.
    fn rotated_folder(folder: &ProtectedFolder, rotation: &Rotation) -> ProtectedFolder {
        let [new] = rotation.request.folders.as_slice() else {
            panic!("one folder");
        };
        assert_eq!(new.node_id, folder.node_id);
        ProtectedFolder {
            wrapped_secret: new.wrapped_secret.clone(),
            epoch: folder.epoch.map(|epoch| epoch + 1),
            ..folder.clone()
        }
    }

    /// A vault with key A on `a`, key B on `b` and a recovery key, and a folder protected
    /// with it whose name "tax.pdf" was sealed.
    fn two_keys_and_a_folder(
        a: &mut SoftAuthenticator,
        b: &mut SoftAuthenticator,
    ) -> (Vault, RecoveryKey, ProtectedFolder, String) {
        let mut vault = vault_with_key(a);
        let [first] = folder_keys(&vault).try_into().expect("one key");
        let credential = b.register(RP);
        let (added, recovery, (request, keys)) = with_vault(
            &vault,
            USER,
            VaultProof::Key {
                auth: &mut *a,
                key: &first,
            },
            |vault_key| {
                let added = enable_folder_key(
                    &mut *b,
                    Some((&vault, vault_key)),
                    USER,
                    "B",
                    &credential,
                    RP,
                )?;
                let recovery = new_vault_recovery_key(&vault, vault_key, USER)?;
                Ok((
                    added,
                    recovery,
                    protect_with_vault(&vault, vault_key, FOLDER)?,
                ))
            },
        )
        .expect("set up");
        vault.unlockers.push(listed(added, 2));
        vault.unlockers.push(listed(recovery.0, 3));
        let sealed = keys
            .seal_name(&FOLDER.0, vault_core::NodeKind::File, "tax.pdf")
            .expect("sealed");
        (vault, recovery.1, folder(&request), sealed)
    }

    fn opens_tax_pdf(
        vault: &Vault,
        proof: VaultProof<'_, SoftAuthenticator>,
        folder: &ProtectedFolder,
        sealed: &str,
    ) -> bool {
        with_vault(vault, USER, proof, |vault_key| {
            open_with_vault(vault, vault_key, folder)
        })
        .and_then(|keys| Ok(keys.open_name(&FOLDER.0, vault_core::NodeKind::File, sealed)?))
        .is_ok_and(|name| name == "tax.pdf")
    }

    #[test]
    fn rotating_after_losing_a_key_leaves_it_out_and_replaces_the_recovery_key() {
        let (mut a, mut b) = (SoftAuthenticator::new(), SoftAuthenticator::new());
        let (vault, old_recovery, folder, sealed) = two_keys_and_a_folder(&mut a, &mut b);
        let keys = folder_keys(&vault);
        let (key_a, key_b) = (&keys[0], &keys[1]);
        let touches = a.touches;

        let rotation = rotate_vault(
            &mut a,
            &vault,
            USER,
            RotationProof::Key(key_a),
            std::slice::from_ref(&folder),
            &[key_b.id],
        )
        .expect("rotated");
        // One touch to open the vault, one to wrap it anew for A.
        assert_eq!(a.touches, touches + 2);
        assert_eq!(rotation.request.epoch, 1);
        let old_recovery_id = recovery_unlocker(&vault).expect("recovery").id;
        assert_eq!(rotation.request.remove, [key_b.id, old_recovery_id]);

        let new_vault = rotated(&vault, &rotation);
        let new_folder = rotated_folder(&folder, &rotation);
        let new_keys = folder_keys(&new_vault);
        assert_eq!(new_keys.len(), 1);
        let proof = VaultProof::Key {
            auth: &mut a,
            key: &new_keys[0],
        };
        assert!(opens_tax_pdf(&new_vault, proof, &new_folder, &sealed));

        let (_, new_recovery) = rotation.recovery.as_ref().expect("a new recovery key");
        let typed = new_recovery.to_display();
        let proof = VaultProof::<SoftAuthenticator>::RecoveryKey(&typed);
        assert!(opens_tax_pdf(&new_vault, proof, &new_folder, &sealed));
        let old = old_recovery.to_display();
        let proof = VaultProof::<SoftAuthenticator>::RecoveryKey(&old);
        assert!(!opens_tax_pdf(&new_vault, proof, &new_folder, &sealed));
    }

    #[test]
    fn rotating_with_the_recovery_key_keeps_it_and_asks_every_key_that_stays() {
        let (mut a, mut b) = (SoftAuthenticator::new(), SoftAuthenticator::new());
        let (vault, recovery, folder, sealed) = two_keys_and_a_folder(&mut a, &mut b);
        let typed = recovery.to_display();

        let rotation = rotate_vault(
            &mut Both(&mut a, &mut b),
            &vault,
            USER,
            RotationProof::RecoveryKey(&typed),
            std::slice::from_ref(&folder),
            &[],
        )
        .expect("rotated");
        assert!(rotation.recovery.is_none());
        assert!(rotation.request.remove.is_empty());
        assert_eq!(rotation.request.unlockers.len(), 3);

        let new_vault = rotated(&vault, &rotation);
        let new_folder = rotated_folder(&folder, &rotation);
        let keys = folder_keys(&new_vault);
        let proof = VaultProof::Key {
            auth: &mut b,
            key: &keys[1],
        };
        assert!(opens_tax_pdf(&new_vault, proof, &new_folder, &sealed));
        let proof = VaultProof::<SoftAuthenticator>::RecoveryKey(&typed);
        assert!(opens_tax_pdf(&new_vault, proof, &new_folder, &sealed));
        // The old wrapping no longer matters: the old vault key does not open the new folder.
        let old_keys = folder_keys(&vault);
        let proof = VaultProof::Key {
            auth: &mut a,
            key: &old_keys[0],
        };
        assert!(!opens_tax_pdf(&vault, proof, &new_folder, &sealed));
    }

    #[test]
    fn rotating_refuses_a_folder_of_another_epoch_and_a_key_that_is_not_plugged_in() {
        let (mut a, mut b) = (SoftAuthenticator::new(), SoftAuthenticator::new());
        let (vault, _, folder, _) = two_keys_and_a_folder(&mut a, &mut b);
        let keys = folder_keys(&vault);
        let mut stale = folder.clone();
        stale.epoch = Some(0);
        let refused = rotate_vault(
            &mut a,
            &vault,
            USER,
            RotationProof::Key(&keys[0]),
            &[stale],
            &[keys[1].id],
        );
        assert!(matches!(refused, Err(ProtectedError::VaultChanged)));

        // B stays but only A answers.
        let refused = rotate_vault(
            &mut a,
            &vault,
            USER,
            RotationProof::Key(&keys[0]),
            std::slice::from_ref(&folder),
            &[],
        );
        assert!(matches!(refused, Err(ProtectedError::SecurityKey(_))));
    }
}
