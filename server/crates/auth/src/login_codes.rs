//! The one-time codes a client trades for a session after signing in in a browser, through the
//! identity provider or the server's own web page. Each is bound to the client's PKCE challenge
//! (RFC 7636), so only the client that started the sign-in can use it.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use time::{Duration, OffsetDateTime};

use db::{Db, TenantId, UserId};

use crate::{AuthError, oidc::verifier_matches, token::hash_refresh_token};

const LOGIN_CODE_TTL: Duration = Duration::seconds(60);
const RANDOM_TOKEN_BYTES: usize = 32;

/// The login codes of one tenant.
#[derive(Clone, Debug)]
pub(crate) struct LoginCodes {
    db: Db,
    tenant: TenantId,
}

impl LoginCodes {
    pub(crate) fn new(db: Db, tenant: TenantId) -> Self {
        Self { db, tenant }
    }

    /// A new code for `user_id`, usable once within a minute with the verifier of `challenge`.
    pub(crate) async fn issue(
        &self,
        user_id: UserId,
        challenge: &str,
    ) -> Result<String, AuthError> {
        let code = random_token();
        self.db
            .insert_login_code(
                self.tenant,
                &hash_refresh_token(&code),
                user_id,
                challenge,
                OffsetDateTime::now_utc() + LOGIN_CODE_TTL,
            )
            .await?;
        Ok(code)
    }

    /// The user `code` was issued to, when `code_verifier` answers its challenge. The code is
    /// used up either way.
    pub(crate) async fn redeem(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<UserId, AuthError> {
        let stored = self
            .db
            .take_login_code(self.tenant, &hash_refresh_token(code))
            .await?
            .ok_or(AuthError::LoginCodeInvalid)?;
        if verifier_matches(code_verifier, &stored.client_challenge) {
            Ok(stored.user_id)
        } else {
            Err(AuthError::LoginCodeInvalid)
        }
    }
}

/// 256 random bits, base64url: for codes, `state` and flow ids, which are stored only hashed.
pub(crate) fn random_token() -> String {
    let mut bytes = [0u8; RANDOM_TOKEN_BYTES];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
