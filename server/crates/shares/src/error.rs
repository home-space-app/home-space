use auth::AuthError;
use db::DbError;
use files::FilesError;
use protocol::{AsProblem, FieldError, Problem, ProblemType};

/// Why a share operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    /// No such share, or no such item in it.
    #[error("not found")]
    NotFound,
    /// The share has expired.
    #[error("no longer available")]
    Gone,
    /// The link has a password, and the request carried no valid proof of it.
    #[error("the share needs its password")]
    Locked,
    /// A share cannot be for the user who owns it.
    #[error("a share cannot be for its owner")]
    OwnRecipient,
    /// Recipients were given for a link.
    #[error("a link is not for named users")]
    LinkWithRecipients,
    /// A password was given for a share with named users.
    #[error("a share with users has no password")]
    PasswordForUsers,
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error(transparent)]
    Db(DbError),
}

/// A missing row is a missing share or item; anything else is the server's fault.
impl From<DbError> for ShareError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::NotFound => Self::NotFound,
            other => Self::Db(other),
        }
    }
}

impl ShareError {
    /// The field of the request at fault, for the refusals that are about one.
    fn field(&self) -> Option<&'static str> {
        match self {
            Self::OwnRecipient | Self::LinkWithRecipients => Some("/recipients"),
            Self::PasswordForUsers => Some("/password"),
            _ => None,
        }
    }
}

impl AsProblem for ShareError {
    fn problem_type(&self) -> ProblemType {
        match self {
            Self::NotFound => ProblemType::NotFound,
            Self::Gone => ProblemType::Gone,
            Self::Locked => ProblemType::Unauthorized,
            Self::OwnRecipient | Self::LinkWithRecipients | Self::PasswordForUsers => {
                ProblemType::ValidationFailed
            }
            Self::Auth(err) => err.problem_type(),
            Self::Files(err) => err.problem_type(),
            Self::Db(_) => ProblemType::Internal,
        }
    }

    fn extend(&self, problem: &mut Problem) {
        if let Some(field) = self.field() {
            problem.errors = vec![FieldError::new(field, self.to_string())];
        }
        match self {
            Self::Auth(err) => err.extend(problem),
            Self::Files(err) => err.extend(problem),
            _ => {}
        }
    }
}
