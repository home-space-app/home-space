//! How names are stored: as they are, or sealed inside a password folder.
//!
//! The server keeps the names inside a password folder sealed with the folder's name key
//! (`shared/vault-core`, the same deterministic AES-SIV as security-key folders), so the
//! database never holds them in plain. Sealing is deterministic, so the unique index on names
//! still refuses two equal names in one folder. The associated data is the protected folder's
//! id rather than the direct parent's, so a node moving within the folder keeps its sealed name
//! and folder merges still find same-named folders.
//!
//! The tree logic works on plain names: [`Names::store`] seals a name right before it is
//! written or looked up, and [`Names::show`] opens one on its way to a client. Both are done by
//! callers that know whether the caller's session may see the folder.

use std::sync::Arc;

use db::{EnclosingPasswordFolder, Node, NodeId, NodeKind};
use protocol::ProtectionState;
use vault_core::FolderKeys;

use crate::FilesError;

/// Why a write is refused whose names were chosen for another protection than the folder has
/// by the time it is written. Choosing them again gets it right.
const PROTECTION_CHANGED: &str = "The folder was protected or unprotected meanwhile. Try again.";

/// How the names of the nodes in one folder are stored.
#[derive(Clone, Debug, Default)]
pub enum Names {
    /// As they are: the folder is not inside a password folder.
    #[default]
    Plain,
    /// Sealed with the keys of the password folder that holds it.
    Sealed(Arc<SealedNames>),
}

/// The keys of the password folder whose names are sealed.
#[derive(Debug)]
pub struct SealedNames {
    folder: NodeId,
    keys: FolderKeys,
    /// The folder is being unprotected: names are opened, none are stored any more.
    read_only: bool,
}

impl Names {
    /// The names inside the password folder `folder`, with its keys.
    pub fn sealed(folder: NodeId, keys: FolderKeys) -> Self {
        Self::Sealed(Arc::new(SealedNames {
            folder,
            keys,
            read_only: false,
        }))
    }

    /// The names inside the password folder `folder` while it is being unprotected: they still
    /// open, but nothing new may be named there until it is done.
    pub fn unsealing(folder: NodeId, keys: FolderKeys) -> Self {
        Self::Sealed(Arc::new(SealedNames {
            folder,
            keys,
            read_only: true,
        }))
    }

    /// The password folder the names are sealed for, if any.
    pub fn folder(&self) -> Option<NodeId> {
        match self {
            Self::Plain => None,
            Self::Sealed(sealed) => Some(sealed.folder),
        }
    }

    /// The password folder's keys, if the names are sealed.
    pub(crate) fn keys(&self) -> Option<&FolderKeys> {
        match self {
            Self::Plain => None,
            Self::Sealed(sealed) => Some(&sealed.keys),
        }
    }

    /// The name to store, or to look up, for a node of `kind` called `name`.
    pub fn store(&self, kind: NodeKind, name: &str) -> Result<String, FilesError> {
        match self {
            Self::Plain => Ok(name.to_owned()),
            Self::Sealed(sealed) if sealed.read_only => Err(FilesError::Protected(
                "The folder is being unprotected. Try again once it is done.",
            )),
            Self::Sealed(sealed) => sealed
                .keys
                .seal_name(&sealed.folder.0, sealing_kind(kind), name)
                .map_err(|_| FilesError::NameTooLongToProtect),
        }
    }

    /// The name a client sees for a node of `kind` stored as `stored`. A name that does not
    /// open is shown as it is stored: one the server has not sealed yet, while the folder is
    /// still being protected.
    pub fn show(&self, kind: NodeKind, stored: &str) -> String {
        match self {
            Self::Plain => stored.to_owned(),
            Self::Sealed(sealed) => sealed
                .keys
                .open_name(&sealed.folder.0, sealing_kind(kind), stored)
                .unwrap_or_else(|_| stored.to_owned()),
        }
    }

    /// Refuses a write into a folder that `holder` holds now, the password folder read under
    /// the transaction's hold on protections (see [`db::Tx::hold_protections`]), unless these
    /// names are still that folder's: it was protected, or stopped being, since they were
    /// chosen, and they would land in the wrong form.
    pub(crate) fn check_holder(
        &self,
        holder: Option<&EnclosingPasswordFolder>,
    ) -> Result<(), FilesError> {
        let current = match (self, holder) {
            (Self::Plain, None) => true,
            (Self::Sealed(sealed), Some(holder)) => {
                holder.node_id == sealed.folder
                    && (sealed.read_only || holder.state != ProtectionState::Unprotecting)
            }
            _ => false,
        };
        if current {
            Ok(())
        } else {
            Err(FilesError::Protected(PROTECTION_CHANGED))
        }
    }

    /// Whether `stored` is already sealed with these names' keys.
    pub fn is_stored(&self, kind: NodeKind, stored: &str) -> bool {
        match self {
            Self::Plain => true,
            Self::Sealed(sealed) => sealed
                .keys
                .open_name(&sealed.folder.0, sealing_kind(kind), stored)
                .is_ok(),
        }
    }

    /// `node` with the name a client sees.
    pub fn show_node(&self, mut node: Node) -> Node {
        if let Self::Sealed(_) = self {
            node.name = self.show(node.kind, &node.name);
        }
        node
    }

    /// `nodes` of one folder with the names a client sees, in the order the database lists a
    /// folder: by kind, then by name.
    pub fn show_listing(&self, nodes: Vec<Node>) -> Vec<Node> {
        if let Self::Plain = self {
            return nodes;
        }
        let mut nodes: Vec<Node> = nodes.into_iter().map(|node| self.show_node(node)).collect();
        // The database sorted the sealed names; keep its grouping by kind and sort each group by
        // the plain name.
        for group in nodes.chunk_by_mut(|a, b| a.kind == b.kind) {
            group.sort_by(|a, b| a.name.cmp(&b.name));
        }
        nodes
    }
}

fn sealing_kind(kind: NodeKind) -> vault_core::NodeKind {
    match kind {
        NodeKind::Folder => vault_core::NodeKind::Folder,
        NodeKind::File => vault_core::NodeKind::File,
    }
}

#[cfg(test)]
mod tests {
    use vault_core::FolderSecret;

    use super::*;

    fn sealed() -> (Names, NodeId) {
        let folder = NodeId::new();
        (
            Names::sealed(folder, FolderSecret::from_raw([7; 32]).keys().unwrap()),
            folder,
        )
    }

    #[test]
    fn names_are_sealed_per_kind_and_open_again() {
        let (names, _) = sealed();
        let stored = names.store(NodeKind::File, "tax 2025.pdf").unwrap();
        assert_ne!(stored, "tax 2025.pdf");
        assert_eq!(names.store(NodeKind::File, "tax 2025.pdf").unwrap(), stored);
        assert_ne!(
            names.store(NodeKind::Folder, "tax 2025.pdf").unwrap(),
            stored
        );
        assert_eq!(names.show(NodeKind::File, &stored), "tax 2025.pdf");
        assert!(names.is_stored(NodeKind::File, &stored));
        assert!(!names.is_stored(NodeKind::Folder, &stored));
        assert!(!names.is_stored(NodeKind::File, "tax 2025.pdf"));
    }

    #[test]
    fn a_name_not_sealed_yet_shows_as_it_is() {
        let (names, _) = sealed();
        assert_eq!(names.show(NodeKind::File, "notes.txt"), "notes.txt");
        assert_eq!(Names::Plain.show(NodeKind::File, "notes.txt"), "notes.txt");
    }

    #[test]
    fn another_folder_cannot_open_the_names() {
        let (names, _) = sealed();
        let (other, _) = sealed();
        let stored = names.store(NodeKind::File, "a.txt").unwrap();
        assert!(!other.is_stored(NodeKind::File, &stored));
    }

    #[test]
    fn long_names_cannot_be_sealed() {
        let (names, _) = sealed();
        assert!(matches!(
            names.store(NodeKind::File, &"a".repeat(160)),
            Err(FilesError::NameTooLongToProtect)
        ));
        assert!(Names::Plain.store(NodeKind::File, &"a".repeat(160)).is_ok());
    }
}
