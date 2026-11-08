use axum::{
    Json,
    http::{HeaderValue, StatusCode, header::RETRY_AFTER},
    response::{IntoResponse, Response},
};

use auth::AuthError;
use files::FilesError;
use protocol::{ErrorBody, ErrorCode};
use search::SearchError;

const INTERRUPTED_BODY: &str = "the request body was cut off";

/// Error returned by handlers, rendered as `{ "error": code, "message": text }`.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("authentication required")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("no longer available")]
    Gone,
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error(transparent)]
    Search(#[from] SearchError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl ApiError {
    /// Whether the request body stopped arriving, as when the client or a proxy in front of
    /// the server drops the connection mid-upload. Bytes received so far are kept.
    fn is_interrupted_body(&self) -> bool {
        matches!(
            self,
            Self::Files(FilesError::Io(err))
                if err.get_ref().is_some_and(|inner| inner.is::<axum::Error>())
        )
    }

    fn status_and_code(&self) -> (StatusCode, ErrorCode) {
        if self.is_interrupted_body() {
            return (StatusCode::BAD_REQUEST, ErrorCode::BadRequest);
        }
        match self {
            Self::Unauthorized | Self::Auth(AuthError::InvalidCredentials) => {
                (StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized)
            }
            Self::Auth(AuthError::UsernameTaken) => (StatusCode::CONFLICT, ErrorCode::Conflict),
            Self::Auth(AuthError::InvalidInput(_)) => {
                (StatusCode::BAD_REQUEST, ErrorCode::BadRequest)
            }
            Self::NotFound
            | Self::Auth(AuthError::OidcDisabled)
            | Self::Files(FilesError::NotFound) => (StatusCode::NOT_FOUND, ErrorCode::NotFound),
            Self::Files(
                FilesError::Conflict | FilesError::LostAndFound | FilesError::OffsetMismatch { .. },
            ) => (StatusCode::CONFLICT, ErrorCode::Conflict),
            Self::BadRequest(_)
            | Self::Files(FilesError::InvalidName | FilesError::Cycle | FilesError::NotAFile) => {
                (StatusCode::BAD_REQUEST, ErrorCode::BadRequest)
            }
            Self::Gone => (StatusCode::GONE, ErrorCode::Gone),
            Self::Auth(AuthError::TooManyAttempts { .. }) => {
                (StatusCode::TOO_MANY_REQUESTS, ErrorCode::TooManyRequests)
            }
            Self::Files(FilesError::TooLarge { .. }) => {
                (StatusCode::PAYLOAD_TOO_LARGE, ErrorCode::PayloadTooLarge)
            }
            Self::Auth(_) | Self::Files(_) | Self::Search(_) | Self::Internal(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal)
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "request failed");
            "internal server error".to_owned()
        } else if self.is_interrupted_body() {
            tracing::warn!(error = %self, "request body interrupted");
            INTERRUPTED_BODY.to_owned()
        } else {
            self.to_string()
        };
        let mut response = (
            status,
            Json(ErrorBody {
                error: code,
                message,
            }),
        )
            .into_response();
        if let Self::Auth(AuthError::TooManyAttempts { retry_after }) = self {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(retry_after));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn a_cut_off_body_is_a_bad_request_not_a_server_error() {
        let cut = io::Error::other(axum::Error::new(io::Error::from(
            io::ErrorKind::ConnectionReset,
        )));
        let error = ApiError::Files(FilesError::Io(cut));
        assert_eq!(
            error.status_and_code(),
            (StatusCode::BAD_REQUEST, ErrorCode::BadRequest)
        );
    }

    #[test]
    fn a_disk_error_is_still_a_server_error() {
        let disk = io::Error::from(io::ErrorKind::StorageFull);
        let error = ApiError::Files(FilesError::Io(disk));
        assert_eq!(
            error.status_and_code(),
            (StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal)
        );
    }
}
