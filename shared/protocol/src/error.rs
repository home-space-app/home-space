use serde::{Deserialize, Serialize};

/// Coarse machine-readable category of an error, sent as a problem's `code`. Clients that
/// need the exact failure use the problem's `type` ([`ProblemType`](crate::ProblemType)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    /// Signed in, but not allowed, such as a non-administrator on an administration endpoint.
    Forbidden,
    NotFound,
    Conflict,
    BadRequest,
    PayloadTooLarge,
    /// What was asked for is no longer kept, such as pruned changes.
    Gone,
    /// Too many attempts, such as wrong passwords; the `Retry-After` header says when to try
    /// again.
    TooManyRequests,
    /// Storing this would take the user past their storage quota (HTTP 507); freeing space,
    /// such as emptying the recycle bin, or a larger quota lets it through.
    QuotaExceeded,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&ErrorCode::PayloadTooLarge).expect("json"),
            r#""payload_too_large""#
        );
        assert_eq!(
            serde_json::to_string(&ErrorCode::QuotaExceeded).expect("json"),
            r#""quota_exceeded""#
        );
    }
}
