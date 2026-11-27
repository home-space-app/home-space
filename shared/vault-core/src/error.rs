use std::fmt;

use crate::names::MAX_NAME_CHARS;

/// Everything that can go wrong in this crate. Messages are for logs; clients map the variants
/// to their own texts.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A wrapped key did not open: the wrong security key, password or recovery key, or the
    /// stored value was changed.
    #[error("the key does not match or the wrapped value was changed")]
    Unwrap,
    /// A wrapped key has a format this version does not know.
    #[error("unknown wrapped key format")]
    WrapFormat,
    /// A name cannot be sealed: empty, too long, or not valid once opened.
    #[error("invalid name: {0}")]
    Name(NameProblem),
    /// A sealed name did not open: wrong folder, wrong parent, or changed.
    #[error("the sealed name does not open")]
    SealedName,
    /// A recovery key was mistyped (bad characters, length or checksum).
    #[error("this is not a valid recovery key")]
    RecoveryKey,
    /// The security key did not answer, or answered without a PRF secret. The source is the
    /// platform's own error.
    #[error("security key: {0}")]
    Authenticator(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// A password folder's Argon2id parameters are outside what clients accept.
    #[error("the password parameters are out of bounds")]
    PasswordParams,
    /// The folder's public key on the server is not the one its secret derives.
    #[error("the folder's public key does not match its secret")]
    RecipientMismatch,
    /// Encrypting or decrypting content failed.
    #[error("content: {0}")]
    Content(#[from] ContentError),
    /// A primitive refused an input this crate sizes itself: a bug, never the user's doing.
    #[error("internal: {0}")]
    Internal(Internal),
    /// Reading or writing a stream of content failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Why a name cannot be sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameProblem {
    /// Nothing left once normalized.
    Empty,
    /// More than [`MAX_NAME_CHARS`] characters after NFC.
    TooLong,
    /// AES-SIV refused it.
    Unsealable,
}

impl fmt::Display for NameProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty"),
            Self::TooLong => write!(f, "longer than {MAX_NAME_CHARS} characters"),
            Self::Unsealable => f.write_str("cannot be sealed"),
        }
    }
}

/// Why age refused to encrypt or decrypt.
#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    /// The recipient text is not an age X25519 recipient; age describes why.
    #[error("{0}")]
    Recipient(&'static str),
    /// age could not start encrypting.
    #[error(transparent)]
    Encrypt(#[from] age::EncryptError),
    /// The file is not age, or does not open with this identity.
    #[error(transparent)]
    Decrypt(#[from] age::DecryptError),
}

/// Which primitive refused input that is always valid by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Internal {
    /// HKDF-SHA256 refused the output length.
    KeyDerivation,
    /// XChaCha20-Poly1305 refused to wrap a key.
    KeyWrap,
    /// A derived X25519 secret did not parse as an age identity.
    AgeIdentity,
}

impl fmt::Display for Internal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::KeyDerivation => "HKDF refused the output length",
            Self::KeyWrap => "a key could not be wrapped",
            Self::AgeIdentity => "a derived key is not an age identity",
        })
    }
}
