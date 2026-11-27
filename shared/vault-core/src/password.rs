//! Password folders: end to end like security-key folders, with the folder secret wrapped by a
//! key derived from the folder's password (Argon2id) and, for a forgotten password, by the
//! folder's own recovery key. The server keeps both wrapped copies and the Argon2id parameters,
//! never the password.
//!
//! Anyone with what the server keeps can try passwords offline, so the parameters are costly
//! (64 MiB, three passes) and clients ask for a password the account's rules accept.

use argon2::{Algorithm, Argon2, Params, Version};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::folder::{FolderKeys, FolderSecret};
use crate::kdf::{Kek, hkdf32};
use crate::recovery::RecoveryKey;
use crate::{Error, Internal, Result};

const PASSWORD_KEK_INFO: &[u8] = b"home-space password kek v1";
const FOLDER_RECOVERY_KEK_INFO: &[u8] = b"home-space folder recovery kek v1";

/// Length of the random Argon2id salt of a password folder.
pub const SALT_LEN: usize = 16;
/// Argon2id memory of a new password folder, in KiB: 64 MiB.
pub const DEFAULT_MEMORY_KIB: u32 = 64 * 1024;
/// Argon2id passes of a new password folder.
pub const DEFAULT_ITERATIONS: u32 = 3;
/// Argon2id lanes of a new password folder: one, since the browser derives on a single thread.
pub const DEFAULT_PARALLELISM: u32 = 1;
/// The least memory a client accepts, in KiB: OWASP's minimum for Argon2id, 19 MiB.
pub const MIN_MEMORY_KIB: u32 = 19 * 1024;
/// The most memory a client accepts, in KiB, so a server cannot make a phone derive for minutes.
pub const MAX_MEMORY_KIB: u32 = 256 * 1024;
/// The fewest passes a client accepts.
pub const MIN_ITERATIONS: u32 = 2;
/// The most passes a client accepts.
pub const MAX_ITERATIONS: u32 = 10;
/// The most lanes a client accepts.
pub const MAX_PARALLELISM: u32 = 4;

/// The Argon2id parameters of a password folder, stored on the server with its wrapped secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordKdf {
    /// Random per folder, and new with every password.
    pub salt: [u8; SALT_LEN],
    /// Memory in KiB.
    pub memory_kib: u32,
    /// Passes over the memory.
    pub iterations: u32,
    /// Lanes.
    pub parallelism: u32,
}

impl PasswordKdf {
    /// The parameters for a new password: a new salt and the default costs.
    pub fn generate() -> Self {
        let mut salt = [0u8; SALT_LEN];
        rand::fill(&mut salt);
        Self {
            salt,
            memory_kib: DEFAULT_MEMORY_KIB,
            iterations: DEFAULT_ITERATIONS,
            parallelism: DEFAULT_PARALLELISM,
        }
    }

    /// Refuses parameters outside the bounds above: too weak to have been made by a client, or
    /// so costly that deriving would hang the app.
    pub fn check(&self) -> Result<()> {
        let within = (MIN_MEMORY_KIB..=MAX_MEMORY_KIB).contains(&self.memory_kib)
            && (MIN_ITERATIONS..=MAX_ITERATIONS).contains(&self.iterations)
            && (1..=MAX_PARALLELISM).contains(&self.parallelism);
        if within {
            Ok(())
        } else {
            Err(Error::PasswordParams)
        }
    }

    fn argon2(&self) -> Result<Argon2<'static>> {
        self.check()?;
        let params = Params::new(self.memory_kib, self.iterations, self.parallelism, Some(32))
            .map_err(|_| Error::PasswordParams)?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }
}

impl Kek {
    /// The KEK of a folder password: `HKDF-SHA256(Argon2id(NFC(password), kdf), "home-space
    /// password kek v1")`. Takes as long as the parameters make it, about a second.
    pub fn from_password(password: &str, kdf: &PasswordKdf) -> Result<Self> {
        let normalized = Zeroizing::new(password.nfc().collect::<String>());
        let mut stretched = Zeroizing::new([0u8; 32]);
        kdf.argon2()?
            .hash_password_into(normalized.as_bytes(), &kdf.salt, stretched.as_mut())
            .map_err(|_| Error::Internal(Internal::KeyDerivation))?;
        hkdf32(stretched.as_ref(), PASSWORD_KEK_INFO).map(Self::from_bytes)
    }

    /// The KEK of a password folder's own recovery key, distinct from the vault's.
    pub fn from_folder_recovery(recovery: &RecoveryKey) -> Result<Self> {
        hkdf32(recovery.expose(), FOLDER_RECOVERY_KEK_INFO).map(Self::from_bytes)
    }
}

/// What the server stores for a password folder, with its recipient.
#[derive(Debug)]
pub struct PasswordWrap {
    /// The Argon2id parameters of the password.
    pub kdf: PasswordKdf,
    /// The folder secret, wrapped by the password's KEK.
    pub wrapped_secret: Vec<u8>,
    /// The folder secret, wrapped by the folder's recovery key.
    pub recovery_wrapped_secret: Vec<u8>,
}

/// A new password folder: what the server stores, and what the user sees once.
#[derive(Debug)]
pub struct NewPasswordFolder {
    /// The wrapped secret and its parameters.
    pub wrap: PasswordWrap,
    /// The folder's age recipient (`age1…`).
    pub recipient: String,
    /// The folder's recovery key, to show once and never store.
    pub recovery_key: RecoveryKey,
    /// The new folder's keys, to seal the names of what it already holds.
    pub keys: FolderKeys,
}

/// How the user opens a password folder.
#[derive(Clone, Copy, Debug)]
pub enum PasswordUnlock<'a> {
    /// The folder's password.
    Password {
        /// What the user typed.
        password: &'a str,
        /// The folder's parameters, as the server lists them.
        kdf: &'a PasswordKdf,
        /// The secret wrapped by the password.
        wrapped_secret: &'a [u8],
    },
    /// The folder's recovery key, when the password is forgotten.
    Recovery {
        /// The parsed recovery key.
        key: &'a RecoveryKey,
        /// The secret wrapped by the recovery key.
        recovery_wrapped_secret: &'a [u8],
    },
}

/// Protects `folder` with `password`: a new folder secret, wrapped by the password and by a new
/// recovery key.
pub fn protect_with_password(password: &str, folder: &uuid::Uuid) -> Result<NewPasswordFolder> {
    protect(password, PasswordKdf::generate(), folder)
}

fn protect(password: &str, kdf: PasswordKdf, folder: &uuid::Uuid) -> Result<NewPasswordFolder> {
    let secret = FolderSecret::generate();
    let recovery_key = RecoveryKey::generate();
    let keys = secret.keys()?;
    Ok(NewPasswordFolder {
        wrap: PasswordWrap {
            kdf,
            wrapped_secret: secret.wrap(&Kek::from_password(password, &kdf)?, folder)?,
            recovery_wrapped_secret: secret
                .wrap(&Kek::from_folder_recovery(&recovery_key)?, folder)?,
        },
        recipient: keys.recipient().to_string(),
        recovery_key,
        keys,
    })
}

/// Opens the password folder `folder`, checking the recipient the server lists against its
/// secret. A wrong password or recovery key is [`Error::Unwrap`].
pub fn unlock_with_password(
    unlock: PasswordUnlock<'_>,
    folder: &uuid::Uuid,
    recipient: &str,
) -> Result<FolderKeys> {
    let keys = open_secret(unlock, folder)?.keys()?;
    keys.check_recipient(recipient)?;
    Ok(keys)
}

/// A new password for a password folder, once `unlock` opened it.
#[derive(Debug)]
pub struct PasswordChange {
    /// The Argon2id parameters of the new password.
    pub kdf: PasswordKdf,
    /// The folder secret, wrapped by the new password's KEK.
    pub wrapped_secret: Vec<u8>,
    /// A new recovery key and the secret wrapped by it, when `unlock` used the old one: it was
    /// typed in, so it is replaced.
    pub recovery: Option<NewRecoveryKey>,
}

/// A password folder's new recovery key.
#[derive(Debug)]
pub struct NewRecoveryKey {
    /// The key, to show once and never store.
    pub key: RecoveryKey,
    /// The folder secret, wrapped by it.
    pub recovery_wrapped_secret: Vec<u8>,
}

/// Wraps the folder's secret for `new_password`. The secret stays, so no file is encrypted
/// again; the recovery key stays too unless it was what opened the folder.
pub fn change_password(
    unlock: PasswordUnlock<'_>,
    new_password: &str,
    folder: &uuid::Uuid,
    recipient: &str,
) -> Result<PasswordChange> {
    rewrap(
        unlock,
        new_password,
        PasswordKdf::generate(),
        folder,
        recipient,
    )
}

fn rewrap(
    unlock: PasswordUnlock<'_>,
    new_password: &str,
    kdf: PasswordKdf,
    folder: &uuid::Uuid,
    recipient: &str,
) -> Result<PasswordChange> {
    let secret = open_secret(unlock, folder)?;
    secret.keys()?.check_recipient(recipient)?;
    let wrapped_secret = secret.wrap(&Kek::from_password(new_password, &kdf)?, folder)?;
    let recovery = match unlock {
        PasswordUnlock::Password { .. } => None,
        PasswordUnlock::Recovery { .. } => {
            let key = RecoveryKey::generate();
            let recovery_wrapped_secret = secret.wrap(&Kek::from_folder_recovery(&key)?, folder)?;
            Some(NewRecoveryKey {
                key,
                recovery_wrapped_secret,
            })
        }
    };
    Ok(PasswordChange {
        kdf,
        wrapped_secret,
        recovery,
    })
}

fn open_secret(unlock: PasswordUnlock<'_>, folder: &uuid::Uuid) -> Result<FolderSecret> {
    match unlock {
        PasswordUnlock::Password {
            password,
            kdf,
            wrapped_secret,
        } => FolderSecret::unwrap(&Kek::from_password(password, kdf)?, folder, wrapped_secret),
        PasswordUnlock::Recovery {
            key,
            recovery_wrapped_secret,
        } => FolderSecret::unwrap(
            &Kek::from_folder_recovery(key)?,
            folder,
            recovery_wrapped_secret,
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::NodeKind;

    const FOLDER: uuid::Uuid = uuid::Uuid::from_bytes([6; 16]);

    /// The cheapest parameters a client accepts, so tests stay quick.
    pub(crate) fn cheap(salt: [u8; SALT_LEN]) -> PasswordKdf {
        PasswordKdf {
            salt,
            memory_kib: MIN_MEMORY_KIB,
            iterations: MIN_ITERATIONS,
            parallelism: 1,
        }
    }

    fn protect_cheaply(password: &str) -> NewPasswordFolder {
        protect(password, cheap([1; SALT_LEN]), &FOLDER).unwrap()
    }

    fn by_password<'a>(folder: &'a NewPasswordFolder, password: &'a str) -> PasswordUnlock<'a> {
        PasswordUnlock::Password {
            password,
            kdf: &folder.wrap.kdf,
            wrapped_secret: &folder.wrap.wrapped_secret,
        }
    }

    #[test]
    fn the_password_opens_the_folder_and_its_names() {
        let folder = protect_cheaply("correct horse");
        let sealed = folder
            .keys
            .seal_name(&FOLDER, NodeKind::File, "tax.pdf")
            .unwrap();
        let keys = unlock_with_password(
            by_password(&folder, "correct horse"),
            &FOLDER,
            &folder.recipient,
        )
        .unwrap();
        assert_eq!(
            keys.open_name(&FOLDER, NodeKind::File, &sealed).unwrap(),
            "tax.pdf"
        );
    }

    #[test]
    fn a_wrong_password_does_not_unwrap() {
        let folder = protect_cheaply("correct horse");
        let unlocked = unlock_with_password(
            by_password(&folder, "wrong horse"),
            &FOLDER,
            &folder.recipient,
        );
        assert!(matches!(unlocked, Err(Error::Unwrap)));
    }

    #[test]
    fn the_password_is_normalized() {
        let folder = protect_cheaply("caf\u{e9}");
        let decomposed = by_password(&folder, "cafe\u{301}");
        assert!(unlock_with_password(decomposed, &FOLDER, &folder.recipient).is_ok());
    }

    #[test]
    fn the_recovery_key_opens_the_folder() {
        let folder = protect_cheaply("correct horse");
        let unlock = PasswordUnlock::Recovery {
            key: &folder.recovery_key,
            recovery_wrapped_secret: &folder.wrap.recovery_wrapped_secret,
        };
        assert!(unlock_with_password(unlock, &FOLDER, &folder.recipient).is_ok());
    }

    #[test]
    fn a_swapped_recipient_is_caught() {
        let folder = protect_cheaply("correct horse");
        let other = FolderSecret::generate().recipient().unwrap().to_string();
        let unlocked = unlock_with_password(by_password(&folder, "correct horse"), &FOLDER, &other);
        assert!(matches!(unlocked, Err(Error::RecipientMismatch)));
    }

    #[test]
    fn changing_the_password_keeps_the_recovery_key() {
        let folder = protect_cheaply("correct horse");
        let change = rewrap(
            by_password(&folder, "correct horse"),
            "battery staple",
            cheap([2; SALT_LEN]),
            &FOLDER,
            &folder.recipient,
        )
        .unwrap();
        assert!(change.recovery.is_none());
        let unlock = PasswordUnlock::Password {
            password: "battery staple",
            kdf: &change.kdf,
            wrapped_secret: &change.wrapped_secret,
        };
        assert!(unlock_with_password(unlock, &FOLDER, &folder.recipient).is_ok());
        assert_ne!(change.kdf.salt, folder.wrap.kdf.salt);
    }

    #[test]
    fn resetting_with_the_recovery_key_replaces_it() {
        let folder = protect_cheaply("correct horse");
        let unlock = PasswordUnlock::Recovery {
            key: &folder.recovery_key,
            recovery_wrapped_secret: &folder.wrap.recovery_wrapped_secret,
        };
        let change = rewrap(
            unlock,
            "battery staple",
            cheap([2; SALT_LEN]),
            &FOLDER,
            &folder.recipient,
        )
        .unwrap();
        let recovery = change.recovery.unwrap();
        let unlock = PasswordUnlock::Recovery {
            key: &recovery.key,
            recovery_wrapped_secret: &recovery.recovery_wrapped_secret,
        };
        assert!(unlock_with_password(unlock, &FOLDER, &folder.recipient).is_ok());
    }

    #[test]
    fn parameters_out_of_bounds_are_refused() {
        let weak = PasswordKdf {
            memory_kib: 1024,
            ..cheap([0; SALT_LEN])
        };
        let costly = PasswordKdf {
            iterations: 100,
            ..cheap([0; SALT_LEN])
        };
        assert!(matches!(
            Kek::from_password("x", &weak),
            Err(Error::PasswordParams)
        ));
        assert!(matches!(costly.check(), Err(Error::PasswordParams)));
        assert!(PasswordKdf::generate().check().is_ok());
    }
}
