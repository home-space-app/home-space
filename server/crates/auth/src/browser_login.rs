//! Browser sign-in for apps: an app opens the server's own web page in the system browser,
//! the user signs in there with whatever the tenant offers and lets the app in, and the app
//! gets a one-time code at its redirect, which it trades for a session with PKCE (RFC 7636,
//! RFC 8252). The web page plays the part an identity provider plays in single sign-on.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use db::{Authentication, Db, TenantId, UserId};
use protocol::{BrowserLoginFlow, paths};

use crate::{
    AuthError,
    login_codes::{LoginCodes, random_token},
    oidc::{is_valid_challenge, native_app},
    oidc_login::append_query,
    sessions::Sessions,
    token::hash_refresh_token,
};

/// How long an app waits for the user to sign in and answer.
const BROWSER_LOGIN_TTL: Duration = Duration::minutes(10);
const MAX_DEVICE_NAME_CHARS: usize = 64;
/// Browser sign-ins one tenant may have waiting at once: far more than its people start in
/// ten minutes, so that only a flood of starts from many addresses reaches it.
const MAX_PENDING_BROWSER_LOGINS: u32 = 500;
/// How long an app is told to wait when its tenant has too many sign-ins waiting.
const FULL_RETRY_AFTER_SECS: u64 = 60;

/// The browser sign-ins of one tenant.
#[derive(Clone, Debug)]
pub(crate) struct BrowserLogin {
    db: Db,
    tenant: TenantId,
    codes: LoginCodes,
}

impl BrowserLogin {
    pub(crate) fn new(db: Db, tenant: TenantId) -> Self {
        Self {
            codes: LoginCodes::new(db.clone(), tenant),
            db,
            tenant,
        }
    }

    /// Remembers an app's sign-in and returns the web client page that asks the user about
    /// it. `redirect_uri` must be an app's address: the web client signs in on its own.
    pub(crate) async fn start(
        &self,
        redirect_uri: &str,
        code_challenge: &str,
        device_name: Option<&str>,
    ) -> Result<String, AuthError> {
        require_app_redirect(redirect_uri)?;
        if !is_valid_challenge(code_challenge) {
            return Err(AuthError::InvalidInput(
                "code_challenge must be an S256 challenge",
            ));
        }
        let flow = random_token();
        let login = db::BrowserLogin {
            client_redirect: redirect_uri.to_owned(),
            client_challenge: code_challenge.to_owned(),
            device_name: device_name.and_then(clean_device_name),
        };
        let inserted = self
            .db
            .insert_browser_login(
                self.tenant,
                &hash_refresh_token(&flow),
                &login,
                OffsetDateTime::now_utc() + BROWSER_LOGIN_TTL,
                MAX_PENDING_BROWSER_LOGINS,
            )
            .await?;
        if !inserted {
            return Err(AuthError::Busy {
                retry_after: FULL_RETRY_AFTER_SECS,
            });
        }
        Ok(append_query(paths::WEB_AUTHORIZE, ("flow", flow)))
    }

    /// What the user is asked about: which kind of app, on which device.
    pub(crate) async fn describe(&self, flow: &str) -> Result<BrowserLoginFlow, AuthError> {
        let login = self
            .db
            .browser_login(self.tenant, &hash_refresh_token(flow))
            .await?
            .ok_or(AuthError::LoginExpired)?;
        Ok(BrowserLoginFlow {
            app: native_app(&login.client_redirect).ok_or(AuthError::LoginExpired)?,
            device_name: login.device_name,
        })
    }

    /// `user_id`, signed in with `session_id`, lets the app in: returns its redirect with a
    /// one-time code. The app's session will say the user signed in as they did for
    /// `session_id`, so a click does not count as proving who they are.
    pub(crate) async fn approve(
        &self,
        sessions: &Sessions,
        flow: &str,
        user_id: UserId,
        session_id: Uuid,
    ) -> Result<String, AuthError> {
        sessions.require_may_sign_in(user_id).await?;
        let authentication = sessions.authentication(session_id, user_id).await?;
        let login = self.take(flow).await?;
        let code = self
            .codes
            .issue(user_id, &login.client_challenge, authentication)
            .await?;
        Ok(append_query(&login.client_redirect, ("code", code)))
    }

    /// The user turned the app away: returns its redirect with `error=access_denied`, so the
    /// app stops waiting.
    pub(crate) async fn deny(&self, flow: &str) -> Result<String, AuthError> {
        let login = self.take(flow).await?;
        Ok(append_query(
            &login.client_redirect,
            ("error", "access_denied"),
        ))
    }

    /// The user the app's `code` was issued to and how they signed in, when `code_verifier`
    /// answers its challenge.
    pub(crate) async fn exchange(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<(UserId, Authentication), AuthError> {
        self.codes.redeem(code, code_verifier).await
    }

    async fn take(&self, flow: &str) -> Result<db::BrowserLogin, AuthError> {
        self.db
            .take_browser_login(self.tenant, &hash_refresh_token(flow))
            .await?
            .ok_or(AuthError::LoginExpired)
    }
}

/// Refuses a `redirect_uri` that is not an app's address: only apps sign in in the browser.
pub(crate) fn require_app_redirect(redirect_uri: &str) -> Result<(), AuthError> {
    match native_app(redirect_uri) {
        Some(_) => Ok(()),
        None => Err(AuthError::InvalidInput(
            "redirect_uri must be a loopback or the mobile app address",
        )),
    }
}

/// The device name as it is shown: one line of at most [`MAX_DEVICE_NAME_CHARS`], or `None`
/// when nothing is left of it.
fn clean_device_name(name: &str) -> Option<String> {
    let line: String = name.chars().filter(|c| !c.is_control()).collect();
    let short: String = line.trim().chars().take(MAX_DEVICE_NAME_CHARS).collect();
    let short = short.trim_end();
    (!short.is_empty()).then(|| short.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_one_short_line() {
        assert_eq!(clean_device_name("  Laptop \n"), Some("Laptop".to_owned()));
        assert_eq!(
            clean_device_name("Pixel\u{0}\u{1b}[31m 9"),
            Some("Pixel[31m 9".to_owned())
        );
        assert_eq!(clean_device_name(" \t "), None);
        let long = "é".repeat(100);
        assert_eq!(
            clean_device_name(&long).map(|name| name.chars().count()),
            Some(MAX_DEVICE_NAME_CHARS)
        );
    }
}
