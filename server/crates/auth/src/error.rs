/// Errors returned by the authentication layer.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Unknown user, wrong password or invalid/expired/revoked token.
    #[error("invalid credentials")]
    InvalidCredentials,
    /// Too many wrong passwords for this username: no password is checked until the wait,
    /// in whole seconds, is over.
    #[error("too many failed sign-in attempts, try again in {}", describe_wait(*retry_after))]
    TooManyAttempts { retry_after: u64 },
    /// The username is already taken.
    #[error("username already exists")]
    UsernameTaken,
    /// The username or password does not meet the requirements.
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    /// Single sign-on is not configured on this server.
    #[error("single sign-on is not enabled")]
    OidcDisabled,
    /// The identity provider authenticated the user but access is restricted to a group.
    #[error("access denied")]
    OidcDenied,
    /// The identity provider could not be reached or sent something invalid.
    #[error("identity provider error: {0}")]
    Oidc(String),
    #[error("password hashing failed: {0}")]
    Hashing(String),
    #[error("token encoding failed: {0}")]
    Token(#[from] jsonwebtoken::errors::Error),
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Db(#[from] db::DbError),
}

/// `seconds` in the largest unit that keeps it readable, rounded up.
fn describe_wait(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..60 => (seconds.max(1), "second"),
        60..3600 => (seconds.div_ceil(60), "minute"),
        _ => (seconds.div_ceil(3600), "hour"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_read_in_a_fitting_unit() {
        assert_eq!(describe_wait(1), "1 second");
        assert_eq!(describe_wait(30), "30 seconds");
        assert_eq!(describe_wait(60), "1 minute");
        assert_eq!(describe_wait(61), "2 minutes");
        assert_eq!(describe_wait(3600), "1 hour");
        assert_eq!(describe_wait(5000), "2 hours");
    }
}
