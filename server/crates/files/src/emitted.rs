//! The domain events each kind of change publishes.

use db::{MoveBatch, MoveOutcome, Node, NodeKind, StoreOutcome};
use events::DomainEvent;

/// What storing a file changed: a new file, new content, or only an older version kept.
pub(crate) fn stored(outcome: &StoreOutcome) -> Vec<DomainEvent> {
    match outcome {
        StoreOutcome::Created(node) => vec![DomainEvent::NodeCreated(node.clone())],
        StoreOutcome::Versioned { node, .. } => vec![
            DomainEvent::ContentChanged {
                owner: node.owner_id,
                node: node.id,
            },
            DomainEvent::NodeUpdated(node.clone()),
        ],
        StoreOutcome::Archived { node, .. } => vec![DomainEvent::NodeUpdated(node.clone())],
        StoreOutcome::Unchanged(_) => Vec::new(),
    }
}

/// What a rename or move of one node changed.
pub(crate) fn moved(outcome: &MoveOutcome) -> Vec<DomainEvent> {
    relocated(
        &outcome.updated,
        &outcome.deleted,
        &outcome.content_changed,
        outcome.node.owner_id,
    )
}

/// What a move or restore of several nodes changed, the recycle bin included.
pub(crate) fn moved_batch(
    owner: db::UserId,
    batch: &MoveBatch,
    trash_changed: bool,
) -> Vec<DomainEvent> {
    let mut events = relocated(
        &batch.updated,
        &batch.deleted,
        &batch.content_changed,
        owner,
    );
    if trash_changed {
        events.push(DomainEvent::TrashChanged { owner });
    }
    events
}

/// Folders and files saved into a tree, folders first as the handlers always announced them.
pub(crate) fn saved(folders: &[Node], files: &[StoreOutcome]) -> Vec<DomainEvent> {
    let mut events: Vec<DomainEvent> = folders
        .iter()
        .map(|folder| DomainEvent::NodeCreated(folder.clone()))
        .collect();
    events.extend(files.iter().flat_map(stored));
    events
}

fn relocated(
    updated: &[Node],
    deleted: &[Node],
    content_changed: &[db::NodeId],
    owner: db::UserId,
) -> Vec<DomainEvent> {
    let mut events: Vec<DomainEvent> = updated
        .iter()
        .map(|node| DomainEvent::NodeMoved(node.clone()))
        .collect();
    events.extend(
        deleted
            .iter()
            .map(|node| DomainEvent::NodeDeleted(node.clone())),
    );
    events.extend(
        content_changed
            .iter()
            .map(|&node| DomainEvent::ContentChanged { owner, node }),
    );
    events
}

/// Restored files were not indexed while binned, so their content is extracted again.
pub(crate) fn restored_content(batch: &MoveBatch, owner: db::UserId) -> Vec<DomainEvent> {
    batch
        .updated
        .iter()
        .filter(|node| node.kind == NodeKind::File)
        .map(|node| DomainEvent::ContentChanged {
            owner,
            node: node.id,
        })
        .collect()
}
