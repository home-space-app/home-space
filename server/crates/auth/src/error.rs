use protocol::{AsProblem, Problem, ProblemType};

/// Errors returned by the authentication layer.
///
/// What a user can run into is said as a sentence, since clients show the problem's `detail`
/// as it is.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Invalid, expired or revoked token, or a wrong share link password.
    #[error("invalid credentials")]
    InvalidCredentials,
    /// Unknown user or wrong password at sign-in. A `401` like
    /// [`InvalidCredentials`](Self::InvalidCredentials), with a detail the sign-in shows.
    #[error("Invalid username or password.")]
    WrongLogin,
    /// The one-time code an app or the web client trades for a session is unknown, used,
    /// expired, or does not match its verifier.
    #[error("This sign-in has expired or was already used. Please sign in again.")]
    LoginCodeInvalid,
    /// Too many wrong passwords for this username: no password is checked until the wait,
    /// in whole seconds, is over.
    #[error("Too many failed sign-in attempts. Try again in {}.", describe_wait(*retry_after))]
    TooManyAttempts { retry_after: u64 },
    /// More passwords are waiting to be checked than the server takes on at once; `retry_after`
    /// is in whole seconds.
    #[error("The server is busy. Try again in {}.", describe_wait(*retry_after))]
    Busy { retry_after: u64 },
    /// The current password given to change it is wrong. Unlike
    /// [`InvalidCredentials`](Self::InvalidCredentials) it leaves the session signed in.
    #[error("The current password is wrong.")]
    WrongPassword,
    /// A sensitive change needs the user to have signed in or confirmed it was them within
    /// the last minutes. `methods` are the ways they can confirm, such as `password`; with
    /// none they have to sign in again.
    #[error("{}", describe_reauthentication(.methods))]
    ReauthenticationRequired { methods: Vec<&'static str> },
    /// The account signs in through single sign-on and has no password to change.
    #[error("This account signs in through single sign-on and has no password.")]
    NoPassword,
    /// The account is disabled or marked for deletion, so it may not sign in.
    #[error("This account is disabled.")]
    AccountDisabled,
    /// The username is already taken.
    #[error("This username is already taken.")]
    UsernameTaken,
    /// The username or password does not meet the requirements.
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    /// Single sign-on is not configured on this server.
    #[error("Single sign-on is not enabled.")]
    OidcDisabled,
    /// The browser sign-in of an app is unknown, already answered or expired.
    #[error("This sign-in request was already used, cancelled or has expired.")]
    LoginExpired,
    /// The identity provider authenticated the user but access is restricted to a group.
    #[error("You are not allowed to sign in with this account.")]
    OidcDenied,
    /// The identity provider could not be reached or sent something invalid.
    #[error("identity provider error: {0}")]
    Oidc(String),
    /// The password was right and the account also needs a second factor. `login_token`
    /// lets a client that asked for two steps finish at `/auth/login/verify`; others only
    /// learn why the sign-in failed.
    #[error("This account uses two-factor sign-in. Sign in in the browser, or update the app.")]
    SecondFactorRequired {
        login_token: Option<String>,
        methods: Vec<&'static str>,
    },
    /// The authenticator app's code, or the recovery code, is wrong or was used already.
    #[error("The code is wrong. Check it and try again.")]
    InvalidCode,
    /// An authenticator app is already set up.
    #[error("An authenticator app is already set up. Remove it first to add another.")]
    TotpExists,
    /// No `HOMESPACE_SECRETS_KEY` is configured, so authenticator app secrets can't be kept.
    #[error(
        "Authenticator apps can't be used on this server yet: its administrator has to set \
         HOMESPACE_SECRETS_KEY."
    )]
    SecretsKeyMissing,
    /// The answer of a passkey or security key is wrong, replayed, for another challenge, or
    /// from a credential the account does not have here.
    #[error("The passkey or security key could not be verified. Try again.")]
    WebAuthnFailed,
    /// Passkeys and security keys can't be used from where the request came; says why.
    #[error("{0}")]
    WebAuthnUnavailable(&'static str),
    /// The account has as many passkeys and security keys as it may.
    #[error(
        "This account has {} passkeys and security keys, the most it may have. Remove one first.",
        db::MAX_WEBAUTHN_CREDENTIALS
    )]
    TooManyKeys,
    /// A passkey or security key of the account that does not exist.
    #[error("There is no such passkey or security key.")]
    UnknownKey,
    /// A sealed secret opens with none of the configured keys.
    #[error(
        "an authenticator app secret could not be opened; is HOMESPACE_SECRETS_KEY the key it was sealed with?"
    )]
    Unsealing,
    /// A secret could not be sealed.
    #[error("an authenticator app secret could not be sealed")]
    Sealing,
    #[error("password hashing failed: {0}")]
    Hashing(String),
    #[error("token encoding failed: {0}")]
    Token(#[from] jsonwebtoken::errors::Error),
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Db(#[from] db::DbError),
}

impl AsProblem for AuthError {
    fn problem_type(&self) -> ProblemType {
        match self {
            Self::InvalidCredentials | Self::WrongLogin | Self::LoginCodeInvalid => {
                ProblemType::Unauthorized
            }
            Self::TooManyAttempts { .. } => ProblemType::TooManyAttempts,
            Self::Busy { .. } => ProblemType::ServerBusy,
            Self::WrongPassword => ProblemType::WrongPassword,
            Self::ReauthenticationRequired { .. } => ProblemType::ReauthenticationRequired,
            Self::NoPassword => ProblemType::NoPassword,
            Self::SecondFactorRequired { .. } => ProblemType::SecondFactorRequired,
            Self::InvalidCode => ProblemType::InvalidCode,
            Self::TotpExists => ProblemType::TotpExists,
            Self::SecretsKeyMissing => ProblemType::SecretsKeyMissing,
            Self::WebAuthnFailed => ProblemType::WebAuthnFailed,
            Self::WebAuthnUnavailable(_) => ProblemType::WebAuthnUnavailable,
            Self::TooManyKeys => ProblemType::TooManyKeys,
            Self::UnknownKey => ProblemType::NotFound,
            Self::AccountDisabled => ProblemType::AccountDisabled,
            Self::UsernameTaken => ProblemType::UsernameTaken,
            Self::InvalidInput(_) => ProblemType::ValidationFailed,
            Self::OidcDisabled => ProblemType::SsoDisabled,
            Self::OidcDenied => ProblemType::SsoAccessDenied,
            Self::LoginExpired => ProblemType::LoginExpired,
            Self::Oidc(_) => ProblemType::IdentityProviderFailed,
            Self::Unsealing
            | Self::Sealing
            | Self::Hashing(_)
            | Self::Token(_)
            | Self::Join(_)
            | Self::Db(_) => ProblemType::Internal,
        }
    }

    fn extend(&self, problem: &mut Problem) {
        match self {
            Self::TooManyAttempts { retry_after } | Self::Busy { retry_after } => {
                problem.retry_after = Some(*retry_after);
            }
            Self::ReauthenticationRequired { methods } => {
                problem.methods = methods.iter().map(|&method| method.to_owned()).collect();
            }
            Self::SecondFactorRequired {
                login_token,
                methods,
            } => {
                problem.login_token.clone_from(login_token);
                problem.methods = methods.iter().map(|&method| method.to_owned()).collect();
            }
            _ => {}
        }
    }
}

/// What the user is asked to do to confirm it is them, given the ways they can.
fn describe_reauthentication(methods: &[&str]) -> String {
    let ways: Vec<&str> = [
        ("password", "enter your password again"),
        ("totp", "enter a code from your authenticator app"),
        ("webauthn", "use your passkey or security key"),
    ]
    .into_iter()
    .filter(|(method, _)| methods.contains(method))
    .map(|(_, way)| way)
    .collect();
    match ways.as_slice() {
        [] => "Sign in again to confirm it is you.".to_owned(),
        [way] => format!("Confirm it is you: {way}."),
        [first @ .., last] => format!("Confirm it is you: {}, or {last}.", first.join(", ")),
    }
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
