//! Server features: short stable names for what a server offers that an older one does not.
//!
//! The server reports [`ALL`] in `GET /capabilities`; each client declares the ones it cannot
//! work without (`REQUIRES`), and [`crate::compat::check`] compares the two. A name never
//! changes meaning and is never reused. See `docs/versioning.md` for when to add one.
//!
//! The list starts empty at 0.13.0: everything older is the baseline every server that
//! reports a version has.

/// Apps sign in through the server's web page: `/auth/browser/start`, `approve`, `deny`,
/// `exchange` and `flows/{flow}`, and `browser_login` in `GET /auth/providers`.
pub const BROWSER_LOGIN: &str = "browser_login";

/// Sensitive changes may answer `403 reauthentication-required`, which
/// `POST /me/reauthenticate` with the password clears for ten minutes.
pub const REAUTHENTICATION: &str = "reauthentication";

/// Accounts can add an authenticator app (`/me/totp`) and recovery codes
/// (`/me/recovery-codes`); a sign-in with `two_step` may then finish at `/auth/login/verify`,
/// and `/me/reauthenticate` takes a `code` too.
pub const TWO_FACTOR: &str = "two_factor";

/// Accounts can add passkeys and security keys (`/me/webauthn`), which count as a second
/// factor; a passkey signs in on its own (`/auth/passkey/start`, `finish`, and `passkeys` in
/// `GET /auth/providers`); `/auth/login/verify` and `/me/reauthenticate` take `webauthn`.
pub const PASSKEYS: &str = "passkeys";

/// `GET /me/webauthn` says for each key whether it made a PRF (`hmac-secret`) secret and
/// whether it is synced (`prf`, `synced`), so clients can tell which keys can lock protected
/// folders.
pub const SECURITY_KEY_PRF: &str = "security_key_prf";

/// Folders can be protected, locked with a security key (end to end) or a password (encrypted
/// by the server): `/vault` (unlockers, rotation) and `/protected` (unlock, password, unprotect).
/// Clients without it hide Protect.
pub const PROTECTED_FOLDERS: &str = "protected_folders";

/// Every feature this server provides, each added with the release that introduced it.
pub const ALL: &[&str] = &[
    BROWSER_LOGIN,
    PASSKEYS,
    PROTECTED_FOLDERS,
    REAUTHENTICATION,
    SECURITY_KEY_PRF,
    TWO_FACTOR,
];

/// What the user reads for each feature, in hints such as "These won't work: …".
const DISPLAY_NAMES: &[(&str, &str)] = &[
    (BROWSER_LOGIN, "Sign-in in the browser"),
    (PASSKEYS, "Passkeys"),
    (PROTECTED_FOLDERS, "Protected folders"),
    (REAUTHENTICATION, "Confirming sensitive changes"),
    (SECURITY_KEY_PRF, "Security keys for protected folders"),
    (TWO_FACTOR, "Two-factor sign-in"),
];

/// What the user reads for `feature`; the name itself for one this build does not know.
pub fn display_name(feature: &str) -> &str {
    DISPLAY_NAMES
        .iter()
        .find(|(name, _)| *name == feature)
        .map_or(feature, |(_, display)| display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_has_a_display_name() {
        for feature in ALL {
            assert!(
                DISPLAY_NAMES.iter().any(|(name, _)| name == feature),
                "{feature} has no display name"
            );
        }
    }

    #[test]
    fn feature_names_are_unique() {
        let mut names = ALL.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }

    #[test]
    fn an_unknown_feature_shows_its_name() {
        assert_eq!(display_name("from_the_future"), "from_the_future");
    }
}
