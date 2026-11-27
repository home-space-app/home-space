//! Passkeys and security keys through `navigator.credentials`: making a credential for a
//! challenge of the server, and answering one with it. Folder keys, which never sign in, are
//! in [`folder_key`].
//!
//! The server's challenges and the browser's answers convert through `webauthn-rs-proto`, which
//! turns the base64url fields into the buffers the browser wants and back.

use js_sys::{Function, Promise, Reflect};
use protocol::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortSignal, CredentialCreationOptions, CredentialRequestOptions};

#[expect(
    dead_code,
    reason = "enabling a folder key also wraps the vault key, which waits for vault-core to take \
              a PRF secret from the browser"
)]
pub mod folder_key;

const PUBLIC_KEY_CREDENTIAL: &str = "PublicKeyCredential";
const CONDITIONAL_CHECK: &str = "isConditionalMediationAvailable";
/// Why the browser gave no credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The user closed the browser's dialog, it timed out, or the page stopped asking.
    Cancelled,
    /// The authenticator already holds a credential of this account.
    AlreadyAdded,
    /// Anything else, said for the user.
    Other(String),
}

impl Failure {
    /// What the page shows; `None` when the user cancelled and nothing needs saying.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Cancelled => None,
            Self::AlreadyAdded => {
                Some("This passkey or security key is already added to your account.".to_owned())
            }
            Self::Other(message) => Some(message.clone()),
        }
    }
}

/// Whether this browser can use passkeys here: it has WebAuthn and the page is a secure
/// context (https, or localhost).
pub fn available() -> bool {
    web_sys::window().is_some_and(|window| {
        window.is_secure_context()
            && Reflect::get(&window, &PUBLIC_KEY_CREDENTIAL.into())
                .is_ok_and(|constructor| constructor.is_function())
    })
}

/// Whether the browser offers passkeys in the username field's autofill.
pub async fn conditional_available() -> bool {
    let Some(window) = web_sys::window().filter(|_| available()) else {
        return false;
    };
    let check = Reflect::get(&window, &PUBLIC_KEY_CREDENTIAL.into()).and_then(|constructor| {
        Ok((
            Reflect::get(&constructor, &CONDITIONAL_CHECK.into())?,
            constructor,
        ))
    });
    let Ok((check, constructor)) = check else {
        return false;
    };
    let Some(check) = check.dyn_ref::<Function>() else {
        return false;
    };
    let Ok(promise) = check
        .call0(&constructor)
        .and_then(|value| value.dyn_into::<Promise>())
    else {
        return false;
    };
    JsFuture::from(promise)
        .await
        .is_ok_and(|available| available.is_truthy())
}

/// Makes a new credential for `challenge`; must follow a click closely.
pub async fn create(
    challenge: CreationChallengeResponse,
) -> Result<RegisterPublicKeyCredential, Failure> {
    let options = CredentialCreationOptions::from(challenge);
    let promise = credentials()?
        .create_with_options(&options)
        .map_err(failure)?;
    let made = JsFuture::from(promise).await.map_err(failure)?;
    let credential = made
        .dyn_into::<web_sys::PublicKeyCredential>()
        .map_err(|_| Failure::Cancelled)?;
    Ok(RegisterPublicKeyCredential::from(credential))
}

/// Answers `challenge` with one of the user's credentials. With `quietly`, the browser offers
/// them in the username field's autofill instead of a dialog, until the user picks one or
/// `quietly` is aborted.
pub async fn get(
    challenge: RequestChallengeResponse,
    quietly: Option<&AbortSignal>,
) -> Result<PublicKeyCredential, Failure> {
    let options = CredentialRequestOptions::from(challenge);
    if let Some(signal) = quietly {
        let set = Reflect::set(&options, &"mediation".into(), &"conditional".into())
            .and_then(|_| Reflect::set(&options, &"signal".into(), signal));
        set.map_err(failure)?;
    }
    let promise = credentials()?.get_with_options(&options).map_err(failure)?;
    let answer = JsFuture::from(promise).await.map_err(failure)?;
    let credential = answer
        .dyn_into::<web_sys::PublicKeyCredential>()
        .map_err(|_| Failure::Cancelled)?;
    Ok(PublicKeyCredential::from(credential))
}

fn credentials() -> Result<web_sys::CredentialsContainer, Failure> {
    web_sys::window()
        .filter(|_| available())
        .map(|window| window.navigator().credentials())
        .ok_or_else(|| Failure::Other("This browser cannot use passkeys here.".to_owned()))
}

fn failure(error: JsValue) -> Failure {
    let name = Reflect::get(&error, &"name".into())
        .ok()
        .and_then(|name| name.as_string())
        .unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "AbortError" => Failure::Cancelled,
        "InvalidStateError" => Failure::AlreadyAdded,
        _ => {
            let message = Reflect::get(&error, &"message".into())
                .ok()
                .and_then(|message| message.as_string())
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "The passkey or security key did not answer.".to_owned());
            Failure::Other(message)
        }
    }
}
