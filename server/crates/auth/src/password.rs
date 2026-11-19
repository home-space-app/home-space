use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::phc::PasswordHash};

use crate::AuthError;

pub use protocol::rules::MAX_PASSWORD_LEN;

/// Checks a password someone wants to set against [`protocol::rules::password_problem`].
pub fn check_new(password: &str) -> Result<(), AuthError> {
    match protocol::rules::password_problem(password) {
        Some(problem) => Err(AuthError::InvalidInput(problem)),
        None => Ok(()),
    }
}

/// Hashes a password with argon2id and a random salt, returning the PHC string.
pub fn hash(password: &str) -> Result<String, AuthError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|err| AuthError::Hashing(err.to_string()))
}

/// Checks a password against a PHC hash string; malformed hashes never verify.
pub fn verify(password: &str, phc_hash: &str) -> bool {
    PasswordHash::new(phc_hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_correct_password() {
        let hashed = hash("hunter2").expect("hash");
        assert!(verify("hunter2", &hashed));
    }

    #[test]
    fn rejects_wrong_password() {
        let hashed = hash("hunter2").expect("hash");
        assert!(!verify("hunter3", &hashed));
    }

    #[test]
    fn a_new_password_must_be_neither_short_nor_long() {
        check_new("correct horse battery").expect("accepted");
        check_new(&"a1".repeat(MAX_PASSWORD_LEN / 2)).expect("longest");
        for refused in ["short1", &"a".repeat(MAX_PASSWORD_LEN + 1)] {
            assert!(
                matches!(check_new(refused), Err(AuthError::InvalidInput(_))),
                "{}",
                &refused[..refused.len().min(20)]
            );
        }
    }

    #[test]
    fn rejects_malformed_hash() {
        assert!(!verify("hunter2", "not-a-hash"));
    }
}
