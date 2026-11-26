//! Passkeys and security keys (WebAuthn) as [`AuthService`] offers them: adding, renaming and
//! removing them, the second step of a sign-in with one, a passkey sign-in without a password,
//! and confirming it is the user with one.
//!
//! Each ceremony has two requests. The first stores webauthn-rs's state, with the page that
//! asked; the second takes it, so a challenge is answered once, and checks the answer against
//! that page.
//!
//! Signing in and confirming it is the user with a key are in `sign_in`.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use webauthn_rs::prelude::{
    AuthenticationResult, AuthenticatorAttachment, Credential, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse,
    SecurityKeyAuthentication, SecurityKeyRegistration,
};
use webauthn_rs_proto::ResidentKeyRequirement;

use db::{DbError, MAX_WEBAUTHN_CREDENTIALS, NewCeremony, NewCredential, StoredCredential, UserId};
use protocol::{RegistrationChallenge, WebAuthnAdded, WebAuthnKey, WebAuthnKind};

use super::AuthService;
use crate::{
    AuthError, recovery_codes,
    sessions::{HWK_METHOD, SWK_METHOD},
    webauthn::{
        RelyingParty, credential_json, parse_credential, parse_state, passkey, security_key,
        state_json,
    },
};

/// How long the browser and the user have to answer a challenge.
const CEREMONY_TTL: Duration = Duration::minutes(5);
const ADDING: [&str; 2] = ["add_passkey", "add_security_key"];

impl AuthService {
    /// The passkeys and security keys of `user_id`, oldest first.
    pub async fn webauthn_keys(&self, user_id: UserId) -> Result<Vec<WebAuthnKey>, AuthError> {
        let stored = self.db.webauthn_credentials(user_id).await?;
        Ok(stored.into_iter().filter_map(key_of).collect())
    }

    /// Starts adding a passkey or security key for the user of `session_id`, who must have
    /// signed in or confirmed it is them recently, from the page of `party`.
    pub async fn start_webauthn(
        &self,
        user_id: UserId,
        session_id: Uuid,
        kind: WebAuthnKind,
        party: &RelyingParty,
    ) -> Result<RegistrationChallenge, AuthError> {
        self.require_recent_sign_in(user_id, session_id).await?;
        let user = self.user(user_id).await?;
        if user.password_hash.is_none() {
            return Err(AuthError::NoPassword);
        }
        let existing = self.db.webauthn_credentials(user_id).await?;
        if existing.len() >= usize::try_from(MAX_WEBAUTHN_CREDENTIALS).unwrap_or(usize::MAX) {
            return Err(AuthError::TooManyKeys);
        }
        let handle = self.db.webauthn_handle(user_id, random_id()).await?;
        let exclude = existing.into_iter().map(|key| key.credential_id).collect();
        let webauthn = party.webauthn()?;
        let name = user.username.as_str();
        let (options, state) = match kind {
            WebAuthnKind::Passkey => {
                let (mut options, state) = webauthn
                    .start_passkey_registration(handle, name, name, Some(exclude))
                    .map_err(|_| AuthError::WebAuthnFailed)?;
                // A passkey signs in without a username, so the authenticator must keep the
                // account: also on a YubiKey, which would otherwise not.
                if let Some(selection) = options.public_key.authenticator_selection.as_mut() {
                    selection.resident_key = Some(ResidentKeyRequirement::Required);
                    selection.require_resident_key = true;
                }
                (options, state_json(&state))
            }
            WebAuthnKind::SecurityKey => {
                let (options, state) = webauthn
                    .start_securitykey_registration(
                        handle,
                        name,
                        name,
                        Some(exclude),
                        None,
                        Some(AuthenticatorAttachment::CrossPlatform),
                    )
                    .map_err(|_| AuthError::WebAuthnFailed)?;
                (options, state_json(&state))
            }
        };
        let challenge_id = random_id();
        let ceremony_kind = match kind {
            WebAuthnKind::Passkey => ADDING[0],
            WebAuthnKind::SecurityKey => ADDING[1],
        };
        self.store_ceremony(challenge_id, Some(user_id), ceremony_kind, party, &state)
            .await?;
        Ok(RegistrationChallenge {
            challenge_id,
            options,
        })
    }

    /// Finishes adding the passkey or security key the browser made for `challenge_id`,
    /// named `name` (or after its kind). Returns it, with the recovery codes when it is the
    /// account's first second factor.
    pub async fn finish_webauthn(
        &self,
        user_id: UserId,
        session_id: Uuid,
        challenge_id: Uuid,
        answer: &RegisterPublicKeyCredential,
        name: Option<&str>,
    ) -> Result<WebAuthnAdded, AuthError> {
        self.require_recent_sign_in(user_id, session_id).await?;
        let ceremony = self
            .db
            .take_webauthn_ceremony(self.tenant, challenge_id, &ADDING)
            .await?
            .filter(|ceremony| ceremony.user_id == Some(user_id))
            .ok_or(AuthError::LoginExpired)?;
        let party = RelyingParty::new(&ceremony.origin)?;
        let webauthn = party.webauthn()?;
        let (kind, credential): (WebAuthnKind, Credential) = if ceremony.kind == ADDING[0] {
            let state: PasskeyRegistration = parse_state(&ceremony.state)?;
            let made = webauthn
                .finish_passkey_registration(answer, &state)
                .map_err(failed)?;
            (WebAuthnKind::Passkey, made.into())
        } else {
            let state: SecurityKeyRegistration = parse_state(&ceremony.state)?;
            let made = webauthn
                .finish_securitykey_registration(answer, &state)
                .map_err(failed)?;
            (WebAuthnKind::SecurityKey, made.into())
        };
        let name = name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(kind.default_name());
        self.add_credential(user_id, kind, &party, &credential, name)
            .await
    }

    async fn add_credential(
        &self,
        user_id: UserId,
        kind: WebAuthnKind,
        party: &RelyingParty,
        credential: &Credential,
        name: &str,
    ) -> Result<WebAuthnAdded, AuthError> {
        let id = Uuid::now_v7();
        let json = credential_json(credential);
        let codes = recovery_codes::new_set();
        let hashes: Vec<Vec<u8>> = codes
            .iter()
            .map(|code| recovery_codes::hash(code))
            .collect();
        let stored = NewCredential {
            id,
            credential_id: &credential.cred_id,
            credential: &json,
            kind: kind.as_str(),
            rp_id: party.id(),
            name,
        };
        let first = match self
            .db
            .add_webauthn_credential(self.tenant, user_id, stored, &hashes)
            .await
        {
            Err(DbError::Conflict) => return Err(self.why_not_added(user_id).await),
            other => other?,
        };
        tracing::info!(user = %user_id, kind = kind.as_str(), "passkey or security key added");
        let key = self
            .webauthn_keys(user_id)
            .await?
            .into_iter()
            .find(|key| key.id == id)
            .ok_or(AuthError::UnknownKey)?;
        Ok(WebAuthnAdded {
            key,
            recovery_codes: if first { codes } else { Vec::new() },
        })
    }

    /// Why storing a new credential conflicted: the limit, or the authenticator already
    /// registered it.
    async fn why_not_added(&self, user_id: UserId) -> AuthError {
        match self.db.webauthn_credentials(user_id).await {
            Ok(keys)
                if keys.len()
                    >= usize::try_from(MAX_WEBAUTHN_CREDENTIALS).unwrap_or(usize::MAX) =>
            {
                AuthError::TooManyKeys
            }
            Ok(_) => AuthError::WebAuthnFailed,
            Err(err) => err.into(),
        }
    }

    /// Renames the user's passkey or security key `id`.
    pub async fn rename_webauthn(
        &self,
        user_id: UserId,
        id: Uuid,
        name: &str,
    ) -> Result<WebAuthnKey, AuthError> {
        match self
            .db
            .rename_webauthn_credential(user_id, id, name.trim())
            .await
        {
            Err(DbError::NotFound) => return Err(AuthError::UnknownKey),
            other => other?,
        }
        self.webauthn_keys(user_id)
            .await?
            .into_iter()
            .find(|key| key.id == id)
            .ok_or(AuthError::UnknownKey)
    }

    /// Removes the passkey or security key `id` of the user of `session_id`, who must have
    /// signed in or confirmed it is them recently; with the last second factor the recovery
    /// codes go too, and the vault unlocker it was. Refused while it is the last unlocker of
    /// protected folders. Their sessions stay.
    pub async fn remove_webauthn(
        &self,
        user_id: UserId,
        session_id: Uuid,
        id: Uuid,
    ) -> Result<(), AuthError> {
        self.require_recent_sign_in(user_id, session_id).await?;
        match self.db.remove_webauthn_credential(user_id, id).await {
            Err(DbError::NotFound) => return Err(AuthError::UnknownKey),
            Err(DbError::Conflict) => return Err(AuthError::LastUnlocker),
            other => other?,
        }
        tracing::info!(user = %user_id, "passkey or security key removed");
        Ok(())
    }

    /// The challenge for one of the user's credentials made for `party`, and its state.
    pub(super) async fn assertion_for(
        &self,
        user_id: UserId,
        party: &RelyingParty,
    ) -> Result<(RequestChallengeResponse, String), AuthError> {
        let keys: Vec<_> = self
            .db
            .webauthn_credentials(user_id)
            .await?
            .into_iter()
            .filter(|stored| stored.rp_id == party.id())
            .map(|stored| parse_credential(&stored.credential).map(security_key))
            .collect::<Result<_, _>>()?;
        if keys.is_empty() {
            return Err(AuthError::WebAuthnUnavailable(
                "None of this account's passkeys or security keys work on this address.",
            ));
        }
        let (mut options, state) = party
            .webauthn()?
            .start_securitykey_authentication(&keys)
            .map_err(failed)?;
        // Passkeys on the phone or computer may answer as well as a key in the USB port.
        options.public_key.hints = None;
        Ok((options, state_json(&state)))
    }

    /// Checks `answer` to the challenge `state` that `origin` was sent for one of the user's
    /// credentials: returns the method a session records for it.
    pub(super) async fn check_assertion(
        &self,
        user_id: UserId,
        origin: &str,
        state: &str,
        answer: &PublicKeyCredential,
    ) -> Result<&'static str, AuthError> {
        let party = RelyingParty::new(origin)?;
        let state: SecurityKeyAuthentication = parse_state(state)?;
        let result = party
            .webauthn()?
            .finish_securitykey_authentication(answer, &state)
            .map_err(failed)?;
        let stored = self
            .db
            .webauthn_credential_by_id(self.tenant, result.cred_id())
            .await?
            .filter(|stored| stored.user_id == user_id)
            .ok_or(AuthError::WebAuthnFailed)?;
        let credential = parse_credential(&stored.credential)?;
        self.record_use(&stored, credential, &result).await?;
        Ok(key_method(&result))
    }

    /// Records that `stored` was used, with its new counter and backup flags. Refused when
    /// the counter is not above the stored one by then: two answers raced, or the key was
    /// copied, and the library only compared it with the counter of when the challenge was
    /// made.
    pub(super) async fn record_use(
        &self,
        stored: &StoredCredential,
        credential: Credential,
        result: &AuthenticationResult,
    ) -> Result<(), AuthError> {
        let mut key = passkey(credential);
        let changed = key.update_credential(result) == Some(true);
        let json = changed.then(|| credential_json(&key.into()));
        let touched = self
            .db
            .touch_webauthn_credential(stored.id, json.as_deref(), result.counter())
            .await?;
        if !touched {
            tracing::warn!(
                user = %stored.user_id,
                key = %stored.id,
                "passkey or security key refused: its counter did not go up, so it may be copied"
            );
            return Err(AuthError::WebAuthnFailed);
        }
        Ok(())
    }

    pub(super) async fn store_ceremony(
        &self,
        id: Uuid,
        user: Option<UserId>,
        kind: &str,
        party: &RelyingParty,
        state: &str,
    ) -> Result<(), AuthError> {
        let ceremony = NewCeremony {
            id,
            user,
            kind,
            origin: party.origin(),
            state,
            expires_at: OffsetDateTime::now_utc() + CEREMONY_TTL,
        };
        self.db
            .insert_webauthn_ceremony(self.tenant, ceremony)
            .await?;
        Ok(())
    }
}

/// What a session records for a credential: `swk` for a passkey that syncs between devices,
/// `hwk` for one that stays on its key or device.
pub(super) fn key_method(result: &AuthenticationResult) -> &'static str {
    if result.backup_eligible() {
        SWK_METHOD
    } else {
        HWK_METHOD
    }
}

fn key_of(stored: StoredCredential) -> Option<WebAuthnKey> {
    let (prf, synced) = prf_and_synced(&stored.credential);
    Some(WebAuthnKey {
        id: stored.id,
        kind: WebAuthnKind::parse(&stored.kind)?,
        name: stored.name,
        rp_id: stored.rp_id,
        created_at: stored.created_at,
        last_used_at: stored.last_used_at,
        prf,
        synced,
    })
}

/// Whether a stored credential has `hmac-secret` and whether it syncs. Both come from the
/// authenticator data the key signed at registration (the browser asks for `prf`, which a
/// security key answers with `hmac-secret`; webauthn-rs keeps it as unsolicited), and the
/// backup flag as last seen. An unreadable credential has neither.
fn prf_and_synced(json: &str) -> (bool, bool) {
    use webauthn_rs_proto::ExtnState;
    match parse_credential(json) {
        Ok(credential) => (
            matches!(
                credential.extensions.hmac_create_secret,
                ExtnState::Set(true) | ExtnState::Unsolicited(true)
            ),
            credential.backup_eligible,
        ),
        Err(_) => (false, false),
    }
}

/// A wrong or unusable answer; the details stay in the log.
pub(super) fn failed(err: webauthn_rs::prelude::WebauthnError) -> AuthError {
    tracing::debug!(error = %err, "WebAuthn answer refused");
    AuthError::WebAuthnFailed
}

/// A random id for what a client must not guess: a challenge, a passkey's user handle.
pub(super) fn random_id() -> Uuid {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

mod sign_in;
