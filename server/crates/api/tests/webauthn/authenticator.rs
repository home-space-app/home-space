//! A software authenticator for the tests: makes ES256 credentials with `none` attestation and
//! answers challenges the way a browser and an authenticator would together, as WebAuthn JSON.
//!
//! It keeps the accounts it was registered with, so it can answer a passkey sign-in, which
//! names no credential and needs the user handle back.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const USER_PRESENT: u8 = 0x01;
const USER_VERIFIED: u8 = 0x04;
const BACKUP_ELIGIBLE: u8 = 0x08;
const BACKUP_STATE: u8 = 0x10;
const ATTESTED_DATA: u8 = 0x40;
const EXTENSION_DATA: u8 = 0x80;

struct Account {
    credential_id: Vec<u8>,
    rp_id: String,
    user_handle: Vec<u8>,
    key: SigningKey,
    counter: u32,
}

/// One authenticator, such as a phone or a YubiKey.
pub struct Authenticator {
    accounts: Vec<Account>,
    /// A passkey that syncs between devices (backup eligible).
    pub synced: bool,
    /// Checks a PIN or biometrics.
    pub verifies_user: bool,
    /// Makes credentials with `hmac-secret` (what a security key does when the browser asks for
    /// PRF), and says so in the authenticator data it signs.
    pub hmac_secret: bool,
}

impl Authenticator {
    pub fn new() -> Self {
        Self {
            accounts: Vec::new(),
            synced: false,
            verifies_user: true,
            hmac_secret: false,
        }
    }

    fn flags(&self) -> u8 {
        let mut flags = USER_PRESENT;
        if self.verifies_user {
            flags |= USER_VERIFIED;
        }
        if self.synced {
            flags |= BACKUP_ELIGIBLE | BACKUP_STATE;
        }
        flags
    }

    /// Answers `navigator.credentials.create` with `options` (the server's
    /// `CreationChallengeResponse`) on a page at `origin`.
    pub fn register(&mut self, origin: &str, options: &Value) -> Value {
        let public_key = &options["publicKey"];
        let rp_id = public_key["rp"]["id"].as_str().expect("rp id").to_owned();
        let user_handle = decode(&public_key["user"]["id"]);
        let mut credential_id = vec![0u8; 16];
        rand_bytes(&mut credential_id);
        let key = new_key();
        let point = key.verifying_key().to_sec1_point(false);
        let cose_key = cbor_map(&[
            (cbor_int(1), cbor_int(2)),
            (cbor_int(3), cbor_int(-7)),
            (cbor_int(-1), cbor_int(1)),
            (cbor_int(-2), cbor_bytes(point.x().expect("x"))),
            (cbor_int(-3), cbor_bytes(point.y().expect("y"))),
        ]);
        let mut auth_data = Sha256::digest(rp_id.as_bytes()).to_vec();
        let extension_flag = if self.hmac_secret { EXTENSION_DATA } else { 0 };
        auth_data.push(self.flags() | ATTESTED_DATA | extension_flag);
        auth_data.extend(0u32.to_be_bytes());
        auth_data.extend([0u8; 16]);
        auth_data.extend(
            u16::try_from(credential_id.len())
                .expect("short")
                .to_be_bytes(),
        );
        auth_data.extend(&credential_id);
        auth_data.extend(cose_key);
        if self.hmac_secret {
            // CBOR `true` is 0xf5.
            auth_data.extend(cbor_map(&[(cbor_text("hmac-secret"), vec![0xf5])]));
        }
        let attestation = cbor_map(&[
            (cbor_text("fmt"), cbor_text("none")),
            (cbor_text("attStmt"), cbor_map(&[])),
            (cbor_text("authData"), cbor_bytes(&auth_data)),
        ]);
        let client_data = client_data("webauthn.create", &public_key["challenge"], origin);
        let id = URL_SAFE_NO_PAD.encode(&credential_id);
        self.accounts.push(Account {
            credential_id,
            rp_id,
            user_handle,
            key,
            counter: 0,
        });
        json!({
            "id": id,
            "rawId": id,
            "type": "public-key",
            "response": {
                "attestationObject": URL_SAFE_NO_PAD.encode(attestation),
                "clientDataJSON": URL_SAFE_NO_PAD.encode(client_data),
            },
            "clientExtensionResults": {},
        })
    }

    /// Answers `navigator.credentials.get` with `options` (the server's
    /// `RequestChallengeResponse`) on a page at `origin`, with the newest account it has that
    /// the options allow; `None` when it has none.
    pub fn sign(&mut self, origin: &str, options: &Value) -> Option<Value> {
        let public_key = &options["publicKey"];
        let rp_id = public_key["rpId"].as_str().expect("rp id");
        let allowed: Vec<Vec<u8>> = public_key["allowCredentials"]
            .as_array()
            .map(|allowed| allowed.iter().map(|item| decode(&item["id"])).collect())
            .unwrap_or_default();
        let flags = self.flags();
        let account = self.accounts.iter_mut().rev().find(|account| {
            account.rp_id == rp_id
                && (allowed.is_empty() || allowed.contains(&account.credential_id))
        })?;
        account.counter += 1;
        let mut auth_data = Sha256::digest(rp_id.as_bytes()).to_vec();
        auth_data.push(flags);
        auth_data.extend(account.counter.to_be_bytes());
        let client_data = client_data("webauthn.get", &public_key["challenge"], origin);
        let mut signed = auth_data.clone();
        signed.extend(Sha256::digest(&client_data));
        let signature: Signature = account.key.sign(&signed);
        let id = URL_SAFE_NO_PAD.encode(&account.credential_id);
        Some(json!({
            "id": id,
            "rawId": id,
            "type": "public-key",
            "response": {
                "authenticatorData": URL_SAFE_NO_PAD.encode(auth_data),
                "clientDataJSON": URL_SAFE_NO_PAD.encode(client_data),
                "signature": URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes()),
                "userHandle": URL_SAFE_NO_PAD.encode(&account.user_handle),
            },
            "clientExtensionResults": {},
        }))
    }
}

fn client_data(kind: &str, challenge: &Value, origin: &str) -> Vec<u8> {
    json!({
        "type": kind,
        "challenge": challenge.as_str().expect("challenge"),
        "origin": origin,
        "crossOrigin": false,
    })
    .to_string()
    .into_bytes()
}

fn decode(value: &Value) -> Vec<u8> {
    URL_SAFE_NO_PAD
        .decode(value.as_str().expect("base64url").trim_end_matches('='))
        .expect("base64url")
}

fn rand_bytes(bytes: &mut [u8]) {
    rand::fill(bytes);
}

fn new_key() -> SigningKey {
    loop {
        let mut secret = [0u8; 32];
        rand_bytes(&mut secret);
        if let Ok(key) = SigningKey::from_slice(&secret) {
            return key;
        }
    }
}

/// A CBOR head: major type and argument.
fn cbor_head(major: u8, value: u64) -> Vec<u8> {
    let major = major << 5;
    match value {
        0..24 => vec![major | u8::try_from(value).expect("small")],
        24..256 => vec![major | 24, u8::try_from(value).expect("byte")],
        256..65536 => {
            let mut head = vec![major | 25];
            head.extend(u16::try_from(value).expect("short").to_be_bytes());
            head
        }
        _ => {
            let mut head = vec![major | 26];
            head.extend(u32::try_from(value).expect("int").to_be_bytes());
            head
        }
    }
}

fn cbor_int(value: i64) -> Vec<u8> {
    if value >= 0 {
        cbor_head(0, value.unsigned_abs())
    } else {
        cbor_head(1, (-1 - value).unsigned_abs())
    }
}

fn cbor_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = cbor_head(2, bytes.len() as u64);
    out.extend(bytes);
    out
}

fn cbor_text(text: &str) -> Vec<u8> {
    let mut out = cbor_head(3, text.len() as u64);
    out.extend(text.as_bytes());
    out
}

fn cbor_map(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut out = cbor_head(5, entries.len() as u64);
    for (key, value) in entries {
        out.extend(key);
        out.extend(value);
    }
    out
}
