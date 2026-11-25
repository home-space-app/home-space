//! The signed-in user's own password: changing it (`/me/password`) and confirming it is them
//! with it (`/me/reauthenticate`).

use protocol::{ChangePasswordRequest, ReauthenticateRequest, paths};

use crate::{ApiError, Client, client::expect_success};

/// How the user confirms it is them: one of the `methods` of
/// [`ApiError::ReauthenticationRequired`].
#[derive(Clone, Copy, Debug)]
pub enum Confirmation<'a> {
    /// `password`: their current password.
    Password(&'a str),
    /// `totp`: a code from their authenticator app.
    Code(&'a str),
}

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

    /// Confirms it is the signed-in user, which allows sensitive changes from this session
    /// for the next ten minutes, after the server answered one with
    /// [`ApiError::ReauthenticationRequired`]. A wrong password or code is a `BadRequest`, not
    /// `Unauthorized`. Needs [`REAUTHENTICATION`](protocol::features::REAUTHENTICATION), and
    /// [`TWO_FACTOR`](protocol::features::TWO_FACTOR) for a code.
    pub async fn reauthenticate(&self, proof: Confirmation<'_>) -> Result<(), ApiError> {
        let request = match proof {
            Confirmation::Password(password) => ReauthenticateRequest {
                password: Some(password.to_owned()),
                code: None,
            },
            Confirmation::Code(code) => ReauthenticateRequest {
                password: None,
                code: Some(code.to_owned()),
            },
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
