//! The vault key and the scoped operations clients call.
//!
//! A security-key folder costs one touch per action: [`with_folder_keys`] asks the key, unwraps
//! the vault key and the folder secret, runs the action, and drops every key before it returns.
//! Password folders keep their keys until Lock ([`unlock_password_folder`]).

use zeroize::Zeroize;

use crate::Result;
use crate::authenticator::{Authenticator, CredentialRef};
use crate::folder::{FolderKeys, FolderSecret};
use crate::kdf::{Kek, PasswordKdf};
use crate::recovery::RecoveryKey;
use crate::secret::secret32;
use crate::wrap::{self, Purpose};

secret32! {
    /// The user's vault key: wraps the secrets of security-key folders. Each unlocker (an
    /// enabled security key, the recovery key) stores its own wrapped copy.
    pub struct VaultKey;
}

impl VaultKey {
    /// A new vault key, made when the user enables the first security key.
    pub fn generate() -> Self {
        Self::random()
    }

    /// Wraps the vault key for an unlocker of `user`.
    pub fn wrap(&self, kek: &Kek, user: &uuid::Uuid) -> Vec<u8> {
        wrap::wrap(kek.expose(), self.expose(), Purpose::VaultKey, user)
    }

    pub fn unwrap(kek: &Kek, user: &uuid::Uuid, wrapped: &[u8]) -> Result<Self> {
        let mut bytes = wrap::unwrap(kek.expose(), wrapped, Purpose::VaultKey, user)?;
        let key = Self::from_bytes(bytes);
        bytes.zeroize();
        Ok(key)
    }
}

/// One way to open the vault, as the server stores it (`vault_unlockers`).
#[derive(Clone, Copy, Debug)]
pub enum Unlocker<'a> {
    SecurityKey {
        credential: CredentialRef<'a>,
        prf_salt: &'a [u8; 32],
        wrapped_vault_key: &'a [u8],
    },
    Recovery {
        key: &'a RecoveryKey,
        wrapped_vault_key: &'a [u8],
    },
}

/// What the server stores for a newly enabled security key.
#[derive(Debug)]
pub struct EnabledKey {
    pub prf_salt: [u8; 32],
    pub wrapped_vault_key: Vec<u8>,
}

/// Enables a security key as an unlocker: one touch. `vault_key` comes from
/// [`with_vault_key`] with another unlocker, or is new ([`VaultKey::generate`]) for the first.
pub fn enable_security_key(
    auth: &mut impl Authenticator,
    credential: CredentialRef<'_>,
    user: &uuid::Uuid,
    vault_key: &VaultKey,
) -> Result<EnabledKey> {
    let mut prf_salt = [0u8; 32];
    rand::fill(&mut prf_salt);
    let kek = Kek::from_prf(&auth.prf(credential, &prf_salt)?);
    Ok(EnabledKey {
        prf_salt,
        wrapped_vault_key: vault_key.wrap(&kek, user),
    })
}

/// Enables another security key, opening the vault with an existing unlocker: two touches, the
/// existing key's then the new one's.
pub fn enable_another_key(
    auth: &mut impl Authenticator,
    user: &uuid::Uuid,
    existing: Unlocker<'_>,
    new: CredentialRef<'_>,
) -> Result<EnabledKey> {
    let vault_key = open_vault(auth, user, existing)?;
    enable_security_key(auth, new, user, &vault_key)
}

/// Wraps the vault key for a recovery key.
pub fn wrap_for_recovery(
    recovery: &RecoveryKey,
    user: &uuid::Uuid,
    vault_key: &VaultKey,
) -> Vec<u8> {
    vault_key.wrap(&Kek::from_recovery(recovery), user)
}

/// Opens the vault with one unlocker (one touch for a security key) and runs `action` with the
/// vault key, which is dropped when it returns.
pub fn with_vault_key<T>(
    auth: &mut impl Authenticator,
    user: &uuid::Uuid,
    unlocker: Unlocker<'_>,
    action: impl FnOnce(&VaultKey) -> Result<T>,
) -> Result<T> {
    action(&open_vault(auth, user, unlocker)?)
}

fn open_vault(
    auth: &mut impl Authenticator,
    user: &uuid::Uuid,
    unlocker: Unlocker<'_>,
) -> Result<VaultKey> {
    let (kek, wrapped) = match unlocker {
        Unlocker::SecurityKey {
            credential,
            prf_salt,
            wrapped_vault_key,
        } => (
            Kek::from_prf(&auth.prf(credential, prf_salt)?),
            wrapped_vault_key,
        ),
        Unlocker::Recovery {
            key,
            wrapped_vault_key,
        } => (Kek::from_recovery(key), wrapped_vault_key),
    };
    VaultKey::unwrap(&kek, user, wrapped)
}

/// A protected folder as the server lists it (`GET /protected`).
#[derive(Clone, Copy, Debug)]
pub struct ProtectedFolder<'a> {
    pub node_id: &'a uuid::Uuid,
    pub wrapped_secret: &'a [u8],
    /// The age recipient the server lists; checked against the secret.
    pub recipient: &'a str,
}

/// One action in a security-key folder: one touch, then the folder keys for `action`, then
/// nothing kept. Fails if the server's recipient for the folder is not the secret's.
pub fn with_folder_keys<T>(
    auth: &mut impl Authenticator,
    user: &uuid::Uuid,
    unlocker: Unlocker<'_>,
    folder: ProtectedFolder<'_>,
    action: impl FnOnce(&FolderKeys) -> Result<T>,
) -> Result<T> {
    with_vault_key(auth, user, unlocker, |vault_key| {
        let keys = FolderSecret::unwrap(vault_key, folder.node_id, folder.wrapped_secret)?.keys();
        keys.check_recipient(folder.recipient)?;
        action(&keys)
    })
}

/// What `POST /protected` takes for a new protected folder.
#[derive(Debug)]
pub struct NewFolder {
    pub wrapped_secret: Vec<u8>,
    /// The folder's age recipient (`age1…`).
    pub recipient: String,
    /// Password folders only: the Argon2id parameters to store.
    pub kdf: Option<PasswordKdf>,
}

/// Protects `folder` with the vault: a new folder secret, wrapped by the vault key. `action`
/// gets the new keys in the same touch (to seal every name of an existing folder).
pub fn protect_with_vault_key<T>(
    vault_key: &VaultKey,
    folder: &uuid::Uuid,
    action: impl FnOnce(&FolderKeys) -> Result<T>,
) -> Result<(NewFolder, T)> {
    let secret = FolderSecret::generate();
    let keys = secret.keys();
    let new = NewFolder {
        wrapped_secret: secret.wrap(vault_key, folder),
        recipient: keys.recipient().to_string(),
        kdf: None,
    };
    drop(secret);
    let result = action(&keys)?;
    Ok((new, result))
}

/// Protects `folder` with a password (checked with [`PasswordKdf::check_new_password`]).
pub fn protect_with_password<T>(
    password: &str,
    folder: &uuid::Uuid,
    action: impl FnOnce(&FolderKeys) -> Result<T>,
) -> Result<(NewFolder, T)> {
    PasswordKdf::check_new_password(password)?;
    let kdf = PasswordKdf::generate();
    let kek = Kek::from_password(password, &kdf)?;
    let secret = FolderSecret::generate();
    let keys = secret.keys();
    let new = NewFolder {
        wrapped_secret: secret.wrap(&kek, folder),
        recipient: keys.recipient().to_string(),
        kdf: Some(kdf),
    };
    drop((kek, secret));
    let result = action(&keys)?;
    Ok((new, result))
}

/// Unlocks a password folder. The caller keeps the keys until Lock (decided for password
/// folders), then drops them.
pub fn unlock_password_folder(
    password: &str,
    kdf: &PasswordKdf,
    folder: ProtectedFolder<'_>,
) -> Result<FolderKeys> {
    let kek = Kek::from_password(password, kdf)?;
    let keys = FolderSecret::unwrap(&kek, folder.node_id, folder.wrapped_secret)?.keys();
    keys.check_recipient(folder.recipient)?;
    Ok(keys)
}

/// Re-wraps a folder secret from one wrapping key to another: vault key rotation, and "change
/// lock" between password and security key. No file is re-encrypted.
pub fn rewrap_folder_secret(
    from: &impl crate::folder::FolderWrappingKey,
    to: &impl crate::folder::FolderWrappingKey,
    folder: &uuid::Uuid,
    wrapped: &[u8],
) -> Result<Vec<u8>> {
    Ok(FolderSecret::unwrap(from, folder, wrapped)?.wrap(to, folder))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeKind, SoftAuthenticator, content};

    const USER: uuid::Uuid = uuid::Uuid::from_bytes([1; 16]);
    const FOLDER: uuid::Uuid = uuid::Uuid::from_bytes([2; 16]);
    const RP: &str = "files.example.org";

    #[test]
    fn whole_flow_with_two_keys_and_recovery() {
        let mut auth = SoftAuthenticator::new();
        let id_a = auth.register(RP);
        let id_b = auth.register(RP);
        let a = CredentialRef {
            credential_id: &id_a,
            rp_id: RP,
        };
        let b = CredentialRef {
            credential_id: &id_b,
            rp_id: RP,
        };

        // Enable key A with a new vault, protect a folder in the same unlock.
        let vault_key = VaultKey::generate();
        let enabled_a = enable_security_key(&mut auth, a, &USER, &vault_key).unwrap();
        let (folder, sealed) = protect_with_vault_key(&vault_key, &FOLDER, |keys| {
            keys.seal_name(&FOLDER, NodeKind::File, "passport.pdf")
        })
        .unwrap();
        let recovery = RecoveryKey::generate();
        let wrapped_recovery = wrap_for_recovery(&recovery, &USER, &vault_key);
        drop(vault_key);
        let unlocker_a = Unlocker::SecurityKey {
            credential: a,
            prf_salt: &enabled_a.prf_salt,
            wrapped_vault_key: &enabled_a.wrapped_vault_key,
        };

        // Enable key B through A: one touch each.
        auth.touches = 0;
        let enabled_b = enable_another_key(&mut auth, &USER, unlocker_a, b).unwrap();
        assert_eq!(auth.touches, 2);
        let unlocker_b = Unlocker::SecurityKey {
            credential: b,
            prf_salt: &enabled_b.prf_salt,
            wrapped_vault_key: &enabled_b.wrapped_vault_key,
        };

        // Upload needs no key for content; a touch for the name.
        let recipient: age::x25519::Recipient = folder.recipient.parse().unwrap();
        let blob = content::encrypt_bytes(&recipient, b"scan").unwrap();

        let listed = ProtectedFolder {
            node_id: &FOLDER,
            wrapped_secret: &folder.wrapped_secret,
            recipient: &folder.recipient,
        };
        for unlocker in [
            unlocker_a,
            unlocker_b,
            Unlocker::Recovery {
                key: &recovery,
                wrapped_vault_key: &wrapped_recovery,
            },
        ] {
            auth.touches = 0;
            let (name, plain) = with_folder_keys(&mut auth, &USER, unlocker, listed, |keys| {
                Ok((
                    keys.open_name(&FOLDER, NodeKind::File, &sealed)?,
                    content::decrypt_bytes(keys.identity(), &blob)?,
                ))
            })
            .unwrap();
            assert_eq!(name, "passport.pdf");
            assert_eq!(plain, b"scan");
            assert!(auth.touches <= 1);
        }

        // A server that swaps the recipient is caught.
        let other = FolderSecret::generate().recipient().to_string();
        let swapped = ProtectedFolder {
            recipient: &other,
            ..listed
        };
        assert!(with_folder_keys(&mut auth, &USER, unlocker_a, swapped, |_| Ok(())).is_err());
    }

    #[test]
    fn password_folder_and_change_lock() {
        let fast = |password: &str| -> Result<(NewFolder, ())> {
            // protect_with_password uses the real parameters; build the same with fast ones.
            PasswordKdf::check_new_password(password)?;
            let kdf = PasswordKdf {
                salt: [7; 16],
                memory_kib: 64,
                passes: 1,
                lanes: 1,
            };
            let kek = Kek::from_password(password, &kdf)?;
            let secret = FolderSecret::generate();
            Ok((
                NewFolder {
                    wrapped_secret: secret.wrap(&kek, &FOLDER),
                    recipient: secret.recipient().to_string(),
                    kdf: Some(kdf),
                },
                (),
            ))
        };
        assert!(fast("short").is_err());
        let (folder, ()) = fast("a long enough password").unwrap();
        let kdf = folder.kdf.clone().unwrap();
        let listed = ProtectedFolder {
            node_id: &FOLDER,
            wrapped_secret: &folder.wrapped_secret,
            recipient: &folder.recipient,
        };
        assert!(unlock_password_folder("a long enough password", &kdf, listed).is_ok());
        assert!(unlock_password_folder("a wrong password here", &kdf, listed).is_err());

        // Password → security key: re-wrap under the vault key, same recipient.
        let kek = Kek::from_password("a long enough password", &kdf).unwrap();
        let vault_key = VaultKey::generate();
        let rewrapped =
            rewrap_folder_secret(&kek, &vault_key, &FOLDER, &folder.wrapped_secret).unwrap();
        let secret = FolderSecret::unwrap(&vault_key, &FOLDER, &rewrapped).unwrap();
        assert_eq!(secret.recipient().to_string(), folder.recipient);
    }
}
