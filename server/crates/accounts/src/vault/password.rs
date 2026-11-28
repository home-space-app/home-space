//! A password folder's new password: its secret wrapped again by the client.

use uuid::Uuid;

use db::{NewPassword, NodeId, UserId};
use protocol::{ChangeFolderPassword, ProtectedFolder};

use super::{VaultService, check_kdf, check_wrapped, protect::folder_of};
use crate::AccountError;

impl VaultService {
    /// Stores the password folder `node`'s secret wrapped for a new password, and for a new
    /// recovery key when the client replaced it. Only a client that opened the folder, with its
    /// password or recovery key, can wrap the secret; the server cannot tell, so it needs a
    /// recent sign-in and leaves the folder's recipient as it is. A client that unwraps a
    /// secret checks it against that recipient.
    pub async fn change_password(
        &self,
        user: UserId,
        session: Uuid,
        node: NodeId,
        request: &ChangeFolderPassword,
    ) -> Result<ProtectedFolder, AccountError> {
        self.auth.require_recent_sign_in(user, session).await?;
        check_wrapped("/wrapped_secret", &request.wrapped_secret)?;
        let kdf = check_kdf("/kdf", &request.kdf)?;
        if let Some(recovery) = &request.recovery_wrapped_secret {
            check_wrapped("/recovery_wrapped_secret", recovery)?;
        }
        let password = NewPassword {
            wrapped_secret: request.wrapped_secret.as_bytes(),
            kdf: &kdf,
            recovery_wrapped_secret: request
                .recovery_wrapped_secret
                .as_ref()
                .map(|wrapped| wrapped.as_bytes()),
        };
        let stored = self.db.set_folder_password(user, node, password).await?;
        tracing::info!(user = %user, folder = %node, recovered = request.recovery_wrapped_secret.is_some(), "password folder's password changed");
        self.changed(user, node, Some(stored.state)).await;
        Ok(folder_of(stored))
    }
}
