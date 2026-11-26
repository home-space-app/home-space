//! What a session sees of the names inside password folders.
//!
//! The server seals those names with the folder's key whatever the session (see
//! `files::Names`). A session reads or writes inside a password folder only once it unlocked
//! it: [`VaultService::names`] answers `Locked` otherwise. Feeds that cover the whole tree, the
//! journal of changes and the live events, leave out what is inside a folder the session has
//! not unlocked, and do not extend the unlock, so a client that only listens lets it expire.

use std::collections::HashMap;

use time::OffsetDateTime;
use uuid::Uuid;

use db::{NodeId, NodeKind, UserId};
use files::Names;

use super::{FOLDER_UNLOCK_TTL, VaultService};
use crate::AccountError;

/// What one session may see of the nodes in a set of folders, from [`VaultService::shown`].
#[derive(Debug, Default)]
pub struct Shown {
    /// The password folder each folder that is one, or is inside one, belongs to.
    holders: HashMap<NodeId, NodeId>,
    /// The names of each of those password folders, `None` while the session has it locked.
    folders: HashMap<NodeId, Option<Names>>,
}

impl Shown {
    /// The name the session sees for a node of `kind` in `parent`, stored as `stored`; `None`
    /// when it is inside a password folder the session has not unlocked.
    pub fn name(&self, parent: Option<NodeId>, kind: NodeKind, stored: &str) -> Option<String> {
        match parent.and_then(|parent| self.holders.get(&parent)) {
            None => Some(stored.to_owned()),
            Some(folder) => self
                .folders
                .get(folder)
                .and_then(Option::as_ref)
                .map(|names| names.show(kind, stored)),
        }
    }
}

impl VaultService {
    /// How the names of the nodes in `parent` (the root when `None`) are stored, for a read or a
    /// write by `session`; `Locked` when `parent` is a password folder, or inside one, that the
    /// session has not unlocked. Using the folder keeps it unlocked.
    pub async fn names(
        &self,
        user: UserId,
        session: Uuid,
        parent: Option<NodeId>,
    ) -> Result<Names, AccountError> {
        let Some(parent) = parent else {
            return Ok(Names::Plain);
        };
        let Some(folder) = self.db.enclosing_password_folder(user, parent).await? else {
            return Ok(Names::Plain);
        };
        let extend_to = OffsetDateTime::now_utc() + FOLDER_UNLOCK_TTL;
        if !self
            .db
            .use_unlocked_folder(session, folder.node_id, extend_to)
            .await?
        {
            return Err(AccountError::Locked);
        }
        self.folder_names(user, folder.node_id).await
    }

    /// The names inside the password folder `folder`, for the server's own work on it.
    pub async fn folder_names(&self, user: UserId, folder: NodeId) -> Result<Names, AccountError> {
        let keys = self.folder_keys(user, folder).await?;
        Ok(Names::sealed(folder, keys))
    }

    /// What `session` may see of the nodes in the user's folders `parents`, for feeds that
    /// cover the whole tree.
    pub async fn shown(
        &self,
        user: UserId,
        session: Uuid,
        parents: &[NodeId],
    ) -> Result<Shown, AccountError> {
        if parents.is_empty() || self.db.password_folders(user).await?.is_empty() {
            return Ok(Shown::default());
        }
        let holders: HashMap<NodeId, NodeId> = self
            .db
            .password_folders_over(user, parents)
            .await?
            .into_iter()
            .collect();
        if holders.is_empty() {
            return Ok(Shown::default());
        }
        let unlocked = self.db.unlocked_folders(session).await?;
        let mut folders = HashMap::new();
        for &folder in holders.values() {
            if folders.contains_key(&folder) {
                continue;
            }
            let names = if unlocked.contains(&folder) {
                Some(self.folder_names(user, folder).await?)
            } else {
                None
            };
            folders.insert(folder, names);
        }
        Ok(Shown { holders, folders })
    }
}
