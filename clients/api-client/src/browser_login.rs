//! Browser sign-in for apps (`/auth/browser/*`): the app opens the server's web page in the
//! system browser and gets a session through a one-time code and PKCE. The web client answers
//! the page's question with the calls that need a session.

use protocol::{
    BrowserLoginDecision, BrowserLoginExchangeRequest, BrowserLoginFlow, BrowserLoginRedirect, Me,
    TokenPair, paths,
};

use crate::{ApiError, Client, Pkce, client::decode};

impl Client {
    /// The URL that starts an app's browser sign-in: open it in the system browser. The server
    /// sends the browser to its web page, and once the user lets the app in, to `redirect_uri`
    /// with a one-time `code` (or an `error`), which
    /// [`browser_login_exchange`](Self::browser_login_exchange) turns into a session. The
    /// page shows the name of the client's device.
    pub fn browser_login_start_url(
        &self,
        redirect_uri: &str,
        pkce: &Pkce,
    ) -> Result<String, ApiError> {
        let mut params = vec![
            ("redirect_uri", redirect_uri),
            ("code_challenge", pkce.challenge()),
        ];
        if let Some(device) = self.device() {
            params.push(("device_name", device.name.as_str()));
        }
        reqwest::Url::parse_with_params(&self.url(paths::BROWSER_LOGIN_START), params)
            .map(String::from)
            .map_err(|err| ApiError::Transport(format!("invalid server address: {err}")))
    }

    /// Completes an app's browser sign-in with the `code` the server put in the redirect.
    pub async fn browser_login_exchange(&self, code: &str, pkce: &Pkce) -> Result<Me, ApiError> {
        let request = BrowserLoginExchangeRequest {
            code: code.to_owned(),
            code_verifier: pkce.verifier().to_owned(),
            device: self.device().cloned(),
        };
        let response = self
            .http()
            .post(self.url(paths::BROWSER_LOGIN_EXCHANGE))
            .json(&request)
            .send()
            .await?;
        let pair: TokenPair = decode(response).await?;
        self.store(pair);
        self.signed_in().await
    }

    /// The app waiting on `flow` for the signed-in user to let it in.
    pub async fn browser_login_flow(&self, flow: &str) -> Result<BrowserLoginFlow, ApiError> {
        self.send_json(|| self.http().get(self.url(&paths::browser_login_flow(flow))))
            .await
    }

    /// Lets the app waiting on `flow` sign in as the signed-in user; returns where to send the
    /// browser so the app gets its code.
    pub async fn browser_login_approve(&self, flow: &str) -> Result<String, ApiError> {
        self.decide(paths::BROWSER_LOGIN_APPROVE, flow).await
    }

    /// Turns away the app waiting on `flow`; returns where to send the browser so the app
    /// stops waiting.
    pub async fn browser_login_deny(&self, flow: &str) -> Result<String, ApiError> {
        self.decide(paths::BROWSER_LOGIN_DENY, flow).await
    }

    async fn decide(&self, path: &str, flow: &str) -> Result<String, ApiError> {
        let request = BrowserLoginDecision {
            flow: flow.to_owned(),
        };
        let answer: BrowserLoginRedirect = self
            .send_json(|| self.http().post(self.url(path)).json(&request))
            .await?;
        Ok(answer.redirect)
    }
}
