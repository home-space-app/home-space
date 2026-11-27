//! The vault of protected folders (feature [`PROTECTED_FOLDERS`](protocol::features::PROTECTED_FOLDERS)):
//! its folder keys and recovery key, all wrapped by keys the server never sees.
//!
//! A folder key is a credential the client makes on a security key for the vault alone, with
//! a challenge of its own; it never signs in. The server keeps what a client needs to ask the
//! key again: its credential id, RP id and PRF salt.

use protocol::{AddUnlocker, ErrorCode, Vault, VaultUnlocker, paths};
use uuid::Uuid;

use crate::{ApiError, Client, client::expect_success};

impl Client {
    /// The signed-in user's vault; `None` before they enabled a folder key or a recovery key.
    pub async fn vault(&self) -> Result<Option<Vault>, ApiError> {
        match self
            .send_json(|| self.http().get(self.url(paths::VAULT)))
            .await
        {
            Err(ApiError::Api {
                code: ErrorCode::NotFound,
                ..
            }) => Ok(None),
            found => found.map(Some),
        }
    }

    /// Enables a folder key or sets the recovery key, wrapped under the vault's epoch (1 for
    /// the first, which creates the vault). Needs a recent sign-in
    /// ([`ApiError::ReauthenticationRequired`]).
    pub async fn add_unlocker(&self, request: &AddUnlocker) -> Result<VaultUnlocker, ApiError> {
        self.send_json(|| {
            self.http()
                .post(self.url(paths::VAULT_UNLOCKERS))
                .json(request)
        })
        .await
    }

    /// Removes a folder key or the recovery key; refused while it is the last unlocker of
    /// protected folders. Needs a recent sign-in.
    pub async fn remove_unlocker(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(&paths::vault_unlocker(id))))
            .await?;
        expect_success(response).await
    }
}
