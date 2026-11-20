//! Files sharing their content (`/duplicates`).

use protocol::{DuplicatesReport, paths};

use crate::{ApiError, Client};

impl Client {
    /// The signed-in user's identical files, and files holding an old version of another.
    pub async fn duplicates(&self) -> Result<DuplicatesReport, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::DUPLICATES)))
            .await
    }
}
