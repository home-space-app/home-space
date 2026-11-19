//! The disk the files are stored on.

use crate::AppState;

/// The disk of the storage root; a failure to read it is logged and reported as unknown.
pub(crate) async fn disk_space(state: &AppState) -> Option<protocol::DiskSpace> {
    state
        .files
        .store()
        .disk_space()
        .await
        .inspect_err(|err| tracing::warn!(error = %err, "reading disk space failed"))
        .ok()
        .flatten()
}
