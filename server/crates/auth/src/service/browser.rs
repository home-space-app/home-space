//! Browser sign-in for apps, as [`AuthService`] offers it; see [`BrowserLogin`].

use std::net::IpAddr;

use db::UserId;
use protocol::{BrowserLoginFlow, DeviceInfo, TokenPair};

use super::AuthService;
use crate::{
    AuthError,
    throttle::{Counter, attempt_source},
};

impl AuthService {
    /// Starts an app's browser sign-in: returns the web client page, relative to the server,
    /// that asks the signed-in user to let the app in.
    pub async fn browser_start(
        &self,
        redirect_uri: &str,
        code_challenge: &str,
        device_name: Option<&str>,
    ) -> Result<String, AuthError> {
        self.browser
            .start(redirect_uri, code_challenge, device_name)
            .await
    }

    /// The app waiting on `flow`, to show the user before they answer.
    pub async fn browser_flow(&self, flow: &str) -> Result<BrowserLoginFlow, AuthError> {
        self.browser.describe(flow).await
    }

    /// `user_id` lets the app waiting on `flow` sign in as them: returns the app's redirect,
    /// carrying the one-time code.
    pub async fn browser_approve(&self, flow: &str, user_id: UserId) -> Result<String, AuthError> {
        self.browser.approve(&self.sessions, flow, user_id).await
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
        let user_id = attempt
            .settle(redeemed, |err| matches!(err, AuthError::InvalidCredentials))
            .await?;
        self.sessions.start(user_id, device).await
    }
}
