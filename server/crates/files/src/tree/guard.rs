//! Keeping a tree change in step with the protection of folders. Names are chosen before the
//! change's transaction, so the transaction first holds the owner's protections and then
//! checks that the folder written to is still protected as the names assumed: nothing is
//! written plain into a password folder, or sealed outside one.

use db::{Node, NodeId, Tx};

use crate::{FilesError, Names};

/// Why a node may not move across the edge of a password folder.
const CROSSING: &str = "Items cannot be moved into or out of a password folder. Copy them instead.";

/// Holds the owner's protections for `tx` (see [`Tx::hold_protections`]); first in the
/// transaction, before any row lock.
pub(crate) async fn hold(tx: &mut Tx) -> Result<(), FilesError> {
    Ok(tx.hold_protections().await?)
}

/// Refuses to write names chosen as `names` into `parent` (the root when `None`) unless they
/// are still those of the password folder that holds it, or of none. `tx` must
/// [`hold`] the owner's protections.
pub(crate) async fn check_names(
    tx: &mut Tx,
    parent: Option<NodeId>,
    names: &Names,
) -> Result<(), FilesError> {
    let holder = tx.password_folder_of(parent).await?;
    names.check_holder(holder.as_ref())
}

/// Refuses to move `node` into `parent` (the root when `None`) unless the same password
/// folder, or none, holds both where it is and where it goes: inside one names are stored
/// sealed and everywhere else plain. A password folder itself sits where names are plain.
/// `tx` must [`hold`] the owner's protections.
pub(crate) async fn refuse_crossing(
    tx: &mut Tx,
    node: &Node,
    parent: Option<NodeId>,
) -> Result<(), FilesError> {
    let from = tx.password_folder_of(node.parent_id).await?;
    let to = tx.password_folder_of(parent).await?;
    if from.map(|folder| folder.node_id) == to.map(|folder| folder.node_id) {
        Ok(())
    } else {
        Err(FilesError::Protected(CROSSING))
    }
}
