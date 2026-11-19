//! The body of every error response: an RFC 9457 problem (`application/problem+json`).

use serde::{Deserialize, Serialize};

use crate::{ErrorCode, ProblemType};

/// Media type of [`Problem`] bodies.
pub const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

/// An error response, as RFC 9457 describes it, with this API's extension members.
///
/// Every member is optional when reading, so a body from a server that predates RFC 9457
/// (`{ "error": code, "message": text }`) reads as well; use [`code`](Self::code) and
/// [`detail`](Self::detail) to get at it either way.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    /// URI of the problem type, one of [`ProblemType`]'s.
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub type_uri: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The HTTP status code, repeated for clients that only keep the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// What went wrong this time, written for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The path of the request that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// The coarse category of the failure, stable for clients to branch on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<ErrorCode>,
    /// Seconds to wait before trying again, as in the `Retry-After` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
    /// The size or count that was passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// The account's storage quota in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<u64>,
    /// Bytes of the quota still free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<u64>,
    /// Where an upload ends, and so where its next chunk must start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_offset: Option<u64>,
    /// The fields that failed validation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FieldError>,
    /// The same as `code`, for clients from before RFC 9457. To be removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorCode>,
    /// The same as `detail`, for clients from before RFC 9457. To be removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One field of a request that is not acceptable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// JSON Pointer (RFC 6901) to the field in the body or query, such as `/node_ids`.
    pub pointer: String,
    pub detail: String,
}

impl FieldError {
    pub fn new(pointer: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
            detail: detail.into(),
        }
    }
}

impl Problem {
    /// A problem of `kind` with `detail`, also filled in under the old member names.
    pub fn new(kind: ProblemType, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            type_uri: kind.uri(),
            title: kind.title().to_owned(),
            status: Some(kind.status()),
            code: Some(kind.code()),
            error: Some(kind.code()),
            message: Some(detail.clone()),
            detail: Some(detail),
            ..Self::default()
        }
    }

    /// The problem type, when the server named one this client knows.
    pub fn kind(&self) -> Option<ProblemType> {
        ProblemType::from_uri(&self.type_uri)
    }

    /// The error code, from `code` or else from the old `error`.
    pub fn code(&self) -> Option<ErrorCode> {
        self.code.or(self.error)
    }

    /// What went wrong, from `detail`, the old `message`, or else the title.
    pub fn detail(&self) -> &str {
        self.detail
            .as_deref()
            .or(self.message.as_deref())
            .unwrap_or(&self.title)
    }
}

/// An error that answers as a problem: it names its [`ProblemType`] and adds the extension
/// members it carries. Implementations match every variant, without a catch-all arm.
pub trait AsProblem {
    fn problem_type(&self) -> ProblemType;

    /// Adds members such as `limit` or `retry_after`. Most errors carry none.
    fn extend(&self, _problem: &mut Problem) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_problem_carries_the_old_members_too() {
        let problem = Problem::new(
            ProblemType::NameTaken,
            "a node with this name already exists",
        );
        let json = serde_json::to_value(&problem).expect("json");
        assert_eq!(json["type"], "/api/v1/problems/name-taken");
        assert_eq!(json["status"], 409);
        assert_eq!(json["code"], "conflict");
        assert_eq!(json["error"], "conflict");
        assert_eq!(json["detail"], json["message"]);
        assert!(json.get("errors").is_none());
    }

    #[test]
    fn an_old_error_body_still_reads() {
        let problem: Problem =
            serde_json::from_str(r#"{"error":"not_found","message":"not found"}"#).expect("json");
        assert_eq!(problem.code(), Some(ErrorCode::NotFound));
        assert_eq!(problem.detail(), "not found");
        assert_eq!(problem.kind(), None);
    }
}
