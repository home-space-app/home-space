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

impl AuthService {
    /// The user of `session_id` proves again who they are with their `password`, which allows
    /// sensitive changes from that session for the next ten minutes.
    ///
    /// Wrong passwords are throttled as when changing the password, counted for the session
    /// asking, so a stolen session cannot be used to guess the password.
    pub async fn reauthenticate(
        &self,
        user_id: UserId,
        session_id: Uuid,
        password: &str,
    ) -> Result<(), AuthError> {
        self.check_current_password(user_id, session_id, password)
            .await?;
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
        let methods = if user.password_hash.is_some() {
            vec![PASSWORD_PROOF]
        } else {
            Vec::new()
        };
        Err(AuthError::ReauthenticationRequired { methods })
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
