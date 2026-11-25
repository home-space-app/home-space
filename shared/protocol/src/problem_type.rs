//! Every problem the server answers with: its RFC 9457 `type`, status, title and error code.
//!
//! This is the one table of error statuses. A server error type picks one of these for each of
//! its variants in an exhaustive `match`, so a new variant cannot turn into a 500 unnoticed.

use crate::ErrorCode;

/// Where problem types live: the `type` of a problem is this followed by its slug.
pub const PROBLEMS_BASE: &str = "/api/v1/problems/";

/// One kind of failure. Clients that need to tell failures apart match on this, through
/// [`Problem::kind`](crate::Problem::kind), not on the text of `detail`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProblemType {
    /// The request could not be read: malformed JSON, a bad id in the path, a missing header.
    BadRequest,
    /// The request body stopped arriving before it was complete.
    BodyInterrupted,
    /// The body is not in a format the endpoint takes, such as JSON without its content type.
    UnsupportedMediaType,
    /// The endpoint exists but not for this method.
    MethodNotAllowed,
    /// The request was read, but some of its fields are not acceptable; `errors` says which.
    ValidationFailed,
    /// No session, or its token is invalid, expired or revoked, or the password is wrong.
    Unauthorized,
    /// Signed in, but not allowed.
    Forbidden,
    /// The account is disabled or marked for deletion.
    AccountDisabled,
    /// The identity provider signed the user in, but the account may not use this server.
    SsoAccessDenied,
    NotFound,
    /// Single sign-on is not set up on this server.
    SsoDisabled,
    /// No site answers on the host the request was sent to.
    UnknownHost,
    /// The share or other resource existed but has expired.
    Gone,
    /// The changes asked for were pruned; the client has to list everything again.
    ChangesPruned,
    /// The transfer was declined, cancelled, saved or has expired.
    TransferGone,
    /// The browser sign-in of an app was already used, cancelled or has expired.
    LoginExpired,
    /// An item with this name already exists in the folder.
    NameTaken,
    UsernameTaken,
    /// Lost & Found, or a folder holding it, cannot be deleted.
    LostAndFound,
    /// A chunk does not start where the upload ends; `current_offset` says where it does.
    OffsetMismatch,
    /// The transfer's files are still being uploaded.
    TransferNotReady,
    /// An account marked for deletion cannot be enabled until the deletion is cancelled.
    MarkedForDeletion,
    /// The current password given to change it is wrong.
    WrongPassword,
    /// A sensitive change needs the user to confirm it is them first; `methods` says how,
    /// and without any they sign in again.
    ReauthenticationRequired,
    /// The account signs in through single sign-on and has no password.
    NoPassword,
    /// The password was right, and the account also needs a second factor. With `two_step`
    /// the problem carries a `login_token` for `POST /auth/login/verify` and the `methods`.
    SecondFactorRequired,
    /// The code from the authenticator app or the recovery code is wrong.
    InvalidCode,
    /// An authenticator app is already set up; it has to be removed before another is added.
    TotpExists,
    /// The server has no key to keep authenticator app secrets with, so none can be set up.
    SecretsKeyMissing,
    /// The passkey or security key could not be verified: a wrong or replayed answer, or one
    /// for another challenge.
    WebAuthnFailed,
    /// Passkeys and security keys cannot be used on this address: it is not https (or
    /// `localhost`), is an IP address, or is not the server's own.
    WebAuthnUnavailable,
    /// The account has as many passkeys and security keys as it may.
    TooManyKeys,
    /// A folder cannot be moved into itself or into one of its folders.
    MoveIntoItself,
    /// The item is a folder, and only a file can do this.
    NotAFile,
    /// The name is empty, too long, `.` or `..`, or holds `/` or `\`.
    InvalidName,
    /// The list of files of a transfer is not acceptable.
    InvalidTransfer,
    /// The `Range` asked for lies outside the file.
    RangeNotSatisfiable,
    /// Too many wrong passwords; `retry_after` says when to try again.
    TooManyAttempts,
    /// More passwords are waiting to be checked than the server takes on; `retry_after` says
    /// when to try again.
    ServerBusy,
    /// The account has as many live connections as it may; `retry_after` says when to try
    /// again.
    TooManyConnections,
    /// The body or upload is larger than `limit` bytes.
    TooLarge,
    /// More than `limit` items were asked for at once.
    TooManyItems,
    /// Storing this would pass the account's quota; `quota` and `available` say by how much.
    QuotaExceeded,
    /// The site of this host is disabled, being deleted, or not set up.
    TenantUnavailable,
    /// Apps cannot sign in in the browser: the server does not serve the web client.
    BrowserLoginUnavailable,
    /// The identity provider could not be reached or answered with something invalid.
    IdentityProviderFailed,
    /// Something went wrong on the server.
    Internal,
}

impl ProblemType {
    /// Every problem type, for documentation and tests.
    pub const ALL: [Self; 47] = [
        Self::BadRequest,
        Self::BodyInterrupted,
        Self::UnsupportedMediaType,
        Self::MethodNotAllowed,
        Self::ValidationFailed,
        Self::Unauthorized,
        Self::Forbidden,
        Self::AccountDisabled,
        Self::SsoAccessDenied,
        Self::NotFound,
        Self::SsoDisabled,
        Self::UnknownHost,
        Self::Gone,
        Self::ChangesPruned,
        Self::TransferGone,
        Self::LoginExpired,
        Self::NameTaken,
        Self::UsernameTaken,
        Self::LostAndFound,
        Self::OffsetMismatch,
        Self::TransferNotReady,
        Self::MarkedForDeletion,
        Self::WrongPassword,
        Self::ReauthenticationRequired,
        Self::NoPassword,
        Self::SecondFactorRequired,
        Self::InvalidCode,
        Self::TotpExists,
        Self::SecretsKeyMissing,
        Self::WebAuthnFailed,
        Self::WebAuthnUnavailable,
        Self::TooManyKeys,
        Self::MoveIntoItself,
        Self::NotAFile,
        Self::InvalidName,
        Self::InvalidTransfer,
        Self::RangeNotSatisfiable,
        Self::TooManyAttempts,
        Self::ServerBusy,
        Self::TooManyConnections,
        Self::TooLarge,
        Self::TooManyItems,
        Self::QuotaExceeded,
        Self::TenantUnavailable,
        Self::BrowserLoginUnavailable,
        Self::IdentityProviderFailed,
        Self::Internal,
    ];

    /// Status, slug, error code and title, in one row per type.
    const fn row(self) -> (u16, &'static str, ErrorCode, &'static str) {
        use ErrorCode as C;
        match self {
            Self::BadRequest => (
                400,
                "bad-request",
                C::BadRequest,
                "The request could not be read",
            ),
            Self::BodyInterrupted => (
                400,
                "body-interrupted",
                C::BadRequest,
                "The request body was cut off",
            ),
            Self::UnsupportedMediaType => (
                415,
                "unsupported-media-type",
                C::BadRequest,
                "The body is not in a format this endpoint takes",
            ),
            Self::MethodNotAllowed => (
                405,
                "method-not-allowed",
                C::BadRequest,
                "This endpoint does not take this method",
            ),
            Self::ValidationFailed => (
                422,
                "validation-failed",
                C::BadRequest,
                "Some fields of the request are not acceptable",
            ),
            Self::Unauthorized => (401, "unauthorized", C::Unauthorized, "Not signed in"),
            Self::Forbidden => (403, "forbidden", C::Forbidden, "Not allowed"),
            Self::AccountDisabled => (
                403,
                "account-disabled",
                C::Forbidden,
                "The account is disabled",
            ),
            Self::SsoAccessDenied => (
                403,
                "sso-access-denied",
                C::Forbidden,
                "The account may not use this server",
            ),
            Self::NotFound => (404, "not-found", C::NotFound, "Not found"),
            Self::SsoDisabled => (
                404,
                "sso-disabled",
                C::NotFound,
                "Single sign-on is not enabled",
            ),
            Self::UnknownHost => (
                421,
                "unknown-host",
                C::NotFound,
                "This server does not answer on this host",
            ),
            Self::Gone => (410, "gone", C::Gone, "No longer available"),
            Self::ChangesPruned => (
                410,
                "changes-pruned",
                C::Gone,
                "These changes are no longer kept",
            ),
            Self::TransferGone => (
                410,
                "transfer-gone",
                C::Gone,
                "The transfer is no longer available",
            ),
            Self::LoginExpired => (
                410,
                "login-expired",
                C::Gone,
                "The sign-in request is no longer valid",
            ),
            Self::NameTaken => (
                409,
                "name-taken",
                C::Conflict,
                "An item with this name already exists",
            ),
            Self::UsernameTaken => (409, "username-taken", C::Conflict, "The username is taken"),
            Self::LostAndFound => (
                409,
                "lost-and-found",
                C::Conflict,
                "Lost & Found cannot be deleted",
            ),
            Self::OffsetMismatch => (
                409,
                "offset-mismatch",
                C::Conflict,
                "The chunk does not start where the upload ends",
            ),
            Self::TransferNotReady => (
                409,
                "transfer-not-ready",
                C::Conflict,
                "The transfer is not ready yet",
            ),
            Self::MarkedForDeletion => (
                409,
                "marked-for-deletion",
                C::Conflict,
                "The account is marked for deletion",
            ),
            Self::WrongPassword => (
                422,
                "wrong-password",
                C::BadRequest,
                "The current password is wrong",
            ),
            Self::ReauthenticationRequired => (
                403,
                "reauthentication-required",
                C::Forbidden,
                "Confirm it is you first",
            ),
            Self::NoPassword => (
                422,
                "no-password",
                C::BadRequest,
                "The account has no password",
            ),
            Self::SecondFactorRequired => (
                403,
                "second-factor-required",
                C::Forbidden,
                "A second factor is needed to sign in",
            ),
            Self::InvalidCode => (422, "invalid-code", C::BadRequest, "The code is wrong"),
            Self::TotpExists => (
                409,
                "totp-exists",
                C::Conflict,
                "An authenticator app is already set up",
            ),
            Self::SecretsKeyMissing => (
                503,
                "secrets-key-missing",
                C::Internal,
                "Two-factor sign-in is not set up on this server",
            ),
            Self::WebAuthnFailed => (
                422,
                "webauthn-failed",
                C::BadRequest,
                "The passkey or security key could not be verified",
            ),
            Self::WebAuthnUnavailable => (
                400,
                "webauthn-unavailable",
                C::BadRequest,
                "Passkeys cannot be used on this address",
            ),
            Self::TooManyKeys => (
                409,
                "too-many-keys",
                C::Conflict,
                "The account has too many passkeys and security keys",
            ),
            Self::MoveIntoItself => (
                422,
                "move-into-itself",
                C::BadRequest,
                "A folder cannot be moved into itself",
            ),
            Self::NotAFile => (422, "not-a-file", C::BadRequest, "The item is not a file"),
            Self::InvalidName => (422, "invalid-name", C::BadRequest, "The name is not valid"),
            Self::InvalidTransfer => (
                422,
                "invalid-transfer",
                C::BadRequest,
                "The transfer is not valid",
            ),
            Self::RangeNotSatisfiable => (
                416,
                "range-not-satisfiable",
                C::BadRequest,
                "The range lies outside the file",
            ),
            Self::TooManyAttempts => (
                429,
                "too-many-attempts",
                C::TooManyRequests,
                "Too many failed sign-in attempts",
            ),
            Self::ServerBusy => (429, "server-busy", C::TooManyRequests, "The server is busy"),
            Self::TooManyConnections => (
                429,
                "too-many-connections",
                C::TooManyRequests,
                "Too many connections are open for this account",
            ),
            Self::TooLarge => (413, "too-large", C::PayloadTooLarge, "Too large"),
            Self::TooManyItems => (
                413,
                "too-many-items",
                C::PayloadTooLarge,
                "Too many items at once",
            ),
            Self::QuotaExceeded => (
                507,
                "quota-exceeded",
                C::QuotaExceeded,
                "Not enough storage left",
            ),
            Self::TenantUnavailable => (
                503,
                "tenant-unavailable",
                C::Internal,
                "This site is not available",
            ),
            Self::BrowserLoginUnavailable => (
                503,
                "browser-login-unavailable",
                C::Internal,
                "Sign-in in the browser is not available on this server",
            ),
            Self::IdentityProviderFailed => (
                502,
                "identity-provider-failed",
                C::Internal,
                "The identity provider could not be reached",
            ),
            Self::Internal => (500, "internal", C::Internal, "Internal server error"),
        }
    }

    /// The HTTP status code.
    pub const fn status(self) -> u16 {
        self.row().0
    }

    /// The stable name in the `type` URI, such as `quota-exceeded`.
    pub const fn slug(self) -> &'static str {
        self.row().1
    }

    /// The coarse category, sent as `code` (and, until clients have moved on, as `error`).
    pub const fn code(self) -> ErrorCode {
        self.row().2
    }

    /// A short summary that is the same for every occurrence.
    pub const fn title(self) -> &'static str {
        self.row().3
    }

    /// The `type` member: [`PROBLEMS_BASE`] and the slug.
    pub fn uri(self) -> String {
        format!("{PROBLEMS_BASE}{}", self.slug())
    }

    /// The problem type a `type` member names, if it is one of these.
    pub fn from_uri(uri: &str) -> Option<Self> {
        let slug = uri.rsplit_once(PROBLEMS_BASE)?.1;
        Self::ALL.into_iter().find(|kind| kind.slug() == slug)
    }

    /// Whether `detail` would only tell the client about the server's insides, so the title
    /// is sent instead and the cause is logged.
    pub const fn hides_detail(self) -> bool {
        matches!(self, Self::Internal | Self::IdentityProviderFailed)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_type_has_its_own_slug_and_an_error_status() {
        let mut slugs = HashSet::new();
        for kind in ProblemType::ALL {
            assert!(slugs.insert(kind.slug()), "{kind:?} repeats a slug");
            assert!((400..600).contains(&kind.status()), "{kind:?}");
            assert_eq!(ProblemType::from_uri(&kind.uri()), Some(kind));
        }
    }

    #[test]
    fn an_absolute_type_uri_is_recognized() {
        assert_eq!(
            ProblemType::from_uri("https://files.example.com/api/v1/problems/quota-exceeded"),
            Some(ProblemType::QuotaExceeded)
        );
        assert_eq!(ProblemType::from_uri("about:blank"), None);
    }

    /// The table in `docs/api-errors.md` lists every type with its status.
    #[test]
    fn the_documented_table_matches() {
        let doc = include_str!("../../../docs/api-errors.md");
        for kind in ProblemType::ALL {
            let row = format!("| `{}` | {} |", kind.slug(), kind.status());
            assert!(doc.contains(&row), "docs/api-errors.md lacks {row}");
        }
    }
}
