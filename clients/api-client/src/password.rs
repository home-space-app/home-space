//! Changing the signed-in user's own password (`/me/password`).

use protocol::{ChangePasswordRequest, paths};

use crate::{ApiError, Client, client::expect_success};

impl Client {
    /// Replaces the signed-in user's password. This session stays signed in; the user's other
    /// sessions end. A wrong `current` password is a `BadRequest`, not `Unauthorized`.
    pub async fn change_password(&self, current: &str, new: &str) -> Result<(), ApiError> {
        let request = ChangePasswordRequest {
            current_password: current.to_owned(),
            new_password: new.to_owned(),
        };
        let response = self
            .send(|| self.http().post(self.url(paths::ME_PASSWORD)).json(&request))
            .await?;
        expect_success(response).await
    }
}
