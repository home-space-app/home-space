//! Generating strong secrets.

use base64::{Engine, engine::general_purpose::STANDARD};

const JWT_SECRET_BYTES: usize = 48;
const DB_PASSWORD_LEN: usize = 32;
const ALPHANUMERIC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// A signing secret like `openssl rand -base64 48`.
pub fn jwt_secret() -> String {
    let mut bytes = [0u8; JWT_SECRET_BYTES];
    rand::fill(&mut bytes);
    STANDARD.encode(bytes)
}

/// The key that seals authenticator app secrets and password-folder keys: as strong as the
/// signing secret, and generated apart from it.
pub fn secrets_key() -> String {
    jwt_secret()
}

/// A database password of letters and digits, so that it needs no quoting anywhere.
pub fn db_password() -> String {
    // 62 letters and digits: bytes from 248 up are dropped so that every character is as likely.
    let limit = u8::try_from(256 - 256 % ALPHANUMERIC.len()).unwrap_or(u8::MAX);
    let mut password = String::with_capacity(DB_PASSWORD_LEN);
    while password.len() < DB_PASSWORD_LEN {
        let mut bytes = [0u8; DB_PASSWORD_LEN];
        rand::fill(&mut bytes);
        password.extend(
            bytes
                .iter()
                .filter(|&&byte| byte < limit)
                .map(|&byte| char::from(ALPHANUMERIC[usize::from(byte) % ALPHANUMERIC.len()]))
                .take(DB_PASSWORD_LEN - password.len()),
        );
    }
    password
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_jwt_secret_passes_the_servers_check() {
        let secret = jwt_secret();
        assert_eq!(secret.len(), 64);
        auth::secrets::check_secret("secret", &secret).expect("accepted");
        assert_ne!(secret, jwt_secret());
    }

    #[test]
    fn a_generated_db_password_is_alphanumeric() {
        let password = db_password();
        assert_eq!(password.len(), DB_PASSWORD_LEN);
        assert!(password.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        assert_ne!(password, db_password());
    }
}
