//! Account management by administrators (`/admin/users`).

use protocol::{Account, AccountDevice, CreateAccount, SetPassword, UpdateAccount, UserId, paths};

use crate::{ApiError, Client, client::expect_success};

impl Client {
    /// Every account on the server, by username; administrators only.
    pub async fn accounts(&self) -> Result<Vec<Account>, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::ADMIN_USERS)))
            .await
    }

    /// Creates an account that signs in with a password.
    pub async fn create_account(&self, request: &CreateAccount) -> Result<Account, ApiError> {
        self.send_json(|| self.http().post(self.url(paths::ADMIN_USERS)).json(request))
            .await
    }

    /// Changes an account's role, state or quota.
    pub async fn update_account(
        &self,
        id: UserId,
        request: &UpdateAccount,
    ) -> Result<Account, ApiError> {
        self.send_json(|| {
            self.http()
                .patch(self.url(&paths::admin_user(id)))
                .json(request)
        })
        .await
    }

    /// Sets a new password for an account that has one; ends all its sessions.
    pub async fn set_account_password(&self, id: UserId, password: &str) -> Result<(), ApiError> {
        let request = SetPassword {
            password: password.to_owned(),
        };
        let response = self
            .send(|| {
                self.http()
                    .put(self.url(&paths::admin_user_password(id)))
                    .json(&request)
            })
            .await?;
        expect_success(response).await
    }

    /// The devices of an account, the most recently seen first.
    pub async fn account_devices(&self, id: UserId) -> Result<Vec<AccountDevice>, ApiError> {
        self.send_json(|| self.http().get(self.url(&paths::admin_user_devices(id))))
            .await
    }

    /// Marks an account for deletion; the server deletes it and its files after a delay.
    pub async fn delete_account(&self, id: UserId) -> Result<Account, ApiError> {
        self.send_json(|| self.http().delete(self.url(&paths::admin_user(id))))
            .await
    }

    /// Takes back an account's pending deletion.
    pub async fn cancel_account_deletion(&self, id: UserId) -> Result<Account, ApiError> {
        self.send_json(|| {
            self.http()
                .delete(self.url(&paths::admin_user_deletion(id)))
        })
        .await
    }
}
