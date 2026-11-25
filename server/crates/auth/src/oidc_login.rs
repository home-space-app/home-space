//! Signing in through the tenant's identity provider, and creating the local user of a
//! provider account on its first sign-in.

use time::{Duration, OffsetDateTime};

use db::{Authentication, Db, DbError, OidcPending, TenantId, UserId};
use protocol::rules::MAX_USERNAME_LEN;

use crate::{
    AuthError, OidcService,
    login_codes::{LoginCodes, random_token},
    oidc::{Identity, is_valid_challenge},
    sessions::{OIDC_METHOD, Sessions, signed_in_now},
    token::hash_refresh_token,
};

const OIDC_PENDING_TTL: Duration = Duration::minutes(10);
const USERNAME_ATTEMPTS: u32 = 20;
pub(crate) const FALLBACK_USERNAME: &str = "user";

/// The single sign-on of one tenant.
#[derive(Clone, Debug)]
pub(crate) struct OidcLogin {
    db: Db,
    tenant: TenantId,
    codes: LoginCodes,
    pub(crate) provider: OidcService,
}

impl OidcLogin {
    pub(crate) fn new(db: Db, tenant: TenantId, provider: OidcService) -> Self {
        Self {
            codes: LoginCodes::new(db.clone(), tenant),
            db,
            tenant,
            provider,
        }
    }

    /// The provider URL to send the user to. `redirect_uri` is where the client wants to land
    /// afterwards and `code_challenge` is the S256 PKCE challenge its later
    /// [`exchange`](Self::exchange) must answer.
    pub(crate) async fn start(
        &self,
        redirect_uri: &str,
        code_challenge: &str,
    ) -> Result<String, AuthError> {
        if !self.provider.allows_redirect(redirect_uri) {
            return Err(AuthError::InvalidInput("redirect_uri is not allowed"));
        }
        if !is_valid_challenge(code_challenge) {
            return Err(AuthError::InvalidInput(
                "code_challenge must be an S256 challenge",
            ));
        }
        let state = random_token();
        let authorization = self.provider.authorize(&state);
        let pending = OidcPending {
            nonce: authorization.nonce,
            idp_pkce_verifier: authorization.pkce_verifier,
            client_redirect: redirect_uri.to_owned(),
            client_challenge: code_challenge.to_owned(),
        };
        self.db
            .insert_oidc_pending(
                self.tenant,
                &hash_refresh_token(&state),
                &pending,
                OffsetDateTime::now_utc() + OIDC_PENDING_TTL,
            )
            .await?;
        Ok(authorization.url)
    }

    /// The client URL to send the user to once the provider sent them back: it carries a
    /// one-time `code` or, if the sign-in failed, an `error`. A `state` that is missing,
    /// unknown, expired or already used has no client to go to, so the browser goes to the
    /// web client with `error=login_expired`.
    pub(crate) async fn callback(
        &self,
        sessions: &Sessions,
        state: Option<&str>,
        code: Option<&str>,
    ) -> Result<String, AuthError> {
        let pending = match state {
            Some(state) => {
                self.db
                    .take_oidc_pending(self.tenant, &hash_refresh_token(state))
                    .await?
            }
            None => None,
        };
        let Some(pending) = pending else {
            tracing::info!("single sign-on came back with an unknown or used state");
            return Ok(append_query(
                &self.provider.web_redirect(),
                ("error", "login_expired".to_owned()),
            ));
        };
        let outcome = match code {
            Some(code) => self.finish(sessions, code, &pending).await,
            None => Err(AuthError::OidcDenied),
        };
        let params = match outcome {
            Ok(code) => ("code", code),
            Err(AuthError::OidcDenied) => ("error", "access_denied".to_owned()),
            Err(AuthError::AccountDisabled) => ("error", "account_disabled".to_owned()),
            Err(err) => {
                tracing::warn!(error = %err, "single sign-on failed");
                ("error", "login_failed".to_owned())
            }
        };
        Ok(append_query(&pending.client_redirect, params))
    }

    /// The user the one-time `code` from the redirect was issued to and how they signed in,
    /// when `code_verifier` answers the client's challenge.
    pub(crate) async fn exchange(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<(UserId, Authentication), AuthError> {
        self.codes.redeem(code, code_verifier).await
    }

    /// Verifies the provider's answer, provisions the user and stores a login code for them.
    async fn finish(
        &self,
        sessions: &Sessions,
        code: &str,
        pending: &OidcPending,
    ) -> Result<String, AuthError> {
        let identity = self
            .provider
            .complete(code, &pending.nonce, &pending.idp_pkce_verifier)
            .await?;
        let user_id = self.user_for_identity(&identity).await?;
        sessions.require_may_sign_in(user_id).await?;
        self.codes
            .issue(
                user_id,
                &pending.client_challenge,
                signed_in_now(OIDC_METHOD),
            )
            .await
    }

    /// The local user for a provider account, created on first sign-in.
    async fn user_for_identity(&self, identity: &Identity) -> Result<UserId, AuthError> {
        let find = || {
            self.db
                .find_user_by_identity(self.tenant, &identity.issuer, &identity.subject)
        };
        if let Some(user) = find().await? {
            return Ok(user.id);
        }
        let base = username_base(identity);
        for attempt in 0..USERNAME_ATTEMPTS {
            let username = username_candidate(&base, attempt);
            match self
                .db
                .create_user_with_identity(
                    self.tenant,
                    &username,
                    &identity.issuer,
                    &identity.subject,
                    identity.email.as_deref(),
                )
                .await
            {
                Ok(user) => return Ok(user.id),
                // Either the name is taken or a concurrent sign-in created this very identity.
                Err(DbError::Conflict) => {
                    if let Some(user) = find().await? {
                        return Ok(user.id);
                    }
                }
                Err(err) => return Err(err.into()),
            }
        }
        Err(AuthError::InvalidInput("no free username for this account"))
    }
}

pub(crate) fn append_query(url: &str, (key, value): (&str, impl AsRef<str>)) -> String {
    // Redirects are validated to carry no query or fragment, so a plain `?` is enough.
    let value: String =
        openidconnect::url::form_urlencoded::byte_serialize(value.as_ref().as_bytes()).collect();
    format!("{url}?{key}={value}")
}

/// The preferred name for a new user: the provider's username, else the e-mail's local part,
/// else the subject.
pub(crate) fn username_base(identity: &Identity) -> String {
    let from_email = identity
        .email
        .as_deref()
        .and_then(|email| email.split('@').next());
    let name = identity
        .username_hint
        .as_deref()
        .or(from_email)
        .unwrap_or(&identity.subject);
    let cleaned: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    if cleaned.is_empty() {
        FALLBACK_USERNAME.to_owned()
    } else {
        cleaned
    }
}

/// `base`, or `base-2`, `base-3`, ... for later attempts, cut to fit [`MAX_USERNAME_LEN`] bytes.
pub(crate) fn username_candidate(base: &str, attempt: u32) -> String {
    let suffix = if attempt == 0 {
        String::new()
    } else {
        format!("-{}", attempt + 1)
    };
    let mut name = String::new();
    for c in base.chars() {
        if name.len() + c.len_utf8() + suffix.len() > MAX_USERNAME_LEN {
            break;
        }
        name.push(c);
    }
    name + &suffix
}
