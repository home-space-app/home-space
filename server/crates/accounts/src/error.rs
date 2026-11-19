use auth::AuthError;
use db::DbError;
use files::FilesError;
use protocol::{AsProblem, FieldError, Problem, ProblemType};

/// Why an account or device operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    /// No such account or device in the tenant.
    #[error("not found")]
    NotFound,
    /// The caller's own account no longer exists in the tenant.
    #[error("authentication required")]
    Unauthorized,
    /// The caller is not an administrator.
    #[error("not allowed")]
    Forbidden,
    /// An administrator tried to disable, delete or demote themselves; the field is the JSON
    /// Pointer of what they asked for (`""` for the request as a whole).
    #[error("administrators cannot disable, delete or demote themselves")]
    NotYourself { field: &'static str },
    /// An account marked for deletion cannot be enabled until the deletion is cancelled.
    #[error("the account is marked for deletion; cancel the deletion to enable it")]
    MarkedForDeletion,
    /// A device can be shown only to other users of the tenant.
    #[error("unknown user, or the device's owner")]
    InvalidViewer,
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error(transparent)]
    Db(DbError),
}

/// A missing row is a missing account or device; anything else is the server's fault.
impl From<DbError> for AccountError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::NotFound => Self::NotFound,
            other => Self::Db(other),
        }
    }
}

impl AsProblem for AccountError {
    fn problem_type(&self) -> ProblemType {
        match self {
            Self::NotFound => ProblemType::NotFound,
            Self::Unauthorized => ProblemType::Unauthorized,
            Self::Forbidden => ProblemType::Forbidden,
            Self::NotYourself { .. } | Self::InvalidViewer => ProblemType::ValidationFailed,
            Self::MarkedForDeletion => ProblemType::MarkedForDeletion,
            Self::Auth(err) => err.problem_type(),
            Self::Files(err) => err.problem_type(),
            Self::Db(_) => ProblemType::Internal,
        }
    }

    fn extend(&self, problem: &mut Problem) {
        match self {
            Self::NotYourself { field } => {
                problem.errors = vec![FieldError::new(*field, self.to_string())];
            }
            Self::InvalidViewer => {
                problem.errors = vec![FieldError::new("/users", self.to_string())];
            }
            Self::Auth(err) => err.extend(problem),
            Self::Files(err) => err.extend(problem),
            _ => {}
        }
    }
}
