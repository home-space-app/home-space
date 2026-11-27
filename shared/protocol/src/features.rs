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
/// and `/me/reauthenticate` takes a `code` too. An account linked to the identity provider
/// (`external` in `/me` and `/admin/users`) gets `409 external-account` for these, for passkeys
/// and for a password sign-in; `PUT /admin/users/{id}/password` makes it a local account and
/// needs the administrator's recent sign-in. Once an account has a second factor, changing its
/// password, authenticator app, recovery codes, passkeys or security keys takes a recent
/// sign-in or confirmation with one, so `reauthentication-required` names only `totp` and
/// `webauthn` there; removing a second factor ends the account's other sessions.
pub const TWO_FACTOR: &str = "two_factor";

/// Accounts can add passkeys and security keys (`/me/webauthn`), which count as a second
/// factor; a passkey signs in on its own (`/auth/passkey/start`, `finish`, and `passkeys` in
/// `GET /auth/providers`); `/auth/login/verify` and `/me/reauthenticate` take `webauthn`.
/// Renaming one needs a recent confirmation, as adding and removing do.
pub const PASSKEYS: &str = "passkeys";

/// Folders can be protected end to end, locked with a security key or a password: `/vault`
/// (unlockers, rotation) and `/protected` (protect, finish, password, unprotect). The client
/// encrypts; the server keeps wrapped secrets, Argon2id parameters and age recipients. The
/// vault's security keys are folder keys, credentials a client makes for the vault alone
/// (`name`, `credential_id`, `rp_id`, `prf_salt`), never sign-in credentials. Moves across
/// a protected folder's edge get `409 protected`. Names inside a protected folder may be up to
/// 1387 characters (`rules::MAX_SEALED_NODE_NAME_CHARS`), the sealed text of a 255-character
/// name; elsewhere the limit stays 255. Clients without it hide Protect.
pub const PROTECTED_FOLDERS: &str = "protected_folders";

/// Every feature this server provides, each added with the release that introduced it.
pub const ALL: &[&str] = &[
    BROWSER_LOGIN,
    PASSKEYS,
    PROTECTED_FOLDERS,
    REAUTHENTICATION,
    TWO_FACTOR,
];

/// What the user reads for each feature, in hints such as "These won't work: …".
const DISPLAY_NAMES: &[(&str, &str)] = &[
    (BROWSER_LOGIN, "Sign-in in the browser"),
    (PASSKEYS, "Passkeys"),
    (PROTECTED_FOLDERS, "Protected folders"),
    (REAUTHENTICATION, "Confirming sensitive changes"),
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
