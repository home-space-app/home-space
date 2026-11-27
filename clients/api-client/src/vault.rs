//! The vault of protected folders (feature [`PROTECTED_FOLDERS`](protocol::features::PROTECTED_FOLDERS)):
//! its folder keys and recovery key, all wrapped by keys the server never sees.
//!
//! A folder key is a credential the client makes on a security key for the vault alone, with
//! a challenge of its own; it never signs in. The server keeps what a client needs to ask the
//! key again: its credential id, RP id and PRF salt.

use protocol::{
    AddUnlocker, ChangeFolderPassword, ErrorCode, NodeId, ProtectFolder, ProtectedFolder,
    RotateVault, Vault, VaultUnlocker, paths,
};
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

    /// Replaces the vault key: every unlocker that stays and every security-key folder,
    /// wrapped anew under the next epoch, in one call. Needs a recent sign-in.
    pub async fn rotate_vault(&self, request: &RotateVault) -> Result<Vault, ApiError> {
        self.send_json(|| {
            self.http()
                .post(self.url(paths::VAULT_ROTATE))
                .json(request)
        })
        .await
    }

    /// The signed-in user's protected folders, of both locks, with what unlocks them.
    pub async fn protected_folders(&self) -> Result<Vec<ProtectedFolder>, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::PROTECTED)))
            .await
    }

    /// Protects a folder with a secret the client made and wrapped. An empty folder is
    /// protected at once, one with content is protecting until [`Client::finish_protection`].
    /// Needs a recent sign-in.
    pub async fn protect_folder(
        &self,
        request: &ProtectFolder,
    ) -> Result<ProtectedFolder, ApiError> {
        self.send_json(|| self.http().post(self.url(paths::PROTECTED)).json(request))
            .await
    }

    /// Marks a folder whose content the client has replaced with its encrypted form as protected.
    pub async fn finish_protection(&self, id: NodeId) -> Result<ProtectedFolder, ApiError> {
        self.send_json(|| self.http().post(self.url(&paths::protected_finish(id))))
            .await
    }

    /// Starts unprotecting a folder: the client then replaces its content with the decrypted
    /// form and calls [`Client::remove_protection`]. Needs a recent sign-in.
    pub async fn unprotect_folder(&self, id: NodeId) -> Result<ProtectedFolder, ApiError> {
        self.send_json(|| self.http().post(self.url(&paths::protected_unprotect(id))))
            .await
    }

    /// Forgets the protection of a folder that is being protected or unprotected. Needs a
    /// recent sign-in.
    pub async fn remove_protection(&self, id: NodeId) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(&paths::protected_folder(id))))
            .await?;
        expect_success(response).await
    }

    /// Stores a password folder's secret wrapped for a new password. Needs a recent sign-in.
    pub async fn change_folder_password(
        &self,
        id: NodeId,
        request: &ChangeFolderPassword,
    ) -> Result<ProtectedFolder, ApiError> {
        self.send_json(|| {
            self.http()
                .post(self.url(&paths::protected_password(id)))
                .json(request)
        })
        .await
    }
}
