//! Moving nodes into the recycle bin and back out to Lost & Found.

use std::collections::HashMap;

use db::{Db, NodeId, OnConflict, UserId};

use super::{MoveBatch, TrashOutcome, moves::move_into};
use crate::FilesError;

/// Deletes node `id` by moving its files to the recycle bin.
///
/// A file moves there itself. A folder's files all move there, each remembering its path,
/// and the folders are removed. Files inside a password folder are deleted for good instead. Lost & Found, and folders containing it, cannot be deleted.
pub(crate) async fn trash_node(
    db: &Db,
    owner: UserId,
    id: NodeId,
) -> Result<TrashOutcome, FilesError> {
    let mut tx = db.transaction(owner).await?;
    let node = tx.lock_node(id).await?;
    let folders = tx.special_folders().await?;
    if tx.is_within(folders.lost_and_found_id, id).await? {
        return Err(FilesError::LostAndFound);
    }
    let (trashed, purged) = tx.bin(&node, folders.trash_id).await?;
    tx.commit().await?;
    Ok(TrashOutcome {
        node,
        trashed,
        purged,
    })
}

/// Moves binned files to Lost & Found.
///
/// The files land directly in Lost & Found under their own names; `on_conflict` decides
/// what happens to files whose name is taken there. Conflict paths are the original paths.
/// Ids that are not binned files are ignored, unknown ids are `NotFound`. When two
/// selected files share a name, the one deleted last ends up current.
pub(crate) async fn restore(
    db: &Db,
    owner: UserId,
    ids: &[NodeId],
    on_conflict: OnConflict,
) -> Result<MoveBatch, FilesError> {
    let mut tx = db.transaction(owner).await?;
    let folders = tx.special_folders().await?;
    let mut binned = tx.lock_binned(ids).await?;
    // Oldest first, so that the newest of several same-named files ends up current.
    binned.sort_by(|a, b| {
        (a.trashed_at, &a.original_path, a.node.id.0).cmp(&(
            b.trashed_at,
            &b.original_path,
            b.node.id.0,
        ))
    });
    let target = tx.lock_node(folders.lost_and_found_id).await?;
    let paths = binned
        .iter()
        .map(|item| (item.node.id, item.original_path.clone()))
        .collect::<HashMap<_, _>>();
    let sources = binned.into_iter().map(|item| item.node).collect::<Vec<_>>();
    let source_ids = sources.iter().map(|node| node.id).collect::<Vec<_>>();
    let mut batch = move_into(&mut tx, sources, Some(target), on_conflict, None).await?;
    for conflict in &mut batch.conflicts {
        if let Some(path) = paths.get(&conflict.source.id) {
            conflict.path.clone_from(path);
        }
    }
    if batch.conflicts.is_empty() || on_conflict != OnConflict::Abort {
        let restored = source_ids
            .into_iter()
            .filter(|id| batch.skipped.iter().all(|skipped| skipped.id != *id))
            .collect::<Vec<_>>();
        tx.forget_trash_entries(&restored).await?;
    }
    tx.commit().await?;
    Ok(batch)
}
