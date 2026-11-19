//! Requests the framework refuses before a handler runs, answered as problems like any other
//! error: a body, query or path that cannot be read, or a body in the wrong format.

use axum::{
    extract::rejection::{JsonRejection, PathRejection, QueryRejection},
    http::StatusCode,
};

use super::ApiError;

/// The error for a rejection with axum's `status` and `text`: the media type and size keep
/// their own problems, and everything else that is the client's fault cannot be read.
fn rejected(status: StatusCode, text: String) -> ApiError {
    match status {
        StatusCode::UNSUPPORTED_MEDIA_TYPE => ApiError::UnsupportedMediaType(text),
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::BodyTooLarge(text),
        status if status.is_server_error() => ApiError::Internal(text),
        _ => ApiError::BadRequest(text),
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        rejected(rejection.status(), rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        rejected(rejection.status(), rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        rejected(rejection.status(), rejection.body_text())
    }
}
