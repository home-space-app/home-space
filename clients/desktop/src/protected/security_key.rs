//! Security keys over USB HID: CTAP2 `hmac-secret` with the key's PIN, for security-key
//! folders (`docs/todo/protected-folders.md`).
//!
//! Every call blocks until the key answers, and a touch can take half a minute, so the interface
//! calls them from `spawn_blocking`. The PIN is asked by the app's own dialog and sent to the key
//! for each request; nothing the key gives is kept.
//!
//! Windows lets only administrators open FIDO devices; it needs the Windows WebAuthn API, which
//! this app does not use yet, so there every call says so.

use client_core::protected::FolderKey;
use vault_core::{Authenticator, CredentialRef, PrfOutput};
use zeroize::Zeroizing;

/// What went wrong with a security key. The texts are for the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("No security key found. Plug it in and try again.")]
    NoKey,
    #[error("Several security keys are plugged in. Leave only the one to add.")]
    SeveralKeys,
    #[error("This security key is not one of your folder keys.")]
    NotEnabled,
    #[error("This security key is one of your folder keys already.")]
    AlreadyEnabled,
    #[error(
        "This security key has no PIN. Set one first, for example with Yubico Authenticator, then try again."
    )]
    NoPin,
    #[error("Wrong PIN.")]
    WrongPin,
    #[error("Wrong PIN. {0} tries left before the key blocks its PIN.")]
    WrongPinTriesLeft(u8),
    #[error(
        "The PIN of this security key is blocked after too many wrong tries. Only resetting the key unblocks it, which deletes its folder keys."
    )]
    PinBlocked,
    #[error("Too many wrong PINs in a row. Unplug the key, plug it in again and retry.")]
    PinPaused,
    #[error("The security key was not touched in time.")]
    NotTouched,
    #[error("This security key cannot protect folders: it lacks the hmac-secret feature.")]
    NoHmacSecret,
    #[error("Plug in “{0}” too: every key that stays is needed to replace the vault key.")]
    Missing(String),
    #[error("No security key was plugged in in time.")]
    NotPluggedIn,
    #[error("The security key refused.")]
    Refused,
    #[error("This is a passkey that syncs, not a security key. Use a hardware security key.")]
    Synced,
    #[error(
        "Security keys work in this app on Linux and macOS only, for now. Open this folder on one of them or in the mobile app."
    )]
    #[cfg_attr(not(windows), allow(dead_code))]
    Unsupported,
    #[error("The security key did not answer: {0}")]
    Device(String),
}

/// A security key plugged into this computer, with the PIN the user typed for it.
pub struct SecurityKey {
    #[cfg(not(windows))]
    device: std::sync::Arc<ctap_hid_fido2::FidoKeyHid>,
    path: String,
    pin: Zeroizing<String>,
}

impl std::fmt::Debug for SecurityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityKey")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SecurityKey {
    /// Where the key is plugged in: it is unplugged once [`plugged`] no longer lists it.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Authenticator for SecurityKey {
    fn prf(
        &mut self,
        credential: CredentialRef<'_>,
        input: &[u8; 32],
    ) -> vault_core::Result<PrfOutput> {
        self.hmac_secret(credential, input)
            .map_err(|err| vault_core::Error::Authenticator(Box::new(err)))
    }
}

/// Several plugged-in keys at once, each opened with its own PIN, answering for the folder keys
/// they hold: what replacing the vault key asks of every key that stays.
#[derive(Debug, Default)]
pub struct KeyRing {
    keys: Vec<(SecurityKey, Vec<u8>)>,
}

impl KeyRing {
    /// Finds the key that holds each of `wanted`, without a touch; one not plugged in is
    /// [`KeyError::Missing`].
    pub fn gather(wanted: &[(FolderKey, Zeroizing<String>)]) -> Result<Self, KeyError> {
        let mut keys = Vec::with_capacity(wanted.len());
        for (key, pin) in wanted {
            let (found, _) =
                SecurityKey::holding(std::slice::from_ref(key), pin.clone()).map_err(|err| {
                    match err {
                        KeyError::NotEnabled | KeyError::NoKey => {
                            KeyError::Missing(key.name.clone())
                        }
                        other => other,
                    }
                })?;
            keys.push((found, key.credential_id.clone()));
        }
        Ok(Self { keys })
    }
}

impl Authenticator for KeyRing {
    fn prf(
        &mut self,
        credential: CredentialRef<'_>,
        input: &[u8; 32],
    ) -> vault_core::Result<PrfOutput> {
        match self
            .keys
            .iter_mut()
            .find(|(_, id)| id.as_slice() == credential.credential_id)
        {
            Some((key, _)) => key.prf(credential, input),
            None => Err(vault_core::Error::Authenticator(Box::new(
                KeyError::NotEnabled,
            ))),
        }
    }
}

/// A random client data hash: the server takes no part in folder key ceremonies, so nothing
/// checks it.
#[cfg(not(windows))]
fn challenge() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    bytes
}

#[cfg(not(windows))]
mod hid {
    use std::{
        fmt::Display,
        sync::{
            Arc, Mutex, Weak,
            atomic::{AtomicU64, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use ctap_hid_fido2::{
        FidoKeyHid, HidParam, LibCfg,
        fidokey::{
            AssertionExtension, CredentialExtension, GetAssertionArgsBuilder,
            MakeCredentialArgsBuilder,
        },
    };
    use vault_core::{CredentialRef, PrfOutput, kdf::prf_salt_input};
    use zeroize::Zeroizing;

    use super::{FolderKey, KeyError, SecurityKey, challenge};

    /// The HID usage page and usage of FIDO devices.
    const FIDO_USAGE_PAGE: u16 = 0xF1D0;
    const FIDO_USAGE: u16 = 0x01;
    /// The `BE` (backup eligible) flag of authenticator data: a passkey that syncs.
    const BACKUP_ELIGIBLE: u8 = 0x08;
    /// Where the flags are in authenticator data, after the RP id hash.
    const FLAGS_AT: usize = 32;

    /// CTAP2 status codes. The library gives them only inside its error texts.
    mod status {
        pub const OPERATION_DENIED: u8 = 0x27;
        pub const KEEPALIVE_CANCEL: u8 = 0x2D;
        pub const NO_CREDENTIALS: u8 = 0x2E;
        pub const USER_ACTION_TIMEOUT: u8 = 0x2F;
        pub const PIN_INVALID: u8 = 0x31;
        pub const PIN_BLOCKED: u8 = 0x32;
        pub const PIN_AUTH_BLOCKED: u8 = 0x34;
        pub const PIN_NOT_SET: u8 = 0x35;
        pub const PIN_REQUIRED: u8 = 0x36;
        /// `CTAP2_ERR_ACTION_TIMEOUT`, which some keys send instead of the user action timeout.
        pub const ACTION_TIMEOUT: u8 = 0x3A;
    }

    /// How long a step waits for the user to plug in the key it needs, and how often it looks.
    const PLUG_IN_TIME: Duration = Duration::from_secs(120);
    const LOOK_EVERY: Duration = Duration::from_millis(500);

    /// Counts the calls to [`cancel`], which also end the waits for a key to be plugged in.
    static CANCELS: AtomicU64 = AtomicU64::new(0);

    /// How the library words a key's answer that is not success, followed by the status in hex.
    const STATUS_PREFIX: &str = "response_status err = 0x";

    /// The keys this app has open, to stop them waiting for a touch.
    static OPEN: Mutex<Vec<Weak<FidoKeyHid>>> = Mutex::new(Vec::new());

    /// Tells every key this app has open to stop waiting for a touch: the request waiting on it
    /// ends with [`KeyError::Refused`] at once instead of when the key gives up, about half a
    /// minute later. Only writes to the key, so it is safe while another thread waits on it.
    pub fn cancel() {
        CANCELS.fetch_add(1, Ordering::SeqCst);
        let Ok(mut open) = OPEN.lock() else {
            return;
        };
        open.retain(|device| device.strong_count() > 0);
        for device in open.iter().filter_map(Weak::upgrade) {
            // A key that is not busy answers nothing, or that it is not busy.
            let _ = device.cancel_selection();
        }
    }

    impl SecurityKey {
        /// The plugged-in key that holds one of `keys`, and which of them. Asking costs no touch.
        pub fn holding(
            keys: &[FolderKey],
            pin: Zeroizing<String>,
        ) -> Result<(Self, usize), KeyError> {
            let mut failed = None;
            for path in paths()? {
                let Some(device) = connect(&path) else {
                    continue;
                };
                match find(&device, keys) {
                    Ok(Some(index)) => return Ok((Self::open(device, path, pin)?, index)),
                    Ok(None) => {}
                    Err(err) => failed = Some(err),
                }
            }
            Err(failed.unwrap_or(KeyError::NotEnabled))
        }

        /// Like [`Self::holding`], waiting for such a key to be plugged in when none is: `waiting`
        /// is called once then, to ask the user for it.
        pub fn holding_or_wait(
            keys: &[FolderKey],
            pin: Zeroizing<String>,
            waiting: impl FnOnce(),
        ) -> Result<(Self, usize), KeyError> {
            wait_for(
                || match Self::holding(keys, pin.clone()) {
                    Ok(found) => Ok(Some(found)),
                    Err(KeyError::NotEnabled | KeyError::NoKey) => Ok(None),
                    Err(err) => Err(err),
                },
                waiting,
            )
        }

        /// Like [`Self::without`], waiting for a new key to be plugged in when there is none,
        /// such as while the user swaps the key that opened the vault for it: `waiting` is
        /// called once then, to ask for it.
        pub fn without_or_wait(
            keys: &[FolderKey],
            pin: Zeroizing<String>,
            waiting: impl FnOnce(),
        ) -> Result<Self, KeyError> {
            wait_for(
                || match Self::without(keys, pin.clone()) {
                    Ok(key) => Ok(Some(key)),
                    Err(KeyError::AlreadyEnabled | KeyError::NoKey) => Ok(None),
                    Err(err) => Err(err),
                },
                waiting,
            )
        }

        /// The one key plugged in that holds none of `keys`, to make a new folder key on.
        pub fn without(keys: &[FolderKey], pin: Zeroizing<String>) -> Result<Self, KeyError> {
            let mut fresh = Vec::new();
            for path in paths()? {
                let Some(device) = connect(&path) else {
                    continue;
                };
                if !matches!(find(&device, keys), Ok(Some(_))) {
                    fresh.push((device, path));
                }
            }
            match fresh.len() {
                0 => Err(KeyError::AlreadyEnabled),
                1 => {
                    let (device, path) = fresh.remove(0);
                    Self::open(device, path, pin)
                }
                _ => Err(KeyError::SeveralKeys),
            }
        }

        /// Checks that the key can protect folders, and picks the PIN protocol it prefers.
        fn open(
            mut device: FidoKeyHid,
            path: String,
            pin: Zeroizing<String>,
        ) -> Result<Self, KeyError> {
            let info = device.get_info().map_err(|err| from_error(&err, None))?;
            if !info.extensions.iter().any(|name| name == "hmac-secret") {
                return Err(KeyError::NoHmacSecret);
            }
            if !info
                .options
                .iter()
                .any(|(name, set)| name == "clientPin" && *set)
            {
                return Err(KeyError::NoPin);
            }
            // The key lists them in the order it prefers; hmac-secret gives the same output
            // with either.
            device.pin_protocol_version = match info.pin_uv_auth_protocols.first() {
                Some(2) => 2,
                _ => 1,
            };
            let device = Arc::new(device);
            if let Ok(mut open) = OPEN.lock() {
                open.retain(|device| device.strong_count() > 0);
                open.push(Arc::downgrade(&device));
            }
            Ok(Self { device, path, pin })
        }

        /// Makes a credential with `hmac-secret` for `rp_id`: one touch. Returns its id.
        pub fn make_credential(&mut self, rp_id: &str) -> Result<Vec<u8>, KeyError> {
            let args = MakeCredentialArgsBuilder::new(rp_id, &challenge())
                .pin(self.pin.as_str())
                .extensions(&[CredentialExtension::HmacSecret(Some(true))])
                .build();
            let made = self.device.make_credential_with_args(&args);
            let attestation = made.map_err(|err| self.error(&err))?;
            if attestation
                .auth_data
                .get(FLAGS_AT)
                .is_some_and(|flags| flags & BACKUP_ELIGIBLE != 0)
            {
                return Err(KeyError::Synced);
            }
            let hmac_secret = attestation
                .extensions
                .iter()
                .any(|ext| matches!(ext, CredentialExtension::HmacSecret(Some(true))));
            if !hmac_secret {
                return Err(KeyError::NoHmacSecret);
            }
            Ok(attestation.credential_descriptor.id)
        }

        /// The `hmac-secret` of `credential` for what a browser's PRF sends for `input`, with
        /// the PIN, so with user verification: one touch.
        pub(super) fn hmac_secret(
            &mut self,
            credential: CredentialRef<'_>,
            input: &[u8; 32],
        ) -> Result<PrfOutput, KeyError> {
            let salt = prf_salt_input(input);
            let args = GetAssertionArgsBuilder::new(credential.rp_id, &challenge())
                .pin(self.pin.as_str())
                .credential_id(credential.credential_id)
                .extensions(&[AssertionExtension::HmacSecret(Some(salt))])
                .build();
            let answer = self.device.get_assertion_with_args(&args);
            let assertions = answer.map_err(|err| self.error(&err))?;
            let secret = assertions
                .iter()
                .flat_map(|assertion| &assertion.extensions)
                .find_map(|ext| match ext {
                    AssertionExtension::HmacSecret(Some(secret)) => Some(Zeroizing::new(*secret)),
                    _ => None,
                })
                .ok_or(KeyError::NoHmacSecret)?;
            Ok(PrfOutput::new(*secret))
        }

        /// What `err` means for the user; a wrong PIN says how many tries are left.
        fn error(&self, err: &impl Display) -> KeyError {
            let tries = (status_of(err) == Some(status::PIN_INVALID))
                .then(|| self.device.get_pin_retries().ok())
                .flatten()
                .and_then(|tries| u8::try_from(tries).ok());
            from_error(err, tries)
        }
    }

    fn config() -> LibCfg {
        // The library prints a touch prompt to stdout by default; the dialog says it instead.
        LibCfg {
            enable_keep_alive_msg: false,
            ..LibCfg::init()
        }
    }

    /// Looks until `look` finds what it looks for, calling `waiting` once when it first does not;
    /// gives up at [`cancel`] or after [`PLUG_IN_TIME`].
    fn wait_for<T>(
        mut look: impl FnMut() -> Result<Option<T>, KeyError>,
        waiting: impl FnOnce(),
    ) -> Result<T, KeyError> {
        let cancels = CANCELS.load(Ordering::SeqCst);
        let until = Instant::now() + PLUG_IN_TIME;
        let mut waiting = Some(waiting);
        loop {
            if let Some(found) = look()? {
                return Ok(found);
            }
            if let Some(waiting) = waiting.take() {
                waiting();
            }
            if CANCELS.load(Ordering::SeqCst) != cancels {
                return Err(KeyError::Refused);
            }
            if Instant::now() >= until {
                return Err(KeyError::NotPluggedIn);
            }
            thread::sleep(LOOK_EVERY);
        }
    }

    fn connect(path: &str) -> Option<FidoKeyHid> {
        FidoKeyHid::new(&[HidParam::Path(path.to_owned())], &config()).ok()
    }

    /// The keys plugged in, or why none can be asked.
    fn paths() -> Result<Vec<String>, KeyError> {
        let api = hidapi::HidApi::new().map_err(|err| KeyError::Device(err.to_string()))?;
        let paths = fido_paths(&api);
        if paths.is_empty() {
            return Err(KeyError::NoKey);
        }
        Ok(paths)
    }

    fn fido_paths(api: &hidapi::HidApi) -> Vec<String> {
        api.device_list()
            .filter(|device| device.usage_page() == FIDO_USAGE_PAGE && device.usage() == FIDO_USAGE)
            .filter_map(|device| device.path().to_str().ok().map(str::to_owned))
            .collect()
    }

    /// Which of `keys` `device` holds, asked without a touch: an assertion without user
    /// presence for each RP id, which names the credential the key found.
    fn find(device: &FidoKeyHid, keys: &[FolderKey]) -> Result<Option<usize>, KeyError> {
        let mut rp_ids: Vec<&str> = keys.iter().map(|key| key.rp_id.as_str()).collect();
        rp_ids.sort_unstable();
        rp_ids.dedup();
        for rp_id in rp_ids {
            let ids: Vec<&[u8]> = keys
                .iter()
                .filter(|key| key.rp_id == rp_id)
                .map(|key| key.credential_id.as_slice())
                .collect();
            let args = ids
                .iter()
                .fold(
                    GetAssertionArgsBuilder::new(rp_id, &challenge())
                        .without_pin_and_uv()
                        .without_up(),
                    |args, id| args.add_credential_id(id),
                )
                .build();
            let found = match device.get_assertion_with_args(&args) {
                Ok(assertions) => match (assertions.first(), ids.as_slice()) {
                    (Some(found), _) if !found.credential_id.is_empty() => {
                        found.credential_id.clone()
                    }
                    // A key may leave the credential out when only one was asked for.
                    (Some(_), [only]) => only.to_vec(),
                    _ => continue,
                },
                Err(err) if status_of(&err) == Some(status::NO_CREDENTIALS) => continue,
                Err(err) => return Err(from_error(&err, None)),
            };
            if let Some(index) = keys
                .iter()
                .position(|key| key.rp_id == rp_id && key.credential_id == found)
            {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    /// The CTAP2 status in one of the library's errors.
    pub(super) fn status_of(err: &impl Display) -> Option<u8> {
        let text = err.to_string();
        let (_, rest) = text.split_once(STATUS_PREFIX)?;
        u8::from_str_radix(rest.get(..2)?, 16).ok()
    }

    pub(super) fn from_error(err: &impl Display, tries_left: Option<u8>) -> KeyError {
        match status_of(err) {
            Some(status::PIN_INVALID) => match tries_left {
                Some(tries) => KeyError::WrongPinTriesLeft(tries),
                None => KeyError::WrongPin,
            },
            Some(status::PIN_BLOCKED) => KeyError::PinBlocked,
            Some(status::PIN_AUTH_BLOCKED) => KeyError::PinPaused,
            Some(status::PIN_NOT_SET | status::PIN_REQUIRED) => KeyError::NoPin,
            Some(status::USER_ACTION_TIMEOUT | status::ACTION_TIMEOUT) => KeyError::NotTouched,
            Some(status::OPERATION_DENIED | status::KEEPALIVE_CANCEL) => KeyError::Refused,
            Some(status::NO_CREDENTIALS) => KeyError::NotEnabled,
            _ => KeyError::Device(err.to_string()),
        }
    }

    /// Where the security keys plugged in are, without opening them.
    pub fn plugged() -> Vec<String> {
        hidapi::HidApi::new()
            .map(|api| fido_paths(&api))
            .unwrap_or_default()
    }
}

#[cfg(not(windows))]
pub use hid::{cancel, plugged};

#[cfg(windows)]
mod unsupported {
    use vault_core::{CredentialRef, PrfOutput};
    use zeroize::Zeroizing;

    use super::{FolderKey, KeyError, SecurityKey};

    impl SecurityKey {
        pub fn holding(_: &[FolderKey], _: Zeroizing<String>) -> Result<(Self, usize), KeyError> {
            Err(KeyError::Unsupported)
        }

        pub fn holding_or_wait(
            _: &[FolderKey],
            _: Zeroizing<String>,
            _: impl FnOnce(),
        ) -> Result<(Self, usize), KeyError> {
            Err(KeyError::Unsupported)
        }

        pub fn without_or_wait(
            _: &[FolderKey],
            _: Zeroizing<String>,
            _: impl FnOnce(),
        ) -> Result<Self, KeyError> {
            Err(KeyError::Unsupported)
        }

        pub fn without(_: &[FolderKey], _: Zeroizing<String>) -> Result<Self, KeyError> {
            Err(KeyError::Unsupported)
        }

        pub fn make_credential(&mut self, _: &str) -> Result<Vec<u8>, KeyError> {
            Err(KeyError::Unsupported)
        }

        pub(super) fn hmac_secret(
            &mut self,
            _: CredentialRef<'_>,
            _: &[u8; 32],
        ) -> Result<PrfOutput, KeyError> {
            Err(KeyError::Unsupported)
        }
    }

    pub fn plugged() -> Vec<String> {
        Vec::new()
    }

    pub fn cancel() {}
}

#[cfg(windows)]
pub use unsupported::{cancel, plugged};

/// Whether this app can use security keys here: not on Windows yet.
pub const SUPPORTED: bool = cfg!(not(windows));

#[cfg(all(test, not(windows)))]
mod tests {
    use super::{
        KeyError,
        hid::{from_error, status_of},
    };

    /// What the library says when the key answers with `status`, as `ctaphid.rs` words it.
    fn answer(status: u8) -> String {
        format!("response_status err = 0x{status:02X} CTAP2_ERR_SOMETHING   Something.")
    }

    #[test]
    fn the_status_is_read_from_the_library_s_text() {
        assert_eq!(
            status_of(&"response_status err = 0x31 CTAP2_ERR_PIN_INVALID   PIN Invalid."),
            Some(0x31)
        );
        assert_eq!(status_of(&"Failed to open device."), None);
        assert_eq!(status_of(&"response_status err = 0x"), None);
    }

    #[test]
    fn the_key_s_answers_read_as_the_user_s_problem() {
        let key = |status| from_error(&answer(status), None);
        assert_eq!(
            from_error(&answer(0x31), Some(5)),
            KeyError::WrongPinTriesLeft(5)
        );
        assert_eq!(key(0x31), KeyError::WrongPin);
        assert_eq!(key(0x32), KeyError::PinBlocked);
        assert_eq!(key(0x34), KeyError::PinPaused);
        assert_eq!(key(0x35), KeyError::NoPin);
        assert_eq!(key(0x2F), KeyError::NotTouched);
        assert_eq!(key(0x3A), KeyError::NotTouched);
        assert_eq!(key(0x27), KeyError::Refused);
        assert_eq!(key(0x2E), KeyError::NotEnabled);
        assert_eq!(
            from_error(&"read err", None),
            KeyError::Device("read err".to_owned())
        );
        assert_eq!(
            KeyError::WrongPinTriesLeft(3).to_string(),
            "Wrong PIN. 3 tries left before the key blocks its PIN."
        );
    }
}
