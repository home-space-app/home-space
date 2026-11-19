//! The one error type of the handlers, answered as an RFC 9457 problem (`docs/api-errors.md`).

mod rejection;
mod response;

pub use response::with_instance;

use protocol::{AsProblem, FieldError, Problem, ProblemType};

use auth::AuthError;
use db::DbError;
use files::FilesError;
use search::SearchError;

const INTERRUPTED_BODY: &str = "the request body was cut off";
/// What `too-many-connections` asks a client to wait: a connection is free again once one of
/// the account's other sockets closes, which a client cannot see, so it tries again later.
const RECONNECT_AFTER_SECS: u64 = 30;

/// Error returned by handlers. Each variant, and each variant of the errors it wraps, answers
/// with exactly one [`ProblemType`].
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("authentication required")]
    Unauthorized,
    #[error("not allowed")]
    Forbidden,
    #[error("not found")]
    NotFound,
    /// The share has expired.
    #[error("no longer available")]
    Gone,
    /// The changes asked for were pruned.
    #[error("these changes are no longer kept; list everything again")]
    ChangesPruned,
    #[error("too many connections are open for this account")]
    TooManyConnections,
    /// No tenant answers on the host the request was sent to.
    #[error("this server does not answer on this host")]
    UnknownHost,
    /// The tenant of the host is disabled, being deleted, or not configured.
    #[error("this site is not available")]
    TenantUnavailable,
    /// An account marked for deletion was to be enabled.
    #[error("the account is marked for deletion; cancel the deletion to enable it")]
    MarkedForDeletion,
    /// The request cannot be read: the body, query, path or a header.
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    UnsupportedMediaType(String),
    /// The body is larger than the endpoint takes.
    #[error("{0}")]
    BodyTooLarge(String),
    #[error("this endpoint does not take this method")]
    MethodNotAllowed,
    /// The request was read, but these fields are not acceptable.
    #[error("{}", describe(.0))]
    Invalid(Vec<FieldError>),
    /// The `Range` lies outside a file of `size` bytes.
    #[error("the range lies outside the file of {size} bytes")]
    RangeNotSatisfiable { size: u64 },
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error(transparent)]
    Search(#[from] SearchError),
    #[error("internal error: {0}")]
    Internal(String),
}

/// The details of `errors`, as one sentence for `detail`.
fn describe(errors: &[FieldError]) -> String {
    let details: Vec<&str> = errors.iter().map(|error| error.detail.as_str()).collect();
    details.join("; ")
}

/// The one mapping of a database error to a response: a missing row is a 404, pruned changes
/// are gone, and anything else is the server's fault. A database error that means something
/// more specific in one place is mapped there, before it gets here.
impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::NotFound => Self::NotFound,
            DbError::Pruned => Self::ChangesPruned,
            other => Self::Internal(other.to_string()),
        }
    }
}

/// The fields a request's [`Validate`](protocol::Validate) found at fault.
impl From<Vec<FieldError>> for ApiError {
    fn from(errors: Vec<FieldError>) -> Self {
        Self::Invalid(errors)
    }
}

impl ApiError {
    /// One field at `pointer` (a JSON Pointer, such as `/recipients`) is not acceptable for a
    /// reason that needs more than the request to see, such as who is asking.
    pub fn invalid(pointer: &str, detail: impl Into<String>) -> Self {
        Self::Invalid(vec![FieldError::new(pointer, detail)])
    }

    /// Whether the request body stopped arriving, as when the client or a proxy in front of
    /// the server drops the connection mid-upload. Bytes received so far are kept.
    fn is_interrupted_body(&self) -> bool {
        matches!(
            self,
            Self::Files(FilesError::Io(err))
                if err.get_ref().is_some_and(|inner| inner.is::<axum::Error>())
        )
    }

    /// The problem this error answers with.
    pub fn problem(&self) -> Problem {
        let kind = self.problem_type();
        let detail = if kind.hides_detail() {
            kind.title().to_owned()
        } else if kind == ProblemType::BodyInterrupted {
            INTERRUPTED_BODY.to_owned()
        } else {
            self.to_string()
        };
        let mut problem = Problem::new(kind, detail);
        self.extend(&mut problem);
        problem
    }
}

impl AsProblem for ApiError {
    fn problem_type(&self) -> ProblemType {
        if self.is_interrupted_body() {
            return ProblemType::BodyInterrupted;
        }
        match self {
            Self::Unauthorized => ProblemType::Unauthorized,
            Self::Forbidden => ProblemType::Forbidden,
            Self::NotFound => ProblemType::NotFound,
            Self::Gone => ProblemType::Gone,
            Self::ChangesPruned => ProblemType::ChangesPruned,
            Self::TooManyConnections => ProblemType::TooManyConnections,
            Self::UnknownHost => ProblemType::UnknownHost,
            Self::TenantUnavailable => ProblemType::TenantUnavailable,
            Self::MarkedForDeletion => ProblemType::MarkedForDeletion,
            Self::BadRequest(_) => ProblemType::BadRequest,
            Self::UnsupportedMediaType(_) => ProblemType::UnsupportedMediaType,
            Self::BodyTooLarge(_) => ProblemType::TooLarge,
            Self::MethodNotAllowed => ProblemType::MethodNotAllowed,
            Self::Invalid(_) => ProblemType::ValidationFailed,
            Self::RangeNotSatisfiable { .. } => ProblemType::RangeNotSatisfiable,
            Self::Auth(err) => err.problem_type(),
            Self::Files(err) => err.problem_type(),
            Self::Search(err) => err.problem_type(),
            Self::Internal(_) => ProblemType::Internal,
        }
    }

    fn extend(&self, problem: &mut Problem) {
        match self {
            Self::Invalid(errors) => problem.errors.clone_from(errors),
            Self::TooManyConnections => problem.retry_after = Some(RECONNECT_AFTER_SECS),
            Self::Auth(err) => err.extend(problem),
            Self::Files(err) => err.extend(problem),
            Self::Search(err) => err.extend(problem),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
