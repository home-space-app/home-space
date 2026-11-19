//! What owners do with their shares.

use std::collections::HashSet;

use uuid::Uuid;

use db::{
    NewShareLink, NewUserShare, NodeId, OwnedShare, Patch, SharePermission, ShareUpdate, UserId,
};
use protocol::{CreateShare, UpdateShare};

use crate::{ShareDetails, ShareError, ShareService};

impl ShareService {
    /// Shares the owner's items through a link, or with named users when the request names
    /// any. The request's own limits were checked when it was read.
    pub async fn create(
        &self,
        owner: UserId,
        request: &CreateShare,
    ) -> Result<ShareDetails, ShareError> {
        let node_ids = unique(&request.node_ids);
        let share = if request.recipients.is_empty() {
            self.create_link(owner, request, &node_ids).await?
        } else {
            let recipients = recipients(owner, &request.recipients)?;
            let new = NewUserShare {
                id: Uuid::now_v7(),
                expires_at: request.expires_at,
            };
            self.db
                .create_user_share(owner, new, &node_ids, &recipients)
                .await?
        };
        self.details(share).await
    }

    async fn create_link(
        &self,
        owner: UserId,
        request: &CreateShare,
        node_ids: &[NodeId],
    ) -> Result<OwnedShare, ShareError> {
        let password_hash = match request.password.as_deref() {
            Some(password) => Some(self.passwords.hash(password).await?),
            None => None,
        };
        let token = auth::new_share_token();
        let new = NewShareLink {
            id: Uuid::now_v7(),
            token: &token,
            password_hash: password_hash.as_deref(),
            permission: SharePermission::Read,
            expires_at: request.expires_at,
        };
        Ok(self
            .db
            .create_share_link(owner, new, node_ids)
            .await?
            .into())
    }

    /// The owner's shares, or only those holding `node` when one is named.
    pub async fn list(
        &self,
        owner: UserId,
        node: Option<NodeId>,
    ) -> Result<Vec<ShareDetails>, ShareError> {
        let shares = self.db.owned_shares(owner, node).await?;
        self.all_details(shares).await
    }

    /// Changes a share. A link takes a password but no recipients, a share with users the
    /// other way round.
    pub async fn update(
        &self,
        owner: UserId,
        id: Uuid,
        request: &UpdateShare,
    ) -> Result<ShareDetails, ShareError> {
        let is_link = self.db.owned_share(owner, id).await?.token.is_some();
        if is_link && request.recipients.is_some() {
            return Err(ShareError::LinkWithRecipients);
        }
        if !is_link && !request.password.is_keep() {
            return Err(ShareError::PasswordForUsers);
        }
        let recipients = request
            .recipients
            .as_deref()
            .map(|named| recipients(owner, named))
            .transpose()?;
        let password_hash = match &request.password {
            Patch::Set(password) => Patch::Set(self.passwords.hash(password).await?),
            Patch::Clear => Patch::Clear,
            Patch::Keep => Patch::Keep,
        };
        let update = ShareUpdate {
            password_hash: password_hash.as_ref().map(String::as_str),
            expires_at: request.expires_at,
            recipients: recipients.as_deref(),
        };
        let share = self.db.update_share(owner, id, update).await?;
        self.details(share).await
    }

    pub async fn delete(&self, owner: UserId, id: Uuid) -> Result<(), ShareError> {
        Ok(self.db.delete_share(owner, id).await?)
    }

    async fn details(&self, share: OwnedShare) -> Result<ShareDetails, ShareError> {
        self.all_details(vec![share])
            .await?
            .pop()
            .ok_or(ShareError::NotFound)
    }

    /// `shares` with their live items and recipients.
    async fn all_details(&self, shares: Vec<OwnedShare>) -> Result<Vec<ShareDetails>, ShareError> {
        let ids: Vec<Uuid> = shares.iter().map(|share| share.id).collect();
        let mut items = self.db.share_items(&ids).await?;
        let mut recipients = self.db.share_recipients(&ids).await?;
        Ok(shares
            .into_iter()
            .map(|share| ShareDetails {
                items: items.remove(&share.id).unwrap_or_default(),
                recipients: recipients.remove(&share.id).unwrap_or_default(),
                share,
            })
            .collect())
    }
}

/// `ids` without repeats, in their first order.
fn unique<T: Copy + Eq + std::hash::Hash>(ids: &[T]) -> Vec<T> {
    let mut seen = HashSet::new();
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

/// `recipients` without repeats; refuses the owner. Their number was checked with the request.
fn recipients(owner: UserId, recipients: &[UserId]) -> Result<Vec<UserId>, ShareError> {
    let recipients = unique(recipients);
    if recipients.contains(&owner) {
        Err(ShareError::OwnRecipient)
    } else {
        Ok(recipients)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_items_are_shared_once_in_their_first_order() {
        let a = NodeId::new();
        let b = NodeId::new();
        assert_eq!(unique(&[a, b, a, b]), vec![a, b]);
    }

    #[test]
    fn a_share_is_never_for_its_owner() {
        let owner = UserId::new();
        assert!(matches!(
            recipients(owner, &[UserId::new(), owner]),
            Err(ShareError::OwnRecipient)
        ));
    }
}
