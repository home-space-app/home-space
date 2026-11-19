//! Password hashing, a few at a time on blocking threads.

use std::{sync::Arc, thread::available_parallelism, time::Duration};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{AuthError, password};

/// Passwords hashed at once when the number of processors is unknown.
const FALLBACK_HASHING_SLOTS: usize = 2;
/// How long a password waits for its turn to be hashed before the request is refused.
const HASHING_WAIT: Duration = Duration::from_secs(5);
const DUMMY_PASSWORD: &str = "dummy-password-for-timing";

/// Hashes and verifies passwords; cheap to clone, and clones share their turns.
#[derive(Clone, Debug)]
pub(crate) struct PasswordHasher {
    /// One permit per password that may be hashed at this moment. Hashing takes a processor
    /// and about 19 MiB each, so without a limit a burst of sign-ins could use up both.
    pub(crate) slots: Arc<Semaphore>,
    /// The hash of nobody's password, checked when there is no real one, so that the time a
    /// check takes does not tell whether there was.
    dummy_hash: String,
}

impl PasswordHasher {
    /// Fails only if the password hasher cannot run.
    pub(crate) fn new() -> Result<Self, AuthError> {
        Ok(Self {
            slots: Arc::new(Semaphore::new(
                available_parallelism().map_or(FALLBACK_HASHING_SLOTS, usize::from),
            )),
            dummy_hash: password::hash(DUMMY_PASSWORD)?,
        })
    }

    /// Checks a new password against the rules and hashes it.
    pub(crate) async fn hash_new(&self, password: &str) -> Result<String, AuthError> {
        password::check_new(password)?;
        self.hash(password).await
    }

    pub(crate) async fn hash(&self, password: &str) -> Result<String, AuthError> {
        let turn = self.turn().await?;
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || {
            let _turn = turn;
            password::hash(&password)
        })
        .await?
    }

    pub(crate) async fn verify(&self, password: &str, hash: String) -> Result<bool, AuthError> {
        let turn = self.turn().await?;
        let password = password.to_owned();
        let verified = tokio::task::spawn_blocking(move || {
            let _turn = turn;
            password::verify(&password, &hash)
        })
        .await?;
        Ok(verified)
    }

    /// Whether `password` matches `hash`; without a hash it is checked against nobody's, which
    /// takes as long, and never matches.
    pub(crate) async fn verify_or_pretend(
        &self,
        password: &str,
        hash: Option<String>,
    ) -> Result<bool, AuthError> {
        let real = hash.is_some();
        let hash = hash.unwrap_or_else(|| self.dummy_hash.clone());
        Ok(self.verify(password, hash).await? && real)
    }

    /// Waits for a turn to hash a password, which lasts until the permit is dropped. It is
    /// handed to the hashing thread, so a request that is given up does not free its turn
    /// while the processor is still at work for it.
    async fn turn(&self) -> Result<OwnedSemaphorePermit, AuthError> {
        turn(&self.slots, HASHING_WAIT).await
    }
}

/// A permit of `slots`, or [`AuthError::Busy`] when none comes free within `wait`.
pub(crate) async fn turn(
    slots: &Arc<Semaphore>,
    wait: Duration,
) -> Result<OwnedSemaphorePermit, AuthError> {
    match tokio::time::timeout(wait, Arc::clone(slots).acquire_owned()).await {
        Ok(Ok(turn)) => Ok(turn),
        // The semaphore is never closed, so this is always the wait running out.
        _ => Err(AuthError::Busy {
            retry_after: wait.as_secs().max(1),
        }),
    }
}
