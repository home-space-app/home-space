//! What a transfer holds: its items, checked, and the sender's files linked into it.

use std::collections::HashMap;

use uuid::Uuid;

use db::{NodeKind, TransferItem, UserId};

use super::TransferService;
use crate::{FilesError, MAX_TRANSFER_ITEMS};

const MAX_PATH_LEN: usize = 4096;
const MAX_NAME_CHARS: usize = 255;
/// How often a file of the sender's tree is linked again when it changes while being linked.
const LINK_ATTEMPTS: usize = 3;

impl TransferService {
    /// Turns the requested items into the transfer's items, expanding folders of the
    /// sender's tree into everything in them.
    pub(super) async fn manifest(
        &self,
        sender: UserId,
        transfer: Uuid,
        requested: Vec<protocol::NewTransferItem>,
    ) -> Result<Vec<TransferItem>, FilesError> {
        if requested.is_empty() {
            return Err(invalid("a transfer needs at least one item".to_owned()));
        }
        let mut items = Vec::with_capacity(requested.len());
        for request in requested {
            if !valid_path(&request.path) {
                return Err(invalid(format!("invalid path: {}", request.path)));
            }
            match request.node_id {
                Some(node) => {
                    for entry in self.content.db.subtree(sender, node).await? {
                        let node = entry.node;
                        let is_file = node.kind == NodeKind::File;
                        items.push(TransferItem {
                            id: Uuid::now_v7(),
                            transfer_id: transfer,
                            path: format!("{}{}", request.path, entry.rel),
                            kind: node.kind,
                            size: if is_file { node.size } else { 0 },
                            sha256: None,
                            mime: is_file.then_some(node.mime),
                            modified_at: Some(node.updated_at),
                            node_id: Some(node.id),
                        });
                    }
                }
                None => items.push(TransferItem {
                    id: Uuid::now_v7(),
                    transfer_id: transfer,
                    path: request.path,
                    kind: request.kind,
                    size: match request.kind {
                        NodeKind::File => i64::try_from(request.size).unwrap_or(i64::MAX),
                        NodeKind::Folder => 0,
                    },
                    sha256: None,
                    mime: request.mime.filter(|_| request.kind == NodeKind::File),
                    modified_at: request.modified_at,
                    node_id: None,
                }),
            }
            if items.len() > MAX_TRANSFER_ITEMS {
                return Err(invalid(format!(
                    "a transfer holds at most {MAX_TRANSFER_ITEMS} items"
                )));
            }
        }
        Ok(items)
    }

    /// Links the files taken from the sender's tree into the transfer and records their
    /// hashes. A file replaced while it is linked is linked again.
    pub(super) async fn link_items(
        &self,
        sender: UserId,
        transfer: Uuid,
        items: &mut [TransferItem],
    ) -> Result<(), FilesError> {
        for item in items.iter_mut() {
            let Some(node) = item.node_id.filter(|_| item.kind == NodeKind::File) else {
                continue;
            };
            let mut linked = false;
            for _ in 0..LINK_ATTEMPTS {
                let before = self.content.db.get_node(sender, node).await?;
                self.content
                    .store
                    .link_into_transfer(sender, node, transfer, item.id)
                    .await?;
                let after = self.content.db.get_node(sender, node).await?;
                if before.version == after.version && before.sha256 == after.sha256 {
                    item.size = after.size;
                    item.sha256 = Some(after.sha256.unwrap_or_default());
                    item.mime = Some(after.mime);
                    linked = true;
                    break;
                }
            }
            if !linked {
                return Err(FilesError::Conflict);
            }
        }
        Ok(())
    }
}

pub(super) fn invalid(message: String) -> FilesError {
    FilesError::InvalidTransfer(message)
}

/// Checks that paths are unique and every folder in a path is an item of the transfer.
pub(super) fn check_manifest(items: &[TransferItem]) -> Result<(), FilesError> {
    let mut kinds = HashMap::with_capacity(items.len());
    for item in items {
        if !valid_path(&item.path) {
            return Err(invalid(format!("invalid path: {}", item.path)));
        }
        if kinds.insert(item.path.as_str(), item.kind).is_some() {
            return Err(invalid(format!("the path {} is listed twice", item.path)));
        }
    }
    for item in items {
        if let (Some(dir), _) = split_path(&item.path)
            && kinds.get(dir) != Some(&NodeKind::Folder)
        {
            return Err(invalid(format!(
                "the folder {dir} of {} is not an item of the transfer",
                item.path
            )));
        }
    }
    Ok(())
}

/// Whether `path` is relative, `/`-separated and every part of it is a valid node name.
pub(crate) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_LEN
        && !path.contains('\\')
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.chars().count() <= MAX_NAME_CHARS
                && !part.chars().any(char::is_control)
        })
}

/// The folder part and the name of a path.
pub(super) fn split_path(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (Some(dir), name),
        None => (None, path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_must_be_relative_and_clean() {
        for good in ["a.txt", "Photos/2026/a.jpg", "a b/c"] {
            assert!(valid_path(good), "{good}");
        }
        for bad in [
            "", "/a", "a/", "a//b", "../a", "a/../b", "./a", "a\\b", "a/\u{0}b",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(!valid_path(&"a".repeat(256)));
    }

    fn item(path: &str, kind: NodeKind) -> TransferItem {
        TransferItem {
            id: Uuid::nil(),
            transfer_id: Uuid::nil(),
            path: path.to_owned(),
            kind,
            size: 0,
            sha256: None,
            mime: None,
            modified_at: None,
            node_id: None,
        }
    }

    #[test]
    fn every_folder_of_a_path_must_be_an_item() {
        let folder = item("Photos", NodeKind::Folder);
        let photo = item("Photos/a.jpg", NodeKind::File);
        assert!(check_manifest(&[folder.clone(), photo.clone()]).is_ok());
        assert!(check_manifest(std::slice::from_ref(&photo)).is_err());
        assert!(check_manifest(&[item("Photos", NodeKind::File), photo]).is_err());
        assert!(check_manifest(&[folder.clone(), folder]).is_err());
    }
}
