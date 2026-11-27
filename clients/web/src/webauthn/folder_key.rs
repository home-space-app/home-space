//! Folder keys: credentials the page makes on a security key for the vault alone, with
//! challenges of its own, so they never sign in. Their PRF secret wraps the vault key; the server
//! only keeps what asks the key again (`GET /vault`).
//!
//! `webauthn-rs-proto` cannot express PRF, so the options are built here by hand.

use std::fmt;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use js_sys::{Array, Function, Object, Reflect, Uint8Array};
use protocol::PRF_SALT_LEN;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CredentialCreationOptions, CredentialRequestOptions};
use zeroize::Zeroizing;

use super::{Failure, credentials, failure};

/// How long the browser waits for a folder key, in milliseconds.
const FOLDER_KEY_TIMEOUT_MS: u32 = 120_000;
/// Length of the challenges the page makes for folder keys; nobody checks them.
const CHALLENGE_LEN: usize = 32;
/// Length of the random user handle of a folder key: never the account's, so a key that keeps
/// the credential anyway cannot replace a sign-in passkey with it.
const USER_HANDLE_LEN: usize = 16;
/// Length of what a key's PRF gives.
const PRF_SECRET_LEN: usize = 32;
/// What the browser calls the site a folder key is for.
const RP_NAME: &str = "Home Space";
/// The signature algorithms a folder key may use (ES256, EdDSA, RS256). Nothing checks its
/// signatures, but WebAuthn asks for one.
const ALGORITHMS: [i32; 3] = [-7, -8, -257];
/// The authenticator data's flag of a credential its provider backs up and syncs.
const BACKUP_ELIGIBLE: u8 = 0x08;
/// Where the flags are in the authenticator data, after the hash of the RP id.
const FLAGS_AT: u32 = 32;
const SYNCED_KEY: &str =
    "This is a synced passkey, which the apps cannot reach. Use a security key, such as a YubiKey.";
const NO_PRF: &str = "This security key gave no secret for protected folders. It needs a PIN \
                      and PRF (hmac-secret); try another key or another browser.";

/// What a folder key's PRF gives for one salt; the vault key's wrapping key is derived from
/// it. Wiped when dropped.
pub struct PrfSecret(Zeroizing<[u8; PRF_SECRET_LEN]>);

impl PrfSecret {
    /// The secret's bytes, for deriving the wrapping key.
    pub fn expose(&self) -> &[u8; PRF_SECRET_LEN] {
        &self.0
    }
}

impl fmt::Debug for PrfSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrfSecret(…)")
    }
}

/// A new folder key: what `POST /vault/unlockers` keeps of it, and its PRF secret.
#[derive(Debug)]
pub struct NewFolderKey {
    /// The new credential's id.
    pub credential_id: Vec<u8>,
    /// The host of this page, which the credential is bound to.
    pub rp_id: String,
    /// The PRF secret for the salt it was made with.
    pub secret: PrfSecret,
}

/// A folder key to ask, as `GET /vault` lists it.
#[derive(Clone, Copy, Debug)]
pub struct FolderKeyRef<'a> {
    /// The credential id the key gave.
    pub credential_id: &'a [u8],
    /// The PRF input of its unlocker.
    pub prf_salt: &'a [u8; PRF_SALT_LEN],
}

/// Makes a folder key on a security key for this page's host, and gets its PRF secret for
/// `salt`: from the new credential when the browser gives it, else from one more assertion.
/// The credential is not discoverable and needs the key's PIN; `existing` are the credential
/// ids of the folder keys, which the key does not make again. `user_name` labels it on the
/// key. Must follow a click closely.
pub async fn create_folder_key(
    user_name: &str,
    salt: &[u8; PRF_SALT_LEN],
    existing: &[&[u8]],
) -> Result<NewFolderKey, Failure> {
    let rp_id = page_host()?;
    let options = creation_options(&rp_id, user_name, salt, existing).map_err(failure)?;
    let promise = credentials()?
        .create_with_options(&options)
        .map_err(failure)?;
    let made = JsFuture::from(promise).await.map_err(failure)?;
    let made = made
        .dyn_into::<web_sys::PublicKeyCredential>()
        .map_err(|_| Failure::Cancelled)?;
    if synced(&made) {
        return Err(Failure::Other(SYNCED_KEY.to_owned()));
    }
    let credential_id = Uint8Array::new(&made.raw_id()).to_vec();
    let secret = match prf_answer(&made).map_err(failure)? {
        PrfAnswer::Secret(secret) => secret,
        PrfAnswer::Missing => return Err(Failure::Other(NO_PRF.to_owned())),
        PrfAnswer::Enabled => {
            let key = FolderKeyRef {
                credential_id: &credential_id,
                prf_salt: salt,
            };
            folder_key_secret(&[key]).await?.1
        }
    };
    Ok(NewFolderKey {
        credential_id,
        rp_id,
        secret,
    })
}

/// Asks one of `keys`, all made for this page's host, for its PRF secret: PIN and touch. The
/// credential id of the key that answered, and its secret.
pub async fn folder_key_secret(keys: &[FolderKeyRef<'_>]) -> Result<(Vec<u8>, PrfSecret), Failure> {
    let options = request_options(&page_host()?, keys).map_err(failure)?;
    let promise = credentials()?.get_with_options(&options).map_err(failure)?;
    let answer = JsFuture::from(promise).await.map_err(failure)?;
    let answer = answer
        .dyn_into::<web_sys::PublicKeyCredential>()
        .map_err(|_| Failure::Cancelled)?;
    let credential_id = Uint8Array::new(&answer.raw_id()).to_vec();
    match prf_answer(&answer).map_err(failure)? {
        PrfAnswer::Secret(secret) => Ok((credential_id, secret)),
        PrfAnswer::Enabled | PrfAnswer::Missing => Err(Failure::Other(NO_PRF.to_owned())),
    }
}

/// `navigator.credentials.create` options of a folder key: a security key, not discoverable,
/// always with user verification, and PRF asked for `salt` at once.
fn creation_options(
    rp_id: &str,
    user_name: &str,
    salt: &[u8; PRF_SALT_LEN],
    existing: &[&[u8]],
) -> Result<CredentialCreationOptions, JsValue> {
    let algorithms = ALGORITHMS
        .iter()
        .map(|alg| object(&[("type", "public-key".into()), ("alg", (*alg).into())]))
        .collect::<Result<Array, _>>()?;
    let public_key = object(&[
        (
            "rp",
            object(&[("id", rp_id.into()), ("name", RP_NAME.into())])?.into(),
        ),
        (
            "user",
            object(&[
                ("id", bytes(&random(USER_HANDLE_LEN)?)),
                ("name", user_name.into()),
                ("displayName", user_name.into()),
            ])?
            .into(),
        ),
        ("challenge", bytes(&random(CHALLENGE_LEN)?)),
        ("pubKeyCredParams", algorithms.into()),
        (
            "authenticatorSelection",
            object(&[
                ("authenticatorAttachment", "cross-platform".into()),
                ("residentKey", "discouraged".into()),
                ("requireResidentKey", false.into()),
                ("userVerification", "required".into()),
            ])?
            .into(),
        ),
        ("excludeCredentials", descriptors(existing)?.into()),
        ("hints", Array::of1(&"security-key".into()).into()),
        ("attestation", "none".into()),
        ("timeout", FOLDER_KEY_TIMEOUT_MS.into()),
        (
            "extensions",
            object(&[("prf", object(&[("eval", prf_input(salt)?.into())])?.into())])?.into(),
        ),
    ])?;
    Ok(object(&[("publicKey", public_key.into())])?.unchecked_into())
}

/// `navigator.credentials.get` options that ask any of `keys` for its PRF secret, each with
/// its own salt.
fn request_options(
    rp_id: &str,
    keys: &[FolderKeyRef<'_>],
) -> Result<CredentialRequestOptions, JsValue> {
    let by_credential = Object::new();
    for key in keys {
        let id = URL_SAFE_NO_PAD.encode(key.credential_id);
        let input: JsValue = prf_input(key.prf_salt)?.into();
        Reflect::set(&by_credential, &id.into(), &input)?;
    }
    let ids: Vec<&[u8]> = keys.iter().map(|key| key.credential_id).collect();
    let public_key = object(&[
        ("challenge", bytes(&random(CHALLENGE_LEN)?)),
        ("rpId", rp_id.into()),
        ("allowCredentials", descriptors(&ids)?.into()),
        ("userVerification", "required".into()),
        ("hints", Array::of1(&"security-key".into()).into()),
        ("timeout", FOLDER_KEY_TIMEOUT_MS.into()),
        (
            "extensions",
            object(&[(
                "prf",
                object(&[("evalByCredential", by_credential.into())])?.into(),
            )])?
            .into(),
        ),
    ])?;
    Ok(object(&[("publicKey", public_key.into())])?.unchecked_into())
}

/// What a credential's PRF extension answered.
enum PrfAnswer {
    Secret(PrfSecret),
    /// PRF is on, but the secret takes an assertion, as most browsers make a credential.
    Enabled,
    /// No PRF: the browser or the key cannot.
    Missing,
}

fn prf_answer(credential: &web_sys::PublicKeyCredential) -> Result<PrfAnswer, JsValue> {
    let results = call(credential, "getClientExtensionResults")?;
    let prf = Reflect::get(&results, &"prf".into())?;
    if !prf.is_object() {
        return Ok(PrfAnswer::Missing);
    }
    let outputs = Reflect::get(&prf, &"results".into())?;
    let first = if outputs.is_object() {
        Reflect::get(&outputs, &"first".into())?
    } else {
        JsValue::UNDEFINED
    };
    if first.is_object() {
        let given = Uint8Array::new(&first);
        let mut secret = Zeroizing::new([0u8; PRF_SECRET_LEN]);
        let fits = usize::try_from(given.length()).is_ok_and(|len| len == PRF_SECRET_LEN);
        if fits {
            given.copy_to(secret.as_mut_slice());
        }
        given.fill(0, 0, given.length());
        return Ok(if fits {
            PrfAnswer::Secret(PrfSecret(secret))
        } else {
            PrfAnswer::Missing
        });
    }
    let enabled = Reflect::get(&prf, &"enabled".into())?;
    Ok(if enabled.as_bool() == Some(false) {
        PrfAnswer::Missing
    } else {
        PrfAnswer::Enabled
    })
}

/// Whether the new credential's provider backs it up, as synced passkeys are; a browser that
/// does not tell counts as not.
fn synced(credential: &web_sys::PublicKeyCredential) -> bool {
    Reflect::get(credential, &"response".into())
        .and_then(|response| call(&response, "getAuthenticatorData"))
        .ok()
        .filter(JsValue::is_object)
        .map(|data| Uint8Array::new(&data))
        .filter(|data| data.length() > FLAGS_AT)
        .is_some_and(|data| data.get_index(FLAGS_AT) & BACKUP_ELIGIBLE != 0)
}

/// `{ first: salt }`, a PRF input.
fn prf_input(salt: &[u8; PRF_SALT_LEN]) -> Result<Object, JsValue> {
    object(&[("first", bytes(salt))])
}

/// Credential descriptors of security keys for `ids`.
fn descriptors(ids: &[&[u8]]) -> Result<Array, JsValue> {
    ids.iter()
        .map(|id| {
            object(&[
                ("type", "public-key".into()),
                ("id", bytes(id)),
                (
                    "transports",
                    Array::of3(&"usb".into(), &"nfc".into(), &"ble".into()).into(),
                ),
            ])
        })
        .collect()
}

fn object(entries: &[(&str, JsValue)]) -> Result<Object, JsValue> {
    let object = Object::new();
    for (name, value) in entries {
        Reflect::set(&object, &(*name).into(), value)?;
    }
    Ok(object)
}

fn bytes(data: &[u8]) -> JsValue {
    Uint8Array::from(data).into()
}

fn random(len: usize) -> Result<Vec<u8>, JsValue> {
    let mut data = vec![0u8; len];
    web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .crypto()?
        .get_random_values_with_u8_array(&mut data)?;
    Ok(data)
}

fn call(target: &JsValue, method: &str) -> Result<JsValue, JsValue> {
    let method: Function = Reflect::get(target, &method.into())?.dyn_into()?;
    method.call0(target)
}

/// The host of this page: the RP id of every folder key made here.
fn page_host() -> Result<String, Failure> {
    web_sys::window()
        .and_then(|window| window.location().hostname().ok())
        .filter(|host| !host.is_empty())
        .ok_or_else(|| Failure::Other("This page has no host name for a security key.".to_owned()))
}
