//! Signing in with a passkey or security key: the second step after the password, a passkey
//! sign-in on its own, and confirming it is the user before a sensitive change.

use std::net::IpAddr;

use uuid::Uuid;
use webauthn_rs::prelude::{DiscoverableAuthentication, PublicKeyCredential};

use db::{PendingLogin, User, UserId};
use protocol::{AssertionChallenge, DeviceInfo, TokenPair, WebAuthnKind};

use super::{failed, key_method, random_id};
use crate::{
    AuthError,
    service::AuthService,
    sessions::{USER_METHOD, signed_in_now_with},
    throttle::{Counter, attempt_source},
    token::hash_refresh_token,
    webauthn::{RelyingParty, parse_credential, parse_state, state_json},
};

const SIGN_IN: &str = "sign_in";
const REAUTHENTICATE: &str = "reauthenticate";

impl AuthService {
    /// The challenge for a passkey or security key as the second step of the sign-in
    /// waiting with `login_token`, asked from the page of `party`. A new one replaces the last.
    pub async fn login_webauthn_options(
        &self,
        login_token: &str,
        party: &RelyingParty,
    ) -> Result<AssertionChallenge, AuthError> {
        let token_hash = hash_refresh_token(login_token);
        let pending = self
            .db
            .pending_login(self.tenant, &token_hash)
            .await?
            .ok_or(AuthError::LoginExpired)?;
        let (options, state) = self.assertion_for(pending.user_id, party).await?;
        self.db
            .set_pending_webauthn(&token_hash, party.origin(), &state)
            .await?;
        Ok(AssertionChallenge {
            challenge_id: None,
            options,
        })
    }

    /// Checks the answer to the challenge of the waiting sign-in `pending` of `user`: returns
    /// the method its session records.
    pub(in crate::service) async fn check_login_webauthn(
        &self,
        user: &User,
        pending: &PendingLogin,
        answer: &PublicKeyCredential,
    ) -> Result<&'static str, AuthError> {
        let (Some(state), Some(origin)) = (&pending.webauthn_state, &pending.webauthn_origin)
        else {
            return Err(AuthError::WebAuthnFailed);
        };
        self.check_assertion(user.id, origin, state, answer).await
    }

    /// The challenge for confirming it is the user of `session_id` with a passkey or security
    /// key, from the page of `party`.
    pub async fn reauth_webauthn_options(
        &self,
        user_id: UserId,
        session_id: Uuid,
        party: &RelyingParty,
    ) -> Result<AssertionChallenge, AuthError> {
        self.sessions.authentication(session_id, user_id).await?;
        let (options, state) = self.assertion_for(user_id, party).await?;
        let challenge_id = random_id();
        self.store_ceremony(challenge_id, Some(user_id), REAUTHENTICATE, party, &state)
            .await?;
        Ok(AssertionChallenge {
            challenge_id: Some(challenge_id),
            options,
        })
    }

    /// Checks the answer to a challenge of
    /// [`reauth_webauthn_options`](Self::reauth_webauthn_options).
    pub(in crate::service) async fn check_reauth_webauthn(
        &self,
        user_id: UserId,
        challenge_id: Uuid,
        answer: &PublicKeyCredential,
    ) -> Result<(), AuthError> {
        let ceremony = self
            .db
            .take_webauthn_ceremony(self.tenant, challenge_id, &[REAUTHENTICATE])
            .await?
            .filter(|ceremony| ceremony.user_id == Some(user_id))
            .ok_or(AuthError::WebAuthnFailed)?;
        self.check_assertion(user_id, &ceremony.origin, &ceremony.state, answer)
            .await
            .map(|_| ())
    }

    /// Starts a passkey sign-in from the page of `party`: the browser offers the passkeys it
    /// has for the site, and the answer says whose it is. Anyone may start one, and each
    /// leaves a row until it expires, so starts are counted against `address` like the other
    /// sign-ins started without an account.
    pub async fn passkey_start(
        &self,
        party: &RelyingParty,
        address: Option<IpAddr>,
    ) -> Result<AssertionChallenge, AuthError> {
        self.count_sign_in_start(address).await?;
        let (mut options, state) = party
            .webauthn()?
            .start_discoverable_authentication()
            .map_err(failed)?;
        // The page decides: a button asks at once, the username field's autofill quietly.
        options.mediation = None;
        let challenge_id = random_id();
        self.store_ceremony(challenge_id, None, SIGN_IN, party, &state_json(&state))
            .await?;
        Ok(AssertionChallenge {
            challenge_id: Some(challenge_id),
            options,
        })
    }

    /// Finishes a passkey sign-in sent from `address`, and starts a session on `device`.
    /// Failed answers count against the address like wrong passwords.
    pub async fn passkey_finish(
        &self,
        challenge_id: Uuid,
        answer: &PublicKeyCredential,
        device: Option<&DeviceInfo>,
        address: Option<IpAddr>,
    ) -> Result<TokenPair, AuthError> {
        let counters = address
            .map(|address| vec![Counter::address(attempt_source(address))])
            .unwrap_or_default();
        let attempt = self.throttle.admit(counters).await?;
        let checked = self.check_passkey(challenge_id, answer).await;
        let (user, methods) = attempt
            .settle(checked, |err| matches!(err, AuthError::WebAuthnFailed))
            .await?;
        tracing::info!(user = %user.id, "signed in with a passkey");
        self.sessions
            .start(user.id, device, &signed_in_now_with(&methods))
            .await
    }

    async fn check_passkey(
        &self,
        challenge_id: Uuid,
        answer: &PublicKeyCredential,
    ) -> Result<(User, Vec<&'static str>), AuthError> {
        let ceremony = self
            .db
            .take_webauthn_ceremony(self.tenant, challenge_id, &[SIGN_IN])
            .await?
            .ok_or(AuthError::LoginExpired)?;
        let party = RelyingParty::new(&ceremony.origin)?;
        let webauthn = party.webauthn()?;
        let state: DiscoverableAuthentication = parse_state(&ceremony.state)?;
        let (handle, credential_id) = webauthn
            .identify_discoverable_authentication(answer)
            .map_err(failed)?;
        let user_id = self
            .db
            .user_by_webauthn_handle(self.tenant, handle)
            .await?
            .ok_or(AuthError::WebAuthnFailed)?;
        let stored = self
            .db
            .webauthn_credential_by_id(self.tenant, credential_id)
            .await?
            .filter(|stored| {
                stored.user_id == user_id
                    && stored.kind == WebAuthnKind::Passkey.as_str()
                    && stored.rp_id == party.id()
            })
            .ok_or(AuthError::WebAuthnFailed)?;
        let credential = parse_credential(&stored.credential)?;
        let result = webauthn
            .finish_discoverable_authentication_v2(answer, state, &credential)
            .map_err(failed)?;
        self.record_use(&stored, credential, &result).await?;
        let mut methods = vec![key_method(&result)];
        if result.user_verified() {
            methods.push(USER_METHOD);
        }
        Ok((self.user(user_id).await?, methods))
    }
}
