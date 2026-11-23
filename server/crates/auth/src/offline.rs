use db::{Db, DbError, TenantId, User, UserId};

use crate::{AuthError, password};

/// Creates a user of `tenant` after validating the username and password.
///
/// # Errors
/// [`AuthError::InvalidInput`] for a username or password that breaks the rules,
/// [`AuthError::UsernameTaken`] when the tenant already has the username, and database or
/// hashing failures.
pub async fn create_user(
    db: &Db,
    tenant: TenantId,
    username: &str,
    password: &str,
) -> Result<User, AuthError> {
    check_username(username)?;
    let hashed = hash_new(password).await?;
    insert_user(db, tenant, username, &hashed).await
}

/// Replaces a user's password and ends all their sessions.
///
/// # Errors
/// [`AuthError::InvalidInput`] for a password that breaks the rules, and database or hashing
/// failures.
pub async fn set_password(db: &Db, user: UserId, password: &str) -> Result<(), AuthError> {
    let hashed = hash_new(password).await?;
    db.set_password_hash(user, &hashed).await?;
    Ok(())
}

pub(crate) fn check_username(username: &str) -> Result<(), AuthError> {
    match protocol::rules::username_problem(username) {
        Some(problem) => Err(AuthError::InvalidInput(problem)),
        None => Ok(()),
    }
}

pub(crate) async fn insert_user(
    db: &Db,
    tenant: TenantId,
    username: &str,
    password_hash: &str,
) -> Result<User, AuthError> {
    db.create_user(tenant, username, password_hash)
        .await
        .map_err(|err| match err {
            DbError::Conflict => AuthError::UsernameTaken,
            other => other.into(),
        })
}

async fn hash_new(password: &str) -> Result<String, AuthError> {
    password::check_new(password)?;
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || password::hash(&password)).await?
}

#[cfg(test)]
mod tests;
