//! Names inside a protected folder: sealed on the server, open only in the client's memory.

use protocol::{Node, NodeId, NodeKind, ProtectionState};
use vault_core::FolderKeys;

use super::{ProtectedError, shown_mime};

/// What a listing shows for a name that does not open: changed on the server, or written there
/// by a client without the keys.
pub const UNREADABLE_NAME: &str = "Unreadable name";

fn kind_of(kind: NodeKind) -> vault_core::NodeKind {
    match kind {
        NodeKind::Folder => vault_core::NodeKind::Folder,
        NodeKind::File => vault_core::NodeKind::File,
    }
}

/// Seals `name` for a node of `kind` in `parent`: the name to send to the server.
pub fn seal_name(
    keys: &FolderKeys,
    parent: NodeId,
    kind: NodeKind,
    name: &str,
) -> Result<String, ProtectedError> {
    Ok(keys.seal_name(&parent.0, kind_of(kind), name)?)
}

/// The name `node` was sealed from, or `None` when it does not open.
pub fn open_name(keys: &FolderKeys, node: &Node) -> Option<String> {
    let parent = node.parent_id?;
    keys.open_name(&parent.0, kind_of(node.kind), &node.name)
        .ok()
}

/// The path to a folder inside the protected folder `root`, in `state`, with the names below
/// `root` opened as [`open_listing`] does: `root` and what is above it have plain names.
pub fn open_path(
    keys: &FolderKeys,
    path: Vec<Node>,
    root: NodeId,
    state: ProtectionState,
) -> Vec<Node> {
    let below = path
        .iter()
        .position(|node| node.id == root)
        .map_or(path.len(), |at| at + 1);
    let mut above = path;
    let inside = above.split_off(below);
    above.extend(open_listing(keys, inside, state));
    above
}

/// `nodes`, of a protected folder in `state`, with their names opened. An opened file gets the
/// type [`shown_mime`] gives its name, since the server stores none. A name that does not open
/// is a plain one while the folder is being protected or unprotected, and is kept; in a
/// protected folder it shows as [`UNREADABLE_NAME`].
pub fn open_listing(keys: &FolderKeys, nodes: Vec<Node>, state: ProtectionState) -> Vec<Node> {
    nodes
        .into_iter()
        .map(|node| match open_name(keys, &node) {
            Some(name) => Node {
                mime: match node.kind {
                    NodeKind::File => shown_mime(&name).to_owned(),
                    NodeKind::Folder => node.mime,
                },
                name,
                ..node
            },
            None if state == ProtectionState::Protected => Node {
                name: UNREADABLE_NAME.to_owned(),
                ..node
            },
            None => node,
        })
        .collect()
}
