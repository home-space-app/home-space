//! Renaming and moving nodes, merging them into same-named nodes at their destination.

use std::collections::{HashMap, HashSet};

use db::{Db, DbError, Node, NodeId, NodeKind, OnConflict, Tx, UserId};

use super::{
    BlobOp, MoveBatch, MoveOutcome, NodeUpdate, conflicts::plan_conflicts, guard, rules::FileMerge,
};
use crate::FilesError;

/// Renames and/or moves node `id`.
///
/// A rename onto a taken name is a `Conflict`. Moving to another folder merges into a
/// same-named node there: files keep the destination, gain a version when the content
/// differs and combine their dates; folders are merged recursively. Moving a folder into
/// itself or below is a `Cycle`. A move never crosses the edge of a protected folder.
pub(crate) async fn update_node(
    db: &Db,
    owner: UserId,
    id: NodeId,
    update: NodeUpdate,
) -> Result<MoveOutcome, FilesError> {
    let mut tx = db.transaction(owner).await?;
    guard::hold(&mut tx).await?;
    let current = tx.lock_node(id).await?;
    let parent = update.parent_id.apply(current.parent_id);
    if parent != current.parent_id {
        guard::refuse_crossing(&mut tx, &current, parent).await?;
    }
    let name = update.name.unwrap_or_else(|| current.name.clone());
    let mut effects = Effects::default();
    let node = if parent == current.parent_id {
        let node = tx.relocate(id, parent, &name, true).await?;
        effects.updated.push(node.clone());
        node
    } else {
        tx.require_folder(parent).await?;
        if let Some(parent) = parent {
            refuse_cycle(&mut tx, &current, parent).await?;
        }
        let placement = Placement {
            node: current.clone(),
            parent,
            name,
            touch: true,
        };
        Placer::new(&mut tx, &HashSet::new(), None, &mut effects)
            .place(placement)
            .await?
            .unwrap_or(current)
    };
    tx.commit().await?;
    Ok(MoveOutcome {
        node,
        updated: effects.updated,
        deleted: effects.deleted,
        content_changed: effects.content_changed,
        blob_ops: effects.blob_ops,
    })
}

/// Moves several nodes into `parent` (the root when `None`).
///
/// Folders merge into same-named folders. A file clashes when a live file of the same name
/// is in its way, or when two selected files share a name; `on_conflict` decides what
/// happens then. Selected nodes inside another selected folder move along with it. None may
/// cross the edge of a protected folder.
pub(crate) async fn move_nodes(
    db: &Db,
    owner: UserId,
    ids: &[NodeId],
    parent: Option<NodeId>,
    on_conflict: OnConflict,
) -> Result<MoveBatch, FilesError> {
    let mut tx = db.transaction(owner).await?;
    guard::hold(&mut tx).await?;
    let sources = select(&mut tx, ids).await?;
    for source in &sources {
        guard::refuse_crossing(&mut tx, source, parent).await?;
    }
    let target = match parent {
        Some(id) => Some(tx.lock_folder(id).await?),
        None => None,
    };
    let bin = match on_conflict {
        OnConflict::Skip => Some(tx.special_folders().await?.trash_id),
        OnConflict::Abort | OnConflict::Override => None,
    };
    let batch = move_into(&mut tx, sources, target, on_conflict, bin).await?;
    tx.commit().await?;
    Ok(batch)
}

/// Moves `sources` into `target`, applying `on_conflict` to clashing files.
///
/// `sources` and `target` must be locked. Binned files can be moved too: they come out live.
/// Files skipped for a clash stay where they are, or go to the recycle bin `bin` when given.
pub(crate) async fn move_into(
    tx: &mut Tx,
    sources: Vec<Node>,
    target: Option<Node>,
    on_conflict: OnConflict,
    bin: Option<NodeId>,
) -> Result<MoveBatch, FilesError> {
    let parent = target.as_ref().map(|folder| folder.id);
    let mut batch = MoveBatch {
        target,
        ..MoveBatch::default()
    };
    if let Some(parent) = parent {
        for folder in sources.iter().filter(|node| node.kind == NodeKind::Folder) {
            refuse_cycle(tx, folder, parent).await?;
        }
    }
    let conflicts = plan_conflicts(tx, &sources, parent).await?;
    let skip = match on_conflict {
        OnConflict::Abort if !conflicts.is_empty() => {
            batch.conflicts = conflicts;
            return Ok(batch);
        }
        OnConflict::Skip => conflicts.iter().map(|c| c.source.id).collect(),
        OnConflict::Abort | OnConflict::Override => HashSet::new(),
    };
    let mut effects = Effects::default();
    let mut placer = Placer::new(tx, &skip, bin, &mut effects);
    for source in sources {
        let placement = Placement {
            name: source.name.clone(),
            node: source,
            parent,
            touch: true,
        };
        if let Some(node) = placer.place(placement).await? {
            batch.moved.push(node);
        }
    }
    batch.skipped = effects.skipped;
    batch.updated = effects.updated;
    batch.deleted = effects.deleted;
    batch.content_changed = effects.content_changed;
    batch.blob_ops = effects.blob_ops;
    batch.binned = effects.binned;
    Ok(batch)
}

/// Locks the selected nodes in the order given, without repeats and without the nodes inside
/// another selected one, which move along with it.
async fn select(tx: &mut Tx, ids: &[NodeId]) -> Result<Vec<Node>, DbError> {
    let mut seen = HashSet::new();
    let ids = ids
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .collect::<Vec<_>>();
    let mut nodes = tx.lock_nodes(&ids).await?;
    let nested = tx.nested_in(&ids).await?;
    nodes.retain(|node| !nested.contains(&node.id));
    let order = ids
        .iter()
        .enumerate()
        .map(|(position, id)| (*id, position))
        .collect::<HashMap<_, _>>();
    nodes.sort_by_key(|node| order[&node.id]);
    Ok(nodes)
}

/// `Cycle` when `parent` is `node` itself or lies below it.
async fn refuse_cycle(tx: &mut Tx, node: &Node, parent: NodeId) -> Result<(), FilesError> {
    if tx.is_within(parent, node.id).await? {
        Err(FilesError::Cycle)
    } else {
        Ok(())
    }
}

/// What placing nodes changed besides the nodes placed.
#[derive(Default)]
struct Effects {
    updated: Vec<Node>,
    deleted: Vec<Node>,
    content_changed: Vec<NodeId>,
    blob_ops: Vec<BlobOp>,
    skipped: Vec<Node>,
    binned: bool,
}

/// Where a node goes.
struct Placement {
    node: Node,
    parent: Option<NodeId>,
    name: String,
    /// Whether the node counts as changed now; the children of a merged folder do not.
    touch: bool,
}

/// Puts nodes in place, merging with same-named nodes at the destination.
///
/// Files in `skip` stay where they are, and so do the folders that still hold them, unless
/// `bin` is given: then they go to that recycle bin and the emptied folders are removed.
struct Placer<'a> {
    tx: &'a mut Tx,
    skip: &'a HashSet<NodeId>,
    bin: Option<NodeId>,
    effects: &'a mut Effects,
    /// The children of merged folders, still to be placed.
    pending: Vec<Placement>,
    /// The merged folders, removed at the end once they are empty.
    emptied: Vec<Node>,
}

impl<'a> Placer<'a> {
    fn new(
        tx: &'a mut Tx,
        skip: &'a HashSet<NodeId>,
        bin: Option<NodeId>,
        effects: &'a mut Effects,
    ) -> Self {
        Self {
            tx,
            skip,
            bin,
            effects,
            pending: Vec::new(),
            emptied: Vec::new(),
        }
    }

    /// Places `first` and, when it merges, what it holds. Returns where `first` ended up,
    /// `None` when it was skipped.
    async fn place(&mut self, first: Placement) -> Result<Option<Node>, FilesError> {
        let placed = self.place_one(first).await?;
        if let Some(node) = &placed {
            self.effects.updated.push(node.clone());
        }
        while let Some(placement) = self.pending.pop() {
            if let Some(node) = self.place_one(placement).await?
                && node.kind == NodeKind::File
            {
                self.effects.updated.push(node);
            }
        }
        for folder in std::mem::take(&mut self.emptied).into_iter().rev() {
            if self.tx.remove_if_empty(folder.id).await? {
                self.effects.deleted.push(folder);
            }
        }
        Ok(placed)
    }

    async fn place_one(&mut self, placement: Placement) -> Result<Option<Node>, FilesError> {
        let Placement {
            node,
            parent,
            name,
            touch,
        } = placement;
        if self.skip.contains(&node.id) {
            self.leave(node).await?;
            return Ok(None);
        }
        let dest = self
            .tx
            .find_sibling(parent, node.kind, &name, Some(node.id))
            .await?;
        let placed = match dest {
            None => self.tx.relocate(node.id, parent, &name, touch).await?,
            Some(dest) if dest.kind == NodeKind::File => self.merge_file(node, &dest).await?,
            Some(dest) => self.merge_folder(node, &dest).await?,
        };
        Ok(Some(placed))
    }

    /// Leaves a skipped file where it is, or bins it.
    async fn leave(&mut self, node: Node) -> Result<(), FilesError> {
        if let Some(trash_id) = self.bin {
            let (_, purged) = self.tx.bin(&node, trash_id).await?;
            if !purged.files.is_empty() {
                // Rolls the whole move back: a skipped file is never deleted for good.
                return Err(FilesError::Protected(
                    "Files in a protected folder cannot be skipped into the recycle bin.",
                ));
            }
            self.effects.deleted.push(node.clone());
            self.effects.binned = true;
        }
        self.effects.skipped.push(node);
        Ok(())
    }

    /// Folds `source` into the same-named file `dest`, as [`FileMerge`] describes.
    async fn merge_file(&mut self, source: Node, dest: &Node) -> Result<Node, FilesError> {
        let merge = FileMerge::of(&source, dest);
        if merge.identical {
            self.effects
                .blob_ops
                .push(BlobOp::Remove { node: source.id });
        } else {
            let version_id = self.tx.archive_current(dest).await?;
            self.effects.blob_ops.extend([
                BlobOp::Archive {
                    node: dest.id,
                    version_id,
                },
                BlobOp::Promote {
                    from: source.id,
                    to: dest.id,
                },
            ]);
            self.effects.content_changed.push(dest.id);
        }
        let merged = self
            .tx
            .absorb(source.id, dest.id, merge.version_offset, &merge.state)
            .await?;
        self.effects.deleted.push(source);
        Ok(merged)
    }

    /// Queues the children of `source` to move into the same-named folder `dest`, which
    /// takes the earlier creation and the later change of the two.
    async fn merge_folder(&mut self, source: Node, dest: &Node) -> Result<Node, FilesError> {
        let children = self.tx.lock_children(source.id).await?;
        self.pending
            .extend(children.into_iter().map(|node| Placement {
                name: node.name.clone(),
                node,
                parent: Some(dest.id),
                touch: false,
            }));
        let merged = self
            .tx
            .set_dates(
                dest.id,
                dest.created_at.min(source.created_at),
                dest.updated_at.max(source.updated_at),
            )
            .await?;
        self.emptied.push(source);
        Ok(merged)
    }
}
