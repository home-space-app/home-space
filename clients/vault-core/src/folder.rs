//! A protected folder's secret and the keys derived from it.

use age::x25519::{Identity, Recipient};
use zeroize::{Zeroize, Zeroizing};

use crate::kdf::{Kek, hkdf32};
use crate::names::{self, NodeKind};
use crate::secret::secret32;
use crate::vault::VaultKey;
use crate::wrap::{self, Purpose};
use crate::{Error, Result};

secret32! {
    /// The random secret of one protected folder. Wrapped by the vault key (security-key
    /// folders) or by a password KEK (password folders), bound to the folder's node id.
    pub struct FolderSecret;
}

/// What can wrap a folder secret: the vault key or a password folder's KEK.
pub trait FolderWrappingKey {
    #[doc(hidden)]
    fn wrapping_bytes(&self) -> &[u8; 32];
}

impl FolderWrappingKey for VaultKey {
    fn wrapping_bytes(&self) -> &[u8; 32] {
        self.expose()
    }
}

impl FolderWrappingKey for Kek {
    fn wrapping_bytes(&self) -> &[u8; 32] {
        self.expose()
    }
}

impl FolderSecret {
    pub fn generate() -> Self {
        Self::random()
    }

    /// Wraps the secret for `folder` (the protected folder's node id).
    pub fn wrap(&self, key: &impl FolderWrappingKey, folder: &uuid::Uuid) -> Vec<u8> {
        wrap::wrap(
            key.wrapping_bytes(),
            self.expose(),
            Purpose::FolderSecret,
            folder,
        )
    }

    pub fn unwrap(
        key: &impl FolderWrappingKey,
        folder: &uuid::Uuid,
        wrapped: &[u8],
    ) -> Result<Self> {
        let mut bytes = wrap::unwrap(key.wrapping_bytes(), wrapped, Purpose::FolderSecret, folder)?;
        let secret = Self::from_bytes(bytes);
        bytes.zeroize();
        Ok(secret)
    }

    /// Derives the name key and the age identity.
    pub fn keys(&self) -> FolderKeys {
        let mut name_key = Zeroizing::new([0u8; 64]);
        hkdf::Hkdf::<sha2::Sha256>::new(None, self.expose())
            .expand(b"home-space name key v1", name_key.as_mut())
            .expect("64 bytes is a valid HKDF-SHA256 length");
        let mut identity_bytes = hkdf32(self.expose(), b"home-space age identity v1");
        let identity = identity_from_bytes(&identity_bytes);
        identity_bytes.zeroize();
        FolderKeys { name_key, identity }
    }

    /// The folder's age recipient, the public key stored on the server.
    pub fn recipient(&self) -> Recipient {
        self.keys().recipient()
    }
}

/// age has no constructor from raw bytes, only its Bech32 text form.
fn identity_from_bytes(bytes: &[u8; 32]) -> Identity {
    let hrp = bech32::Hrp::parse_unchecked("AGE-SECRET-KEY-");
    let encoded = Zeroizing::new(age_core::primitives::bech32_encode(hrp, bytes).to_uppercase());
    encoded
        .parse()
        .expect("32 bytes Bech32-encoded are an age identity")
}

/// The keys of an unlocked folder. Hold it only for the action at hand: it can open every name
/// and every file in the folder.
pub struct FolderKeys {
    name_key: Zeroizing<[u8; 64]>,
    identity: Identity,
}

impl std::fmt::Debug for FolderKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FolderKeys(…)")
    }
}

impl FolderKeys {
    /// Seals a name for a node of `kind` in the folder `parent` (its direct parent, which may be
    /// a subfolder of the protected folder).
    pub fn seal_name(&self, parent: &uuid::Uuid, kind: NodeKind, name: &str) -> Result<Vec<u8>> {
        names::seal(&self.name_key, parent, kind, name)
    }

    pub fn open_name(&self, parent: &uuid::Uuid, kind: NodeKind, sealed: &[u8]) -> Result<String> {
        names::open(&self.name_key, parent, kind, sealed)
    }

    /// The age identity, to decrypt files. Prefer [`crate::content::decrypt`], which uses it only
    /// to unwrap the file key.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn recipient(&self) -> Recipient {
        self.identity.to_public()
    }

    /// Checks the recipient the server lists for the folder against the one the secret derives.
    /// Clients encrypt to the recipient without a key, so a server that swapped it could read
    /// new uploads; clients call this whenever they have the keys, and keep the checked value.
    pub fn check_recipient(&self, listed: &str) -> Result<Recipient> {
        let ours = self.recipient();
        if ours.to_string() == listed.trim() {
            Ok(ours)
        } else {
            Err(Error::RecipientMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLDER: uuid::Uuid = uuid::Uuid::from_bytes([3; 16]);

    #[test]
    fn wrap_round_trip_bound_to_folder() {
        let vault = VaultKey::generate();
        let secret = FolderSecret::generate();
        let wrapped = secret.wrap(&vault, &FOLDER);
        let back = FolderSecret::unwrap(&vault, &FOLDER, &wrapped).unwrap();
        assert_eq!(back.expose(), secret.expose());
        assert!(FolderSecret::unwrap(&vault, &uuid::Uuid::nil(), &wrapped).is_err());
        assert!(FolderSecret::unwrap(&VaultKey::generate(), &FOLDER, &wrapped).is_err());
    }

    #[test]
    fn keys_are_deterministic() {
        let a = FolderSecret::from_bytes([4; 32]);
        let b = FolderSecret::from_bytes([4; 32]);
        assert_eq!(a.recipient().to_string(), b.recipient().to_string());
        assert!(a.recipient().to_string().starts_with("age1"));
        assert_ne!(
            a.recipient().to_string(),
            FolderSecret::from_bytes([5; 32]).recipient().to_string()
        );
        let sealed = a.keys().seal_name(&FOLDER, NodeKind::File, "x").unwrap();
        assert_eq!(
            b.keys()
                .open_name(&FOLDER, NodeKind::File, &sealed)
                .unwrap(),
            "x"
        );
    }

    #[test]
    fn recipient_check() {
        let keys = FolderSecret::generate().keys();
        let listed = keys.recipient().to_string();
        assert!(keys.check_recipient(&listed).is_ok());
        let other = FolderSecret::generate().recipient().to_string();
        assert!(matches!(
            keys.check_recipient(&other),
            Err(Error::RecipientMismatch)
        ));
    }
}
