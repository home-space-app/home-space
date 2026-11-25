//! Browser sign-in for apps, as [`AuthService`] offers it; see [`BrowserLogin`].

use std::net::IpAddr;

use uuid::Uuid;

use db::UserId;
use protocol::{BrowserLoginFlow, DeviceInfo, TokenPair};

use super::AuthService;
use crate::{
    AuthError,
    browser_login::require_app_redirect,
    throttle::{Counter, attempt_source},
};

impl AuthService {
    /// Starts an app's browser sign-in: returns the web client page, relative to the server,
    /// that asks the signed-in user to let the app in. Anyone may start one, so starts are
    /// counted against `address`, and refused once it started too many.
    ///
    /// A tenant that signs in only with its provider has nothing to ask on its page, so the
    /// browser goes straight to the provider, which sends the code to the app as single
    /// sign-on does; the app trades it at [`browser_exchange`](Self::browser_exchange) all the
    /// same. Passkeys are a sign-in of their own, though: when one of the tenant's passkeys
    /// works at `host`, the name the request was sent to, the page offers it beside the
    /// provider.
    pub async fn browser_start(
        &self,
        redirect_uri: &str,
        code_challenge: &str,
        device_name: Option<&str>,
        host: Option<&str>,
        address: Option<IpAddr>,
    ) -> Result<String, AuthError> {
        self.count_sign_in_start(address).await?;
        if let (false, Some(oidc)) = (self.password_login, &self.oidc)
            && !self.passkeys_at(host).await?
        {
            require_app_redirect(redirect_uri)?;
            return oidc.start(redirect_uri, code_challenge).await;
        }
        self.browser
            .start(redirect_uri, code_challenge, device_name)
            .await
    }

    /// Whether one of the tenant's passkeys was made for `host`, so it can sign in there.
    async fn passkeys_at(&self, host: Option<&str>) -> Result<bool, AuthError> {
        let Some(host) = host else {
            return Ok(false);
        };
        let rp_id = host.to_ascii_lowercase();
        Ok(self.db.tenant_has_passkeys(self.tenant, &rp_id).await?)
    }

    /// Counts a sign-in started from `address`, in the browser or with single sign-on: each
    /// leaves a row until it expires, so one address may only start so many.
    pub(super) async fn count_sign_in_start(
        &self,
        address: Option<IpAddr>,
    ) -> Result<(), AuthError> {
        let Some(address) = address else {
            return Ok(());
        };
        let counters = vec![Counter::sign_in_starts(attempt_source(address))];
        self.throttle.admit(counters).await?.keep();
        Ok(())
    }

    /// The app waiting on `flow`, to show the user before they answer.
    pub async fn browser_flow(&self, flow: &str) -> Result<BrowserLoginFlow, AuthError> {
        self.browser.describe(flow).await
    }

    /// `user_id`, signed in with `session_id`, lets the app waiting on `flow` sign in as them:
    /// returns the app's redirect, carrying the one-time code. The app's session says they
    /// signed in as they did for `session_id`.
    pub async fn browser_approve(
        &self,
        flow: &str,
        user_id: UserId,
        session_id: Uuid,
    ) -> Result<String, AuthError> {
        self.browser
            .approve(&self.sessions, flow, user_id, session_id)
            .await
    }

    /// The user turned away the app waiting on `flow`: returns the app's redirect, carrying
    /// the refusal.
    pub async fn browser_deny(&self, flow: &str) -> Result<String, AuthError> {
        self.browser.deny(flow).await
    }

    /// Trades the app's one-time code for a session on `device`. A wrong code counts against
    /// the address like a wrong password, though at 256 bits it cannot be guessed anyway.
    pub async fn browser_exchange(
        &self,
        code: &str,
        code_verifier: &str,
        device: Option<&DeviceInfo>,
        address: Option<IpAddr>,
    ) -> Result<TokenPair, AuthError> {
        let counters = address
            .map(|address| vec![Counter::address(attempt_source(address))])
            .unwrap_or_default();
        let attempt = self.throttle.admit(counters).await?;
        let redeemed = self.browser.exchange(code, code_verifier).await;
        let (user_id, authentication) = attempt
            .settle(redeemed, |err| matches!(err, AuthError::LoginCodeInvalid))
            .await?;
        self.sessions.start(user_id, device, &authentication).await
    }
}
