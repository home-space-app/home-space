//! Confirming it is still the user before a sensitive change ("sudo mode"): a session's user
//! proves again who they are, and for a while after that, or after signing in, the change is
//! allowed.
//!
//! Changes to how the account signs in (its password, authenticator app, recovery codes,
//! passkeys and security keys) ask more once the account has a second factor: only a recent
//! proof with a second factor counts, so that a stolen session and password cannot take the
//! second factor away or add one of the thief's.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use std::fmt;

use db::{Confirmation, SecondFactorState, User, UserId};
use protocol::{AuthMethod, PublicKeyCredential, redact::Redacted};

use super::AuthService;
use crate::{
    AuthError,
    password::MAX_PASSWORD_LEN,
    throttle::{Counter, session_source},
};

/// How long after signing in or confirming a sensitive change is allowed.
const RECENT: Duration = Duration::minutes(10);
/// How the user proves again who they are.
#[derive(Clone, Copy)]
pub enum ReauthProof<'a> {
    /// Their current password.
    Password(&'a str),
    /// A code from their authenticator app.
    Code(&'a str),
    /// A passkey's or security key's answer to the challenge of
    /// [`reauth_webauthn_options`](AuthService::reauth_webauthn_options).
    WebAuthn {
        challenge_id: Uuid,
        answer: &'a PublicKeyCredential,
    },
}

impl fmt::Debug for ReauthProof<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password(_) => f.debug_tuple("Password").field(&Redacted).finish(),
            Self::Code(_) => f.debug_tuple("Code").field(&Redacted).finish(),
            Self::WebAuthn {
                challenge_id,
                answer,
            } => f
                .debug_struct("WebAuthn")
                .field("challenge_id", challenge_id)
                .field("answer", answer)
                .finish(),
        }
    }
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
        if self.user(user_id).await?.external {
            return Err(AuthError::ExternalAccount);
        }
        let with = match proof {
            ReauthProof::Password(password) => {
                self.check_current_password(user_id, session_id, password)
                    .await?;
                Confirmation::Password
            }
            ReauthProof::Code(code) => {
                self.check_current_code(user_id, session_id, code).await?;
                Confirmation::SecondFactor
            }
            ReauthProof::WebAuthn {
                challenge_id,
                answer,
            } => {
                self.check_current_key(user_id, session_id, challenge_id, answer)
                    .await?;
                Confirmation::SecondFactor
            }
        };
        self.sessions.confirm(session_id, user_id, with).await
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
        if is_recent(authentication.authenticated_at) {
            return Ok(());
        }
        let user = self
            .db
            .find_user(user_id)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        let mut methods = Vec::with_capacity(3);
        // The identity provider confirms who they are: they sign in there again.
        if user.external {
            return Err(AuthError::ReauthenticationRequired { methods });
        }
        if user.password_hash.is_some() {
            methods.push(AuthMethod::Password);
        }
        let factors = self.db.second_factors(user_id).await?;
        methods.extend(factor_proofs(factors));
        Err(AuthError::ReauthenticationRequired { methods })
    }

    /// Like [`require_recent_sign_in`](Self::require_recent_sign_in), for a change to how the
    /// user signs in. Once the account has a second factor, only a recent proof with one
    /// counts: a sign-in that used one, or a confirmation with a code, a passkey or a security
    /// key; the refusal then names only those. Without one, the password does as for any
    /// sensitive change.
    pub async fn require_recent_second_factor(
        &self,
        user_id: UserId,
        session_id: Uuid,
    ) -> Result<(), AuthError> {
        let factors = self.db.second_factors(user_id).await?;
        if !factors.any() {
            return self.require_recent_sign_in(user_id, session_id).await;
        }
        self.recent_second_factor(user_id, session_id, factors)
            .await
    }

    /// For a change the current password allows by itself, such as setting a new one: once
    /// the account has a second factor, refuses as
    /// [`require_recent_second_factor`](Self::require_recent_second_factor) does unless a
    /// second factor proved the user recently too.
    pub(super) async fn require_second_factor_if_enrolled(
        &self,
        user_id: UserId,
        session_id: Uuid,
    ) -> Result<(), AuthError> {
        let factors = self.db.second_factors(user_id).await?;
        if !factors.any() {
            return Ok(());
        }
        self.recent_second_factor(user_id, session_id, factors)
            .await
    }

    /// Refuses unless a second factor proved the user of `session_id`, whose account has
    /// `factors`, within the last ten minutes.
    async fn recent_second_factor(
        &self,
        user_id: UserId,
        session_id: Uuid,
        factors: SecondFactorState,
    ) -> Result<(), AuthError> {
        let authentication = self.sessions.authentication(session_id, user_id).await?;
        if authentication.second_factor_at.is_some_and(is_recent) {
            return Ok(());
        }
        // The identity provider confirms who they are: they sign in there again.
        let methods = if self.user(user_id).await?.external {
            Vec::new()
        } else {
            factor_proofs(factors).collect()
        };
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
        let attempt = self
            .throttle
            .admit(vec![session_counter(&user, session_id)])
            .await?;
        let checked = self.check_totp(&user, code).await;
        attempt
            .settle(checked, |err| matches!(err, AuthError::InvalidCode))
            .await
    }

    /// Checks the answer of a passkey or security key of `user_id`, given from `session_id`,
    /// with wrong answers throttled like wrong current passwords.
    async fn check_current_key(
        &self,
        user_id: UserId,
        session_id: Uuid,
        challenge_id: Uuid,
        answer: &PublicKeyCredential,
    ) -> Result<(), AuthError> {
        let user = self.user(user_id).await?;
        let attempt = self
            .throttle
            .admit(vec![session_counter(&user, session_id)])
            .await?;
        let checked = self
            .check_reauth_webauthn(user_id, challenge_id, answer)
            .await;
        attempt
            .settle(checked, |err| matches!(err, AuthError::WebAuthnFailed))
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
        let counter = session_counter(&user, session_id);
        let hash = user.password_hash.ok_or(AuthError::NoPassword)?;
        // No password is that long, so it is not worth a count or the time to hash it.
        if current.len() > MAX_PASSWORD_LEN {
            return Err(AuthError::WrongPassword);
        }
        let attempt = self.throttle.admit(vec![counter]).await?;
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

/// Whether a proof made `at` still allows sensitive changes.
fn is_recent(at: OffsetDateTime) -> bool {
    OffsetDateTime::now_utc() - at <= RECENT
}

/// Wrong proofs of `user` given from `session_id`, which count only against that session.
fn session_counter(user: &User, session_id: Uuid) -> Counter {
    Counter::key(
        user.tenant_id,
        user.username.as_str(),
        session_source(session_id),
    )
}

/// The second factors an account with `factors` can confirm it is the user with.
fn factor_proofs(factors: SecondFactorState) -> impl Iterator<Item = AuthMethod> {
    [
        (factors.totp, AuthMethod::Totp),
        (factors.webauthn > 0, AuthMethod::Webauthn),
    ]
    .into_iter()
    .filter_map(|(has, name)| has.then_some(name))
}
