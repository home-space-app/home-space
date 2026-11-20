//! Downloads that stop as soon as they are cancelled.

use std::ops::Deref;

use tokio::sync::watch;

use crate::{CoreError, last_cause};

/// `Cancelled` once `cancelled` is set.
pub(crate) fn not_cancelled(cancelled: &watch::Receiver<bool>) -> Result<(), CoreError> {
    if *cancelled.borrow() {
        Err(CoreError::Cancelled)
    } else {
        Ok(())
    }
}

/// The next chunk of `response`, `None` at its end; `Cancelled` as soon as `cancelled` is set,
/// even while waiting for the server.
pub(crate) async fn next_chunk(
    response: &mut reqwest::Response,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<Option<impl Deref<Target = [u8]> + AsRef<[u8]>>, CoreError> {
    let chunk = tokio::select! {
        biased;
        _ = cancelled.wait_for(|cancelled| *cancelled) => return Err(CoreError::Cancelled),
        chunk = response.chunk() => chunk,
    };
    chunk.map_err(|err| CoreError::Unreachable {
        detail: last_cause(&err.to_string()).to_owned(),
    })
}
