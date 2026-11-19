//! What visitors of a link see. `access` is the proof of the link's password the visitor
//! presented, if any.

use std::net::IpAddr;

use tokio::fs;

use db::{Node, NodeId, ShareLink};
use files::ArchiveEntry;
use protocol::ShareAccess;

use crate::{LinkView, ShareError, ShareService, service::live};

/// No token is longer; anything longer is refused before the database is asked.
const MAX_TOKEN_LEN: usize = 128;

impl ShareService {
    /// The link of `token` of the tenant: `NotFound` when there is none, `Gone` once it has
    /// expired.
    async fn link_of(&self, token: &str) -> Result<ShareLink, ShareError> {
        if token.len() > MAX_TOKEN_LEN {
            return Err(ShareError::NotFound);
        }
        live(self.db.share_by_token(self.tenant(), token).await?)
    }

    /// Whether `access` lets the visitor read `link`: it has no password, or `access` proves
    /// its current one.
    fn is_unlocked(&self, link: &ShareLink, access: Option<&str>) -> bool {
        link.password_hash.is_none()
            || access.is_some_and(|access| self.passwords.verifies(link, access))
    }

    /// The link of `token`, when the visitor may read it.
    async fn readable_link(
        &self,
        token: &str,
        access: Option<&str>,
    ) -> Result<ShareLink, ShareError> {
        let link = self.link_of(token).await?;
        if self.is_unlocked(&link, access) {
            Ok(link)
        } else {
            Err(ShareError::Locked)
        }
    }

    /// The link of `token` as a visitor sees it: who shared it, and its items once unlocked.
    pub async fn link(&self, token: &str, access: Option<&str>) -> Result<LinkView, ShareError> {
        let link = self.link_of(token).await?;
        let items = if self.is_unlocked(&link, access) {
            let mut items = self.db.share_items(&[link.id]).await?;
            Some(items.remove(&link.id).unwrap_or_default())
        } else {
            None
        };
        Ok(LinkView { link, items })
    }

    /// Unlocks the link of `token` with its password, for a visitor at `address`.
    pub async fn unlock(
        &self,
        token: &str,
        password: &str,
        address: Option<IpAddr>,
    ) -> Result<ShareAccess, ShareError> {
        let link = self.link_of(token).await?;
        Ok(self.passwords.unlock(&link, password, address).await?)
    }

    /// The children of the folder `parent` inside the link.
    pub async fn link_children(
        &self,
        token: &str,
        access: Option<&str>,
        parent: NodeId,
    ) -> Result<Vec<Node>, ShareError> {
        let link = self.readable_link(token, access).await?;
        self.children(&link, parent).await
    }

    /// A file of the link, opened for download.
    pub async fn link_file(
        &self,
        token: &str,
        access: Option<&str>,
        id: NodeId,
    ) -> Result<(Node, fs::File), ShareError> {
        let link = self.readable_link(token, access).await?;
        self.file(&link, id).await
    }

    /// Everything the link shares, or one folder inside it, with the owner's name to name
    /// the archive after.
    pub async fn link_archive(
        &self,
        token: &str,
        access: Option<&str>,
        folder: Option<NodeId>,
    ) -> Result<(String, Vec<ArchiveEntry>), ShareError> {
        let link = self.readable_link(token, access).await?;
        let entries = self.archive(&link, folder).await?;
        Ok((link.owner_name, entries))
    }
}
