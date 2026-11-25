//! Passkeys and security keys (feature [`PASSKEYS`](protocol::features::PASSKEYS)): the
//! signed-in user's own, the challenge of the second step of a sign-in, and a passkey sign-in.
//!
//! The challenges are for `navigator.credentials`, so only a browser can answer them, and the
//! server takes them only from its own pages.

use protocol::{
    AssertionChallenge, LoginWebAuthnRequest, Me, PasskeyFinish, PublicKeyCredential,
    RegisterPublicKeyCredential, RegistrationChallenge, RenameWebAuthnKey, TokenPair,
    WebAuthnAdded, WebAuthnFinish, WebAuthnKey, WebAuthnKind, WebAuthnStart, paths,
};
use uuid::Uuid;

use crate::{
    ApiError, Client,
    client::{decode_sign_in, expect_success},
};

impl Client {
    /// The signed-in user's passkeys and security keys.
    pub async fn webauthn_keys(&self) -> Result<Vec<WebAuthnKey>, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::ME_WEBAUTHN)))
            .await
    }

    /// The challenge for a new passkey or security key, for `navigator.credentials.create`.
    /// Needs a recent sign-in ([`ApiError::ReauthenticationRequired`]).
    pub async fn start_webauthn(
        &self,
        kind: WebAuthnKind,
    ) -> Result<RegistrationChallenge, ApiError> {
        let request = WebAuthnStart { kind };
        self.send_json(|| {
            self.http()
                .post(self.url(paths::ME_WEBAUTHN_START))
                .json(&request)
        })
        .await
    }

    /// Keeps the browser's new credential, named `name` (or after its kind); the answer has
    /// the recovery codes when it is the account's first second factor.
    pub async fn finish_webauthn(
        &self,
        challenge_id: Uuid,
        credential: &RegisterPublicKeyCredential,
        name: Option<&str>,
    ) -> Result<WebAuthnAdded, ApiError> {
        let request = WebAuthnFinish {
            challenge_id,
            credential: credential.clone(),
            name: name.map(str::to_owned),
        };
        self.send_json(|| {
            self.http()
                .post(self.url(paths::ME_WEBAUTHN_FINISH))
                .json(&request)
        })
        .await
    }

    pub async fn rename_webauthn(&self, id: Uuid, name: &str) -> Result<WebAuthnKey, ApiError> {
        let request = RenameWebAuthnKey {
            name: name.to_owned(),
        };
        self.send_json(|| {
            self.http()
                .patch(self.url(&paths::me_webauthn_key(id)))
                .json(&request)
        })
        .await
    }

    /// Removes a passkey or security key; with the last second factor the recovery codes go
    /// too. Needs a recent sign-in.
    pub async fn remove_webauthn(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(&paths::me_webauthn_key(id))))
            .await?;
        expect_success(response).await
    }

    /// The challenge for confirming it is the user with a passkey or security key, answered
    /// through [`Confirmation::WebAuthn`](crate::Confirmation::WebAuthn).
    pub async fn reauth_webauthn_options(&self) -> Result<AssertionChallenge, ApiError> {
        self.send_json(|| {
            self.http()
                .post(self.url(paths::ME_REAUTHENTICATE_WEBAUTHN))
        })
        .await
    }

    /// The challenge for a passkey or security key as the second step of a sign-in, answered
    /// through [`SecondFactor::WebAuthn`](crate::SecondFactor::WebAuthn).
    pub async fn login_webauthn_options(
        &self,
        login_token: &str,
    ) -> Result<AssertionChallenge, ApiError> {
        let request = LoginWebAuthnRequest {
            login_token: login_token.to_owned(),
        };
        let response = self
            .http()
            .post(self.url(paths::LOGIN_WEBAUTHN))
            .json(&request)
            .send()
            .await?;
        decode_sign_in(response).await
    }

    /// The challenge for signing in with a passkey.
    pub async fn passkey_start(&self) -> Result<AssertionChallenge, ApiError> {
        let response = self
            .http()
            .post(self.url(paths::PASSKEY_START))
            .send()
            .await?;
        decode_sign_in(response).await
    }

    /// Signs in with a passkey's answer to the challenge `challenge_id` of
    /// [`passkey_start`](Self::passkey_start).
    pub async fn passkey_finish(
        &self,
        challenge_id: Uuid,
        credential: &PublicKeyCredential,
    ) -> Result<Me, ApiError> {
        let request = PasskeyFinish {
            challenge_id,
            credential: credential.clone(),
            device: self.device().cloned(),
            cookie: self.cookie_session(),
        };
        let response = self
            .http()
            .post(self.url(paths::PASSKEY_FINISH))
            .json(&request)
            .send()
            .await?;
        let pair: TokenPair = decode_sign_in(response).await?;
        self.store(pair);
        self.signed_in().await
    }
}
