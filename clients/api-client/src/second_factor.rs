//! Two-factor sign-in (feature [`TWO_FACTOR`](protocol::features::TWO_FACTOR)): a sign-in in
//! two steps, and the signed-in user's authenticator app and recovery codes.

use protocol::{
    AuthMethod, ErrorCode, LoginRequest, LoginVerifyRequest, Me, Problem, ProblemType,
    PublicKeyCredential, RecoveryCodes, TokenPair, TotpConfirmRequest, TotpEnrolment, UserId,
    paths,
};
use reqwest::StatusCode;

use crate::{
    ApiError, Client,
    client::{decode_sign_in, expect_success, sign_in_error_from},
};

/// How a sign-in with the password went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignIn {
    /// Signed in.
    Done(Me),
    /// The password was right; the account also needs a second factor, which
    /// [`login_verify`](Client::login_verify) finishes the sign-in with.
    SecondFactor(SecondFactorStep),
}

/// The second step a sign-in waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecondFactorStep {
    /// What [`login_verify`](Client::login_verify) takes; valid for a few minutes.
    pub login_token: String,
    /// What the account can finish with: an authenticator app, a passkey or security key, and
    /// recovery codes.
    pub methods: Vec<AuthMethod>,
}

/// What finishes a sign-in that waits for its second factor.
#[derive(Clone, Debug)]
pub enum SecondFactor<'a> {
    /// A code from the authenticator app.
    Code(&'a str),
    /// One of the account's recovery codes, used up by this.
    RecoveryCode(&'a str),
    /// A passkey's or security key's answer to the challenge of
    /// [`login_webauthn_options`](Client::login_webauthn_options) (feature
    /// [`PASSKEYS`](protocol::features::PASSKEYS)).
    WebAuthn(Box<PublicKeyCredential>),
}

impl Client {
    /// Signs in with a username and password, telling the server that this client can ask
    /// for a second factor. Older servers never ask for one.
    pub async fn login_two_step(&self, username: &str, password: &str) -> Result<SignIn, ApiError> {
        let request = LoginRequest {
            username: username.to_owned(),
            password: password.to_owned(),
            device: self.device().cloned(),
            cookie: self.cookie_session(),
            two_step: true,
        };
        let response = self
            .http()
            .post(self.url(paths::LOGIN))
            .json(&request)
            .send()
            .await?;
        if response.status() == StatusCode::FORBIDDEN {
            let mut problem: Problem = response.json().await?;
            if problem.kind() == Some(ProblemType::SecondFactorRequired)
                && let Some(login_token) = problem.login_token.take()
            {
                return Ok(SignIn::SecondFactor(SecondFactorStep {
                    login_token,
                    methods: problem.methods,
                }));
            }
            return Err(ApiError::Api {
                code: problem.code().unwrap_or(ErrorCode::Forbidden),
                message: problem.detail().to_owned(),
            });
        }
        if !response.status().is_success() {
            return Err(sign_in_error_from(response).await);
        }
        let pair: TokenPair = response.json().await?;
        self.store(pair);
        Ok(SignIn::Done(self.signed_in().await?))
    }

    /// Finishes a sign-in that waits for its second factor. A wrong code is an
    /// `Api { code: BadRequest }`, after which the same `login_token` can be tried again; one
    /// that expired or had too many wrong codes is `Api { code: Gone }`.
    pub async fn login_verify(
        &self,
        login_token: &str,
        factor: SecondFactor<'_>,
    ) -> Result<Me, ApiError> {
        let (code, recovery_code, webauthn) = match factor {
            SecondFactor::Code(code) => (Some(code.to_owned()), None, None),
            SecondFactor::RecoveryCode(code) => (None, Some(code.to_owned()), None),
            SecondFactor::WebAuthn(answer) => (None, None, Some(*answer)),
        };
        let request = LoginVerifyRequest {
            login_token: login_token.to_owned(),
            code,
            recovery_code,
            webauthn,
            device: self.device().cloned(),
            cookie: self.cookie_session(),
        };
        let response = self
            .http()
            .post(self.url(paths::LOGIN_VERIFY))
            .json(&request)
            .send()
            .await?;
        let pair: TokenPair = decode_sign_in(response).await?;
        self.store(pair);
        self.signed_in().await
    }

    /// Starts adding an authenticator app: the secret to show as a QR code and as text. It
    /// counts once [`confirm_totp`](Self::confirm_totp) gets a code from it. Needs a recent
    /// sign-in ([`ApiError::ReauthenticationRequired`]).
    pub async fn start_totp(&self) -> Result<TotpEnrolment, ApiError> {
        self.send_json(|| self.http().post(self.url(paths::ME_TOTP)))
            .await
    }

    /// Confirms the new authenticator app with its first `code`; returns the recovery codes,
    /// which the user sees only now. They are empty when the account had another second
    /// factor already, and keeps its codes.
    pub async fn confirm_totp(&self, code: &str) -> Result<RecoveryCodes, ApiError> {
        let request = TotpConfirmRequest {
            code: code.to_owned(),
        };
        self.send_json(|| {
            self.http()
                .post(self.url(paths::ME_TOTP_CONFIRM))
                .json(&request)
        })
        .await
    }

    /// Removes the authenticator app, and the recovery codes with the last second factor; this
    /// session stays signed in.
    pub async fn remove_totp(&self) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(paths::ME_TOTP)))
            .await?;
        expect_success(response).await
    }

    /// A new set of recovery codes, voiding the old ones.
    pub async fn new_recovery_codes(&self) -> Result<RecoveryCodes, ApiError> {
        self.send_json(|| self.http().post(self.url(paths::ME_RECOVERY_CODES)))
            .await
    }

    /// An administrator removes `user`'s second factors and ends their sessions.
    pub async fn reset_second_factors(&self, user: UserId) -> Result<(), ApiError> {
        let response = self
            .send(|| {
                self.http()
                    .delete(self.url(&paths::admin_user_second_factors(user)))
            })
            .await?;
        expect_success(response).await
    }
}
