//! The signed-in user's own password: changing it (`/me/password`) and confirming it is them
//! with it (`/me/reauthenticate`).

use protocol::{ChangePasswordRequest, ReauthenticateRequest, paths};

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
            .send(|| {
                self.http()
                    .post(self.url(paths::ME_PASSWORD))
                    .json(&request)
            })
            .await?;
        expect_success(response).await
    }

    /// Confirms it is the signed-in user with their password, which allows sensitive changes
    /// from this session for the next ten minutes, after the server answered one with
    /// `reauthentication-required`. A wrong password is a `BadRequest`, not `Unauthorized`.
    /// Needs [`REAUTHENTICATION`](protocol::features::REAUTHENTICATION).
    pub async fn reauthenticate(&self, password: &str) -> Result<(), ApiError> {
        let request = ReauthenticateRequest {
            password: password.to_owned(),
        };
        let response = self
            .send(|| {
                self.http()
                    .post(self.url(paths::ME_REAUTHENTICATE))
                    .json(&request)
            })
            .await?;
        expect_success(response).await
    }
}
