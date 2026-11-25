//! Passkeys and security keys (WebAuthn): adding them to an account, and signing in with them.
//!
//! The challenges and the browser's answers are the WebAuthn JSON of `webauthn-rs-proto`,
//! which the web client hands to `navigator.credentials` as they are.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DeviceInfo;
pub use webauthn_rs_proto::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

/// The longest name of a passkey or security key, in characters.
pub const MAX_KEY_NAME_LEN: usize = 64;

/// What a credential can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebAuthnKind {
    /// Signs in on its own: the authenticator keeps the account and checks a PIN or biometrics
    /// (iCloud Keychain, Google Password Manager, Windows Hello, a YubiKey with a PIN).
    Passkey,
    /// Replaces the authenticator app's code after the password (a YubiKey without a PIN).
    SecurityKey,
}

impl WebAuthnKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passkey => "passkey",
            Self::SecurityKey => "security_key",
        }
    }

    /// The kind stored as `name`, from [`Self::as_str`].
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "passkey" => Some(Self::Passkey),
            "security_key" => Some(Self::SecurityKey),
            _ => None,
        }
    }

    /// The name a new credential gets until the user picks one.
    pub fn default_name(self) -> &'static str {
        match self {
            Self::Passkey => "Passkey",
            Self::SecurityKey => "Security key",
        }
    }
}

/// One of the user's passkeys and security keys, from `GET /me/webauthn`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebAuthnKey {
    pub id: Uuid,
    pub name: String,
    pub kind: WebAuthnKind,
    /// The site it was made for; browsers offer it only there, so one made for another address
    /// of the server does not work on this one.
    pub rp_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

/// `POST /me/webauthn/start`: begins adding a passkey or security key.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WebAuthnStart {
    pub kind: WebAuthnKind,
}

/// The challenge for `navigator.credentials.create`, from `POST /me/webauthn/start`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistrationChallenge {
    /// Sent back with the answer.
    pub challenge_id: Uuid,
    pub options: CreationChallengeResponse,
}

/// The challenge for `navigator.credentials.get`: from `POST /auth/passkey/start` and
/// `POST /me/reauthenticate/webauthn` with a `challenge_id`, from `POST /auth/login/webauthn`
/// without one (the sign-in's `login_token` names it).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssertionChallenge {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge_id: Option<Uuid>,
    pub options: RequestChallengeResponse,
}

/// `POST /me/webauthn/finish`: the browser's new credential.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebAuthnFinish {
    pub challenge_id: Uuid,
    pub credential: RegisterPublicKeyCredential,
    /// What the user calls it; "Passkey" or "Security key" when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The added credential, and the account's recovery codes when it was its first second
/// factor (shown only now).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebAuthnAdded {
    pub key: WebAuthnKey,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovery_codes: Vec<String>,
}

/// `PATCH /me/webauthn/{id}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameWebAuthnKey {
    pub name: String,
}

/// `POST /auth/login/webauthn`: the challenge for the second step of a sign-in.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginWebAuthnRequest {
    pub login_token: String,
}

/// A browser's answer to an [`AssertionChallenge`] with a `challenge_id`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebAuthnAnswer {
    pub challenge_id: Uuid,
    pub credential: PublicKeyCredential,
}

/// `POST /auth/passkey/finish`: signs in with a passkey, without username or password.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasskeyFinish {
    pub challenge_id: Uuid,
    pub credential: PublicKeyCredential,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
    /// As in [`crate::LoginRequest::cookie`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cookie: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_stored_by_name() {
        for kind in [WebAuthnKind::Passkey, WebAuthnKind::SecurityKey] {
            assert_eq!(WebAuthnKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).expect("json"),
                serde_json::Value::from(kind.as_str())
            );
        }
        assert_eq!(WebAuthnKind::parse("phone"), None);
    }
}
