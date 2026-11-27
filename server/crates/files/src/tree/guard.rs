//! Keeping moves in step with the protection of folders. Inside a protected folder, of either
//! lock, everything is ciphertext the client made and outside one it is plain, so a move
//! across a protected folder's edge is refused. The move's transaction first holds the
//! owner's protections, so the folders checked stay protected as they are until it ends.
//!
//! Names inside a protected folder are the client's sealing of them, longer than plain names
//! (see [`protocol::rules::MAX_SEALED_NODE_NAME_CHARS`]); only there may a name be that long.

use db::{Db, Node, NodeId, Tx, UserId};
use protocol::rules::{node_name_problem, sealed_node_name_problem};

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

/// Refuses `name` for a node in `parent` (the root when `None`) unless it is a valid node
/// name, or a sealed name up to [`protocol::rules::MAX_SEALED_NODE_NAME_CHARS`] characters and
/// `parent` is a protected folder or inside one. Request validation allows the longer names
/// everywhere, not knowing where the node goes. Asks the database only for a long name.
pub(crate) async fn refuse_invalid_name(
    tx: &mut Tx,
    parent: Option<NodeId>,
    name: &str,
) -> Result<(), FilesError> {
    if node_name_problem(name).is_none() {
        return Ok(());
    }
    if sealed_node_name_problem(name).is_none() && tx.protected_folder_of(parent).await?.is_some() {
        Ok(())
    } else {
        Err(FilesError::InvalidName)
    }
}

/// [`refuse_invalid_name`] for what only records a name to use later, such as a resumable
/// upload; storing the node checks again. Opens a transaction only for a long name.
pub(crate) async fn refuse_invalid_name_of(
    db: &Db,
    owner: UserId,
    parent: Option<NodeId>,
    name: &str,
) -> Result<(), FilesError> {
    if node_name_problem(name).is_none() {
        return Ok(());
    }
    let mut tx = db.transaction(owner).await?;
    refuse_invalid_name(&mut tx, parent, name).await
}
