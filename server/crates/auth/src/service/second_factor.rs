//! Two-factor sign-in with an authenticator app (TOTP) and recovery codes, as [`AuthService`]
//! offers it: the second step of a sign-in, adding and removing the app, new recovery codes,
//! and an administrator's reset.

use std::net::IpAddr;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use db::{DbError, User, UserId};
use protocol::{DeviceInfo, RecoveryCodes, TokenPair, TotpEnrolment};

use super::AuthService;
use crate::{
    AuthError, SecretBox,
    login_codes::random_token,
    recovery_codes,
    sessions::{OTP_METHOD, PASSWORD_METHOD, RECOVERY_METHOD, signed_in_now_with},
    throttle::{Counter, attempt_source},
    token::hash_refresh_token,
    totp,
};

/// How long the second step of a sign-in may take.
const LOGIN_TOKEN_TTL: Duration = Duration::minutes(5);
/// Wrong codes that end a waiting sign-in, so the password has to be given again.
const MAX_CODE_FAILURES: i32 = 5;
/// The throttle counts wrong second factors of a username under this source.
const SECOND_FACTOR_SOURCE: &str = "2fa";
/// The issuer an authenticator app shows next to the account.
const TOTP_ISSUER: &str = "Home Space";
/// The second factors a sign-in can finish with, as clients are told them.
const SECOND_FACTOR_METHODS: [&str; 2] = ["totp", "recovery"];

/// What finishes a sign-in that waits for its second factor.
#[derive(Clone, Copy, Debug)]
pub enum SecondFactorProof<'a> {
    /// A code from the authenticator app.
    Totp(&'a str),
    /// One of the recovery codes, used up by this.
    Recovery(&'a str),
}

impl AuthService {
    /// Keeps authenticator app secrets with `keys`, the tenant's `HOMESPACE_SECRETS_KEY`;
    /// without them no app can be added, and apps added before can't be used.
    #[must_use]
    pub fn with_secrets_key(mut self, keys: Option<SecretBox>) -> Self {
        self.secret_box = keys;
        self
    }

    /// After the right password of `user`: refuses with
    /// [`AuthError::SecondFactorRequired`] when they have a second factor. With `two_step`
    /// the refusal carries the token that [`login_verify`](Self::login_verify) finishes the
    /// sign-in with.
    pub(super) async fn ask_second_factor(
        &self,
        user: &User,
        two_step: bool,
    ) -> Result<(), AuthError> {
        if !self.db.second_factors(user.id).await?.totp {
            return Ok(());
        }
        if !user.may_sign_in() {
            return Err(AuthError::AccountDisabled);
        }
        let login_token = if two_step {
            let token = random_token();
            let expires_at = OffsetDateTime::now_utc() + LOGIN_TOKEN_TTL;
            self.db
                .insert_pending_login(
                    self.tenant,
                    &hash_refresh_token(&token),
                    user.id,
                    expires_at,
                )
                .await?;
            Some(token)
        } else {
            None
        };
        Err(AuthError::SecondFactorRequired {
            login_token,
            methods: SECOND_FACTOR_METHODS.to_vec(),
        })
    }

    /// Finishes a sign-in whose password was right with its second factor, sent from
    /// `address`, and starts a session on `device`.
    ///
    /// Wrong codes are throttled for the username like wrong passwords, and after five the
    /// sign-in ends ([`AuthError::LoginExpired`]), so the password has to be given again.
    pub async fn login_verify(
        &self,
        login_token: &str,
        proof: SecondFactorProof<'_>,
        device: Option<&DeviceInfo>,
        address: Option<IpAddr>,
    ) -> Result<TokenPair, AuthError> {
        let token_hash = hash_refresh_token(login_token);
        let pending = self
            .db
            .pending_login(self.tenant, &token_hash)
            .await?
            .ok_or(AuthError::LoginExpired)?;
        let user = self
            .db
            .find_user(pending.user_id)
            .await?
            .ok_or(AuthError::LoginExpired)?;
        let mut counters = Vec::with_capacity(2);
        if let Some(address) = address {
            counters.push(Counter::address(attempt_source(address)));
        }
        counters.push(Counter::key(
            self.tenant,
            &user.username,
            SECOND_FACTOR_SOURCE,
        ));
        let attempt = self.throttle.admit(counters).await?;
        let checked = self.check_second_factor(&user, proof).await;
        let method = match attempt
            .settle(checked, |err| matches!(err, AuthError::InvalidCode))
            .await
        {
            Ok(method) => method,
            Err(AuthError::InvalidCode) => {
                let failures = self.db.count_pending_login_failure(&token_hash).await?;
                if failures.is_none_or(|failures| failures >= MAX_CODE_FAILURES) {
                    self.db.take_pending_login(&token_hash).await?;
                    return Err(AuthError::LoginExpired);
                }
                return Err(AuthError::InvalidCode);
            }
            Err(err) => return Err(err),
        };
        if !self.db.take_pending_login(&token_hash).await? {
            return Err(AuthError::LoginExpired);
        }
        let authentication = signed_in_now_with(&[PASSWORD_METHOD, method]);
        self.sessions.start(user.id, device, &authentication).await
    }

    /// Checks `proof` for `user`, using it up: returns the method a session records for it.
    async fn check_second_factor(
        &self,
        user: &User,
        proof: SecondFactorProof<'_>,
    ) -> Result<&'static str, AuthError> {
        match proof {
            SecondFactorProof::Totp(code) => {
                self.check_totp(user, code).await?;
                Ok(OTP_METHOD)
            }
            SecondFactorProof::Recovery(code) => {
                if !self
                    .db
                    .use_recovery_code(user.id, &recovery_codes::hash(code))
                    .await?
                {
                    return Err(AuthError::InvalidCode);
                }
                let left = self.db.second_factors(user.id).await?.recovery_codes_left;
                tracing::info!(user = %user.id, left, "recovery code used to sign in");
                Ok(RECOVERY_METHOD)
            }
        }
    }

    /// Checks a code of `user`'s confirmed authenticator app and uses it up, so it works
    /// once. A secret sealed with an earlier key is sealed again with the current one.
    pub(super) async fn check_totp(&self, user: &User, code: &str) -> Result<(), AuthError> {
        let stored = self
            .db
            .totp(user.id)
            .await?
            .filter(|stored| stored.confirmed_at.is_some())
            .ok_or(AuthError::InvalidCode)?;
        let keys = self.secret_box()?;
        let opened = keys.open(self.tenant, user.id, &stored.secret)?;
        let step = totp::matching_step(&opened.plain, code, OffsetDateTime::now_utc())
            .filter(|&step| step > stored.last_step)
            .ok_or(AuthError::InvalidCode)?;
        if !self.db.accept_totp_step(user.id, step).await? {
            return Err(AuthError::InvalidCode);
        }
        if opened.stale {
            let sealed = keys.seal(self.tenant, user.id, &opened.plain);
            self.db.reseal_totp(user.id, &sealed).await?;
        }
        Ok(())
    }

    /// Starts adding an authenticator app for the user of `session_id`, who must have signed
    /// in or confirmed it is them recently. `account` names the account in the app, such as
    /// `alice@files.example.com`. The app counts only once a code from it is confirmed with
    /// [`confirm_totp`](Self::confirm_totp).
    pub async fn start_totp(
        &self,
        user_id: UserId,
        session_id: Uuid,
        account: &str,
    ) -> Result<TotpEnrolment, AuthError> {
        self.require_recent_sign_in(user_id, session_id).await?;
        let user = self.user(user_id).await?;
        if user.password_hash.is_none() {
            return Err(AuthError::NoPassword);
        }
        let keys = self.secret_box()?;
        let secret = totp::new_secret();
        let sealed = keys.seal(self.tenant, user_id, &secret);
        match self.db.start_totp(self.tenant, user_id, &sealed).await {
            Err(DbError::Conflict) => return Err(AuthError::TotpExists),
            other => other?,
        }
        Ok(TotpEnrolment {
            secret: totp::encode_secret(&secret),
            uri: totp::otpauth_uri(TOTP_ISSUER, account, &secret),
        })
    }

    /// Confirms the new authenticator app with its first `code`: from now on signing in takes
    /// one of its codes. Returns the user's recovery codes, which replace any earlier ones.
    pub async fn confirm_totp(
        &self,
        user_id: UserId,
        session_id: Uuid,
        code: &str,
    ) -> Result<RecoveryCodes, AuthError> {
        self.require_recent_sign_in(user_id, session_id).await?;
        let stored = self.db.totp(user_id).await?.ok_or(AuthError::InvalidCode)?;
        if stored.confirmed_at.is_some() {
            return Err(AuthError::TotpExists);
        }
        let opened = self
            .secret_box()?
            .open(self.tenant, user_id, &stored.secret)?;
        let step = totp::matching_step(&opened.plain, code, OffsetDateTime::now_utc())
            .ok_or(AuthError::InvalidCode)?;
        let codes = recovery_codes::new_set();
        let hashes = hashes(&codes);
        match self
            .db
            .confirm_totp(self.tenant, user_id, step, &hashes)
            .await
        {
            Err(DbError::NotFound) => return Err(AuthError::InvalidCode),
            other => other?,
        }
        tracing::info!(user = %user_id, "authenticator app added");
        Ok(RecoveryCodes { codes })
    }

    /// Removes the authenticator app and the recovery codes of the user of `session_id`, who
    /// must have signed in or confirmed it is them recently. Their sessions stay.
    pub async fn remove_totp(&self, user_id: UserId, session_id: Uuid) -> Result<(), AuthError> {
        self.require_recent_sign_in(user_id, session_id).await?;
        self.db.remove_second_factors(user_id).await?;
        tracing::info!(user = %user_id, "authenticator app removed");
        Ok(())
    }

    /// A new set of recovery codes for the user of `session_id`, voiding the old ones. They
    /// must have an authenticator app, and have signed in or confirmed it is them recently.
    pub async fn new_recovery_codes(
        &self,
        user_id: UserId,
        session_id: Uuid,
    ) -> Result<RecoveryCodes, AuthError> {
        self.require_recent_sign_in(user_id, session_id).await?;
        if !self.db.second_factors(user_id).await?.totp {
            return Err(AuthError::InvalidInput(
                "recovery codes need an authenticator app",
            ));
        }
        let codes = recovery_codes::new_set();
        self.db
            .replace_recovery_codes(self.tenant, user_id, &hashes(&codes))
            .await?;
        tracing::info!(user = %user_id, "new recovery codes");
        Ok(RecoveryCodes { codes })
    }

    /// An administrator's reset: removes the second factors of `user_id` and ends all their
    /// sessions, so they sign in with the password alone. The caller checks that the user is
    /// of this tenant and that the administrator confirmed it is them.
    pub async fn reset_second_factors(&self, user_id: UserId) -> Result<(), AuthError> {
        self.db.reset_second_factors(self.tenant, user_id).await?;
        self.sessions_ended(user_id);
        tracing::info!(user = %user_id, "second factors reset by an administrator");
        Ok(())
    }

    fn secret_box(&self) -> Result<&SecretBox, AuthError> {
        self.secret_box.as_ref().ok_or(AuthError::SecretsKeyMissing)
    }

    pub(super) async fn user(&self, user_id: UserId) -> Result<User, AuthError> {
        self.db
            .find_user(user_id)
            .await?
            .filter(|user| user.tenant_id == self.tenant)
            .ok_or(AuthError::InvalidCredentials)
    }
}

fn hashes(codes: &[String]) -> Vec<Vec<u8>> {
    codes
        .iter()
        .map(|code| recovery_codes::hash(code))
        .collect()
}
