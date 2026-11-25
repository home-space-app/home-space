//! Confirming it is still the user before a sensitive change ("sudo mode"): a session's user
//! proves again who they are, and for a while after that, or after signing in, the change is
//! allowed.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use db::UserId;

use super::AuthService;
use crate::{AuthError, password::MAX_PASSWORD_LEN, throttle::Counter};

/// How long after signing in or confirming a sensitive change is allowed.
const RECENT: Duration = Duration::minutes(10);
/// The name of the current password among the ways to confirm, as clients are told them.
pub(crate) const PASSWORD_PROOF: &str = "password";
/// The name of an authenticator app's code among the ways to confirm.
pub(crate) const TOTP_PROOF: &str = "totp";

/// How the user proves again who they are.
#[derive(Clone, Copy, Debug)]
pub enum ReauthProof<'a> {
    /// Their current password.
    Password(&'a str),
    /// A code from their authenticator app.
    Code(&'a str),
}

impl AuthService {
    /// The user of `session_id` proves again who they are, which allows sensitive changes
    /// from that session for the next ten minutes.
    ///
    /// Wrong passwords and codes are throttled as when changing the password, counted for
    /// the session asking, so a stolen session cannot be used to guess them.
    pub async fn reauthenticate(
        &self,
        user_id: UserId,
        session_id: Uuid,
        proof: ReauthProof<'_>,
    ) -> Result<(), AuthError> {
        match proof {
            ReauthProof::Password(password) => {
                self.check_current_password(user_id, session_id, password)
                    .await?;
            }
            ReauthProof::Code(code) => self.check_current_code(user_id, session_id, code).await?,
        }
        self.sessions.confirm(session_id, user_id).await
    }

    /// Refuses with [`AuthError::ReauthenticationRequired`] unless the user of `session_id`
    /// signed in or confirmed it was them within the last ten minutes. The refusal names the
    /// ways they can confirm; none means they have to sign in again.
    pub async fn require_recent_sign_in(
        &self,
        user_id: UserId,
        session_id: Uuid,
    ) -> Result<(), AuthError> {
        let authentication = self.sessions.authentication(session_id, user_id).await?;
        if OffsetDateTime::now_utc() - authentication.authenticated_at <= RECENT {
            return Ok(());
        }
        let user = self
            .db
            .find_user(user_id)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        let mut methods = Vec::with_capacity(2);
        if user.password_hash.is_some() {
            methods.push(PASSWORD_PROOF);
        }
        if self.db.second_factors(user_id).await?.totp {
            methods.push(TOTP_PROOF);
        }
        Err(AuthError::ReauthenticationRequired { methods })
    }

    /// Checks a code of the authenticator app of `user_id`, given from `session_id`, with
    /// wrong codes throttled like wrong current passwords.
    async fn check_current_code(
        &self,
        user_id: UserId,
        session_id: Uuid,
        code: &str,
    ) -> Result<(), AuthError> {
        let user = self.user(user_id).await?;
        let source = format!("session:{session_id}");
        let attempt = self
            .throttle
            .admit(vec![Counter::key(user.tenant_id, &user.username, source)])
            .await?;
        let checked = self.check_totp(&user, code).await;
        attempt
            .settle(checked, |err| matches!(err, AuthError::InvalidCode))
            .await
    }

    /// Checks the current password of `user_id`, given from `session_id`.
    ///
    /// Wrong passwords are throttled like wrong sign-ins, counted for the session asking.
    /// Sign-in is not affected: whoever holds a session must not be able to keep its owner
    /// from signing in.
    pub(super) async fn check_current_password(
        &self,
        user_id: UserId,
        session_id: Uuid,
        current: &str,
    ) -> Result<(), AuthError> {
        let user = self
            .db
            .find_user(user_id)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        let hash = user.password_hash.ok_or(AuthError::NoPassword)?;
        // No password is that long, so it is not worth a count or the time to hash it.
        if current.len() > MAX_PASSWORD_LEN {
            return Err(AuthError::WrongPassword);
        }
        let source = format!("session:{session_id}");
        let attempt = self
            .throttle
            .admit(vec![Counter::key(user.tenant_id, user.username, source)])
            .await?;
        let checked = match self.hasher.verify(current, hash).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(AuthError::WrongPassword),
            Err(err) => Err(err),
        };
        attempt
            .settle(checked, |err| matches!(err, AuthError::WrongPassword))
            .await
    }
}
