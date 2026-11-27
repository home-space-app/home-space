//! Keeping moves in step with the protection of folders. Inside a protected folder, of either
//! lock, everything is ciphertext the client made and outside one it is plain, so a move
//! across a protected folder's edge is refused. The move's transaction first holds the
//! owner's protections, so the folders checked stay protected as they are until it ends.

use db::{Node, NodeId, Tx};

use crate::FilesError;

/// Why a node may not move across the edge of a protected folder.
const CROSSING: &str =
    "Items cannot be moved into or out of a protected folder. Copy them instead.";

/// Holds the owner's protections for `tx` (see [`Tx::hold_protections`]); first in the
/// transaction, before any row lock.
pub(crate) async fn hold(tx: &mut Tx) -> Result<(), FilesError> {
    Ok(tx.hold_protections().await?)
}

/// Refuses to move `node` into `parent` (the root when `None`) unless the same protected
/// folder, or none, holds both where it is and where it goes. A protected folder itself sits
/// outside every protected folder and moves with what it holds. `tx` must [`hold`] the
/// owner's protections.
pub(crate) async fn refuse_crossing(
    tx: &mut Tx,
    node: &Node,
    parent: Option<NodeId>,
) -> Result<(), FilesError> {
    let from = tx.protected_folder_of(node.parent_id).await?;
    let to = tx.protected_folder_of(parent).await?;
    if from == to {
        Ok(())
    } else {
        Err(FilesError::Protected(CROSSING))
    }
}
