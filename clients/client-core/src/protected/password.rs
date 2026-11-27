//! The password lock: what to send to protect a folder, unlock it and change its password.

use protocol::{
    ChangeFolderPassword, FolderLock, NodeId, ProtectFolder, ProtectedFolder, encoding::Base64Url,
};
use vault_core::{FolderKeys, PasswordKdf, PasswordUnlock, RecoveryKey, password};

use super::ProtectedError;

/// How the user proves they may open a password folder.
#[derive(Clone, Copy, Debug)]
pub enum PasswordProof<'a> {
    /// The folder's password.
    Password(&'a str),
    /// The folder's recovery key, as typed.
    RecoveryKey(&'a str),
}

/// A folder about to be protected with a password.
#[derive(Debug)]
pub struct NewPasswordFolder {
    /// What `POST /protected` takes.
    pub request: ProtectFolder,
    /// The folder's recovery key, to show once.
    pub recovery_key: RecoveryKey,
    /// Its keys, to seal what it already holds and keep it unlocked.
    pub keys: FolderKeys,
}

/// A password folder's new password.
#[derive(Debug)]
pub struct NewFolderPassword {
    /// What `POST /protected/{id}/password` takes.
    pub request: ChangeFolderPassword,
    /// The new recovery key, to show once, when the old one opened the folder.
    pub recovery_key: Option<RecoveryKey>,
}

/// Protects `folder` with `password`. Derives the key from it, which takes about a second.
pub fn protect_with_password(
    folder: NodeId,
    password: &str,
) -> Result<NewPasswordFolder, ProtectedError> {
    let new = password::protect_with_password(password, &folder.0)?;
    Ok(NewPasswordFolder {
        request: ProtectFolder {
            node_id: folder,
            lock: FolderLock::Password,
            wrapped_secret: Base64Url::from(new.wrap.wrapped_secret),
            recipient: new.recipient,
            epoch: None,
            kdf: Some(to_protocol(&new.wrap.kdf)),
            recovery_wrapped_secret: Some(Base64Url::from(new.wrap.recovery_wrapped_secret)),
        },
        recovery_key: new.recovery_key,
        keys: new.keys,
    })
}

/// Opens the password folder `folder`, as `GET /protected` lists it.
pub fn unlock_password_folder(
    folder: &ProtectedFolder,
    proof: PasswordProof<'_>,
) -> Result<FolderKeys, ProtectedError> {
    with_unlock(folder, proof, |unlock| {
        password::unlock_with_password(unlock, &folder.node_id.0, &folder.recipient)
    })
}

/// Wraps the secret of the password folder `folder` for `new_password`.
pub fn change_folder_password(
    folder: &ProtectedFolder,
    proof: PasswordProof<'_>,
    new_password: &str,
) -> Result<NewFolderPassword, ProtectedError> {
    let change = with_unlock(folder, proof, |unlock| {
        password::change_password(unlock, new_password, &folder.node_id.0, &folder.recipient)
    })?;
    let (recovery_key, recovery_wrapped_secret) = match change.recovery {
        Some(recovery) => (
            Some(recovery.key),
            Some(Base64Url::from(recovery.recovery_wrapped_secret)),
        ),
        None => (None, None),
    };
    Ok(NewFolderPassword {
        request: ChangeFolderPassword {
            wrapped_secret: Base64Url::from(change.wrapped_secret),
            kdf: to_protocol(&change.kdf),
            recovery_wrapped_secret,
        },
        recovery_key,
    })
}

fn with_unlock<T>(
    folder: &ProtectedFolder,
    proof: PasswordProof<'_>,
    action: impl FnOnce(PasswordUnlock<'_>) -> vault_core::Result<T>,
) -> Result<T, ProtectedError> {
    if folder.lock != FolderLock::Password {
        return Err(ProtectedError::NotPassword);
    }
    match proof {
        PasswordProof::Password(password) => {
            let listed = folder.kdf.as_ref().ok_or(ProtectedError::NotPassword)?;
            let kdf = from_protocol(listed)?;
            action(PasswordUnlock::Password {
                password,
                kdf: &kdf,
                wrapped_secret: folder.wrapped_secret.as_bytes(),
            })
            .map_err(|error| wrong(error, ProtectedError::WrongPassword))
        }
        PasswordProof::RecoveryKey(text) => {
            let recovery_wrapped_secret = folder
                .recovery_wrapped_secret
                .as_ref()
                .ok_or(ProtectedError::NotPassword)?;
            let key = RecoveryKey::parse(text).map_err(|_| ProtectedError::WrongRecoveryKey)?;
            action(PasswordUnlock::Recovery {
                key: &key,
                recovery_wrapped_secret: recovery_wrapped_secret.as_bytes(),
            })
            .map_err(|error| wrong(error, ProtectedError::WrongRecoveryKey))
        }
    }
}

fn wrong(error: vault_core::Error, wrong: ProtectedError) -> ProtectedError {
    match error {
        vault_core::Error::Unwrap => wrong,
        other => other.into(),
    }
}

fn from_protocol(kdf: &protocol::PasswordKdf) -> Result<PasswordKdf, ProtectedError> {
    let salt = kdf
        .salt
        .as_bytes()
        .try_into()
        .map_err(|_| ProtectedError::UnacceptedParameters)?;
    let kdf = PasswordKdf {
        salt,
        memory_kib: kdf.memory_kib,
        iterations: kdf.iterations,
        parallelism: kdf.parallelism,
    };
    kdf.check()?;
    Ok(kdf)
}

fn to_protocol(kdf: &PasswordKdf) -> protocol::PasswordKdf {
    protocol::PasswordKdf {
        salt: Base64Url::from(kdf.salt.to_vec()),
        memory_kib: kdf.memory_kib,
        iterations: kdf.iterations,
        parallelism: kdf.parallelism,
    }
}
