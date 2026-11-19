//! The one error type of the handlers, answered as an RFC 9457 problem (`docs/api-errors.md`).

mod rejection;
mod response;

pub use response::with_instance;

use protocol::{AsProblem, FieldError, Problem, ProblemType};

use accounts::AccountError;
use auth::AuthError;
use files::FilesError;
use search::SearchError;
use shares::ShareError;

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
    #[error("too many connections are open for this account")]
    TooManyConnections,
    /// No tenant answers on the host the request was sent to.
    #[error("this server does not answer on this host")]
    UnknownHost,
    /// The tenant of the host is disabled, being deleted, or not configured.
    #[error("this site is not available")]
    TenantUnavailable,
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
    #[error(transparent)]
    Accounts(#[from] AccountError),
    #[error(transparent)]
    Shares(#[from] ShareError),
    #[error("internal error: {0}")]
    Internal(String),
}

/// The details of `errors`, as one sentence for `detail`.
fn describe(errors: &[FieldError]) -> String {
    let details: Vec<&str> = errors.iter().map(|error| error.detail.as_str()).collect();
    details.join("; ")
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

    /// The problem this error answers with.
    pub fn problem(&self) -> Problem {
        let kind = self.problem_type();
        let detail = if kind.hides_detail() {
            kind.title().to_owned()
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
        match self {
            Self::Unauthorized => ProblemType::Unauthorized,
            Self::Forbidden => ProblemType::Forbidden,
            Self::NotFound => ProblemType::NotFound,
            Self::TooManyConnections => ProblemType::TooManyConnections,
            Self::UnknownHost => ProblemType::UnknownHost,
            Self::TenantUnavailable => ProblemType::TenantUnavailable,
            Self::BadRequest(_) => ProblemType::BadRequest,
            Self::UnsupportedMediaType(_) => ProblemType::UnsupportedMediaType,
            Self::BodyTooLarge(_) => ProblemType::TooLarge,
            Self::MethodNotAllowed => ProblemType::MethodNotAllowed,
            Self::Invalid(_) => ProblemType::ValidationFailed,
            Self::RangeNotSatisfiable { .. } => ProblemType::RangeNotSatisfiable,
            Self::Auth(err) => err.problem_type(),
            Self::Files(err) => err.problem_type(),
            Self::Search(err) => err.problem_type(),
            Self::Accounts(err) => err.problem_type(),
            Self::Shares(err) => err.problem_type(),
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
            Self::Accounts(err) => err.extend(problem),
            Self::Shares(err) => err.extend(problem),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
