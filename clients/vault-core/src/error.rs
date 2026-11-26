/// Everything that can go wrong in this crate. Messages are for logs; clients map the variants
/// to their own texts.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A wrapped key did not open: the wrong security key, recovery key or password, or the
    /// stored value was changed.
    #[error("the key does not match or the wrapped value was changed")]
    Unwrap,
    /// A wrapped key has a format this version does not know.
    #[error("unknown wrapped key format")]
    WrapFormat,
    /// A name cannot be sealed: empty, too long, or not valid once opened.
    #[error("invalid name: {0}")]
    Name(&'static str),
    /// A sealed name did not open: wrong folder, wrong parent, or changed.
    #[error("the sealed name does not open")]
    SealedName,
    /// A folder password is too weak to be accepted.
    #[error("the password must have at least {min} characters")]
    PasswordTooShort { min: usize },
    /// Argon2id refused its parameters.
    #[error("invalid password parameters: {0}")]
    Kdf(String),
    /// A recovery key was mistyped (bad characters, length or checksum).
    #[error("this is not a valid recovery key")]
    RecoveryKey,
    /// The security key did not answer, or answered without a PRF secret.
    #[error("security key: {0}")]
    Authenticator(String),
    /// The folder's public key on the server is not the one its secret derives.
    #[error("the folder's public key does not match its secret")]
    RecipientMismatch,
    /// Encrypting or decrypting content failed.
    #[error("content: {0}")]
    Content(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
