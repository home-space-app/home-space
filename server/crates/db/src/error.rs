/// Errors returned by the database layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// The row does not exist or is not owned by the caller.
    #[error("not found")]
    NotFound,
    /// A uniqueness constraint was violated (e.g. duplicate name in a folder).
    #[error("conflict")]
    Conflict,
    /// A value violated a check constraint (e.g. invalid node name).
    #[error("invalid value")]
    Invalid,
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("database error: {0}")]
    Sqlx(sqlx::Error),
}

impl From<sqlx::Error> for DbError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::RowNotFound => Self::NotFound,
            sqlx::Error::Database(db) if db.is_unique_violation() => Self::Conflict,
            sqlx::Error::Database(db) if db.is_check_violation() => Self::Invalid,
            sqlx::Error::Database(db) if db.is_foreign_key_violation() => Self::NotFound,
            _ => Self::Sqlx(err),
        }
    }
}
