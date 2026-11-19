use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};

use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    DEVICE_SETTING, Db, DbError, Node, NodeId, NodeKind, OnConflict, UserId,
    trash::{bin_subtree, special_folders},
};

mod types;
pub use types::{
    BlobOp, DeletedNodes, MoveBatch, MoveConflict, MoveOutcome, NewContent, NewNode,
    NodeProperties, NodeUpdate, StoreOptions, StoreOutcome,
};

/// MIME type stored for folders.
pub const FOLDER_MIME: &str = "inode/directory";

macro_rules! node_columns {
    () => {
        "id, owner_id, parent_id, kind, name, size, sha256, mime, version, created_at, updated_at, \
         edited_on"
    };
}

/// The longest node name, as the `nodes` table checks it.
const MAX_NAME_CHARS: usize = 255;
/// A longer "extension" is not one, and is not kept apart when a number is added to a name.
const MAX_NUMBERED_EXTENSION: usize = 32;

#[derive(Default)]
struct Effects {
    updated: Vec<Node>,
    deleted: Vec<Node>,
    content_changed: Vec<NodeId>,
    blob_ops: Vec<BlobOp>,
    skipped: Vec<Node>,
    binned: bool,
}

struct Placement {
    node: Node,
    parent: Option<NodeId>,
    name: String,
    touch: bool,
}

impl Db {
    /// Inserts a node under a folder owned by `owner` (or at the root).
    pub async fn create_node(&self, owner: UserId, new: NewNode) -> Result<Node, DbError> {
        let mut tx = self.begin().await?;
        require_folder(&mut tx, owner, new.parent_id).await?;
        let node = sqlx::query_as::<_, Node>(concat!(
            "INSERT INTO nodes (id, owner_id, parent_id, kind, name, size, sha256, mime) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING ",
            node_columns!(),
            ""
        ))
        .bind(new.id)
        .bind(owner)
        .bind(new.parent_id)
        .bind(new.kind)
        .bind(new.name)
        .bind(new.size)
        .bind(new.sha256)
        .bind(new.mime)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(node)
    }

    /// Fetches a node owned by `owner`; `NotFound` for missing or foreign nodes.
    pub async fn get_node(&self, owner: UserId, id: NodeId) -> Result<Node, DbError> {
        let node = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            node_columns!(),
            " FROM nodes WHERE id = $1 AND owner_id = $2 AND ",
            visible!("nodes")
        ))
        .bind(id)
        .bind(owner)
        .fetch_one(self.pool())
        .await?;
        Ok(node)
    }

    /// Lists the children of a folder (or the root) ordered folders first, then by name.
    pub async fn list_children(
        &self,
        owner: UserId,
        parent_id: Option<NodeId>,
    ) -> Result<Vec<Node>, DbError> {
        self.require_folder(owner, parent_id).await?;
        let nodes = sqlx::query_as::<_, Node>(concat!(
            "SELECT ",
            node_columns!(),
            " FROM nodes \
             WHERE owner_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND ",
            visible!("nodes"),
            " ORDER BY kind, name"
        ))
        .bind(owner)
        .bind(parent_id)
        .fetch_all(self.pool())
        .await?;
        Ok(nodes)
    }

    /// Lists the ancestors of a node from the root down to its parent.
    pub async fn list_ancestors(&self, owner: UserId, id: NodeId) -> Result<Vec<Node>, DbError> {
        let node = self.get_node(owner, id).await?;
        let ancestors = sqlx::query_as::<_, Node>(concat!(
            "WITH RECURSIVE chain AS ( \
                 SELECT ",
            node_columns!(),
            ", 1 AS depth FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT ",
            "n.id, n.owner_id, n.parent_id, n.kind, n.name, n.size, n.sha256, n.mime, \
                 n.version, n.created_at, n.updated_at, n.edited_on, c.depth + 1 \
                 FROM nodes n JOIN chain c ON n.id = c.parent_id AND n.owner_id = $2 \
             ) SELECT ",
            node_columns!(),
            " FROM chain ORDER BY depth DESC"
        ))
        .bind(node.parent_id)
        .bind(owner)
        .fetch_all(self.pool())
        .await?;
        Ok(ancestors)
    }

    /// Renames and/or moves a node.
    ///
    /// A rename onto a taken name is a `Conflict`. Moving to another folder merges into a
    /// same-named node there: files keep the destination, gain a version when the content
    /// differs and combine their dates; folders are merged recursively.
    /// Returns `Cycle` when moving a folder into itself or a descendant.
    pub async fn update_node(
        &self,
        owner: UserId,
        id: NodeId,
        update: NodeUpdate,
    ) -> Result<MoveOutcome, DbError> {
        let mut tx = self.begin().await?;
        let current = lock_node(&mut tx, owner, id).await?;
        let new_parent = update.parent_id.unwrap_or(current.parent_id);
        let name = update.name.unwrap_or_else(|| current.name.clone());
        let mut effects = Effects::default();
        let node = if new_parent == current.parent_id {
            let node = relocate(
                &mut tx,
                &Placement {
                    node: current,
                    parent: new_parent,
                    name,
                    touch: true,
                },
            )
            .await?;
            effects.updated.push(node.clone());
            node
        } else {
            require_folder(&mut tx, owner, new_parent).await?;
            if let Some(parent) = new_parent {
                ensure_not_descendant(&mut tx, owner, id, parent).await?;
            }
            let first = Placement {
                node: current.clone(),
                parent: new_parent,
                name,
                touch: true,
            };
            place(&mut tx, owner, first, &HashSet::new(), None, &mut effects)
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
    /// happens then. Selected nodes inside another selected folder move along with it.
    /// Returns `Cycle` when moving a folder into itself or a descendant.
    pub async fn move_nodes(
        &self,
        owner: UserId,
        ids: &[NodeId],
        parent: Option<NodeId>,
        on_conflict: OnConflict,
    ) -> Result<MoveBatch, DbError> {
        let mut tx = self.begin().await?;
        let sources = lock_selection(&mut tx, owner, ids).await?;
        let target = match parent {
            Some(id) => Some(lock_folder(&mut tx, owner, id).await?),
            None => None,
        };
        let bin = match on_conflict {
            OnConflict::Skip => Some(special_folders(&mut tx, owner).await?.trash_id),
            OnConflict::Abort | OnConflict::Override => None,
        };
        let batch = move_into(&mut tx, owner, sources, target, on_conflict, bin).await?;
        tx.commit().await?;
        Ok(batch)
    }

    /// Stores content for the file `name` under `parent`.
    ///
    /// Creates the file, keeps the existing one when the content is identical, or archives
    /// the previous content as a version; see [`apply_content`] for how it settles which content stays current.
    pub async fn store_file(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
        name: &str,
        content: NewContent,
    ) -> Result<StoreOutcome, DbError> {
        self.store_file_with(owner, parent, name, content, StoreOptions::default())
            .await
    }

    /// Like [`Db::store_file`], with `options` on how to treat a taken name and what to record.
    /// With `keep_both`, the stored node's name can differ from `name`.
    pub async fn store_file_with(
        &self,
        owner: UserId,
        parent: Option<NodeId>,
        name: &str,
        content: NewContent,
        options: StoreOptions,
    ) -> Result<StoreOutcome, DbError> {
        let mut tx = self.begin().await?;
        require_folder(&mut tx, owner, parent).await?;
        let (existing, name) = if options.keep_both {
            match find_sibling(&mut tx, owner, parent, NodeKind::File, name, None).await? {
                Some(node) if Some(node.id) == options.version_of => (Some(node), name.to_owned()),
                _ => place_beside(&mut tx, owner, parent, name, &content).await?,
            }
        } else {
            let existing = find_sibling(&mut tx, owner, parent, NodeKind::File, name, None).await?;
            (existing, name.to_owned())
        };
        let outcome = match existing {
            Some(node) => apply_content(&mut tx, node, content).await?,
            None => {
                let node = sqlx::query_as::<_, Node>(concat!(
                    "INSERT INTO nodes \
                     (id, owner_id, parent_id, kind, name, size, sha256, mime, updated_at) \
                     VALUES ($1, $2, $3, 'file', $4, $5, $6, $7, COALESCE($8, now())) RETURNING ",
                    node_columns!(),
                    ""
                ))
                .bind(NodeId::new())
                .bind(owner)
                .bind(parent)
                .bind(name)
                .bind(content.size)
                .bind(content.sha256)
                .bind(content.mime)
                .bind(clamp_to_now(content.modified_at))
                .fetch_one(&mut *tx)
                .await?;
                StoreOutcome::Created(node)
            }
        };
        if options.auto_upload
            && let StoreOutcome::Created(node) | StoreOutcome::Versioned { node, .. } = &outcome
        {
            record_source_device(&mut tx, owner, node.id).await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }

    /// The predefined properties of a node of `owner`; none of them set if it has no row.
    pub async fn node_properties(
        &self,
        owner: UserId,
        id: NodeId,
    ) -> Result<NodeProperties, DbError> {
        self.get_node(owner, id).await?;
        let properties = sqlx::query_as::<_, NodeProperties>(
            "SELECT source_device FROM node_properties WHERE node_id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(properties.unwrap_or_default())
    }

    /// Replaces the content of an existing file, archiving the previous content as a version
    /// unless it is identical.
    pub async fn replace_file_content(
        &self,
        owner: UserId,
        id: NodeId,
        content: NewContent,
    ) -> Result<StoreOutcome, DbError> {
        let mut tx = self.begin().await?;
        let node = lock_node(&mut tx, owner, id).await?;
        if node.kind != NodeKind::File {
            return Err(DbError::NotFound);
        }
        let outcome = apply_content(&mut tx, node, content).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// Deletes a node and its descendants; returns the ids whose stored content is now unused.
    pub async fn delete_node(&self, owner: UserId, id: NodeId) -> Result<DeletedNodes, DbError> {
        let mut tx = self.begin().await?;
        let files = sqlx::query_scalar::<_, NodeId>(
            "WITH RECURSIVE subtree AS ( \
                 SELECT id, kind FROM nodes WHERE id = $1 AND owner_id = $2 \
                 UNION ALL \
                 SELECT n.id, n.kind FROM nodes n JOIN subtree s ON n.parent_id = s.id \
             ) SELECT id FROM subtree WHERE kind = 'file'",
        )
        .bind(id)
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        let versions = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM file_versions WHERE owner_id = $1 AND node_id = ANY($2)",
        )
        .bind(owner)
        .bind(files.iter().map(|file| file.0).collect::<Vec<_>>())
        .fetch_all(&mut *tx)
        .await?;
        let deleted = sqlx::query("DELETE FROM nodes WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        if deleted.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        tx.commit().await?;
        Ok(DeletedNodes { files, versions })
    }

    async fn require_folder(&self, owner: UserId, id: Option<NodeId>) -> Result<(), DbError> {
        let mut conn = self.pool().acquire().await?;
        require_folder(&mut conn, owner, id).await
    }
}

pub(crate) async fn lock_node(
    conn: &mut PgConnection,
    owner: UserId,
    id: NodeId,
) -> Result<Node, DbError> {
    let node = sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes WHERE id = $1 AND owner_id = $2 AND ",
        visible!("nodes"),
        " FOR UPDATE"
    ))
    .bind(id)
    .bind(owner)
    .fetch_one(conn)
    .await?;
    Ok(node)
}

async fn lock_folder(conn: &mut PgConnection, owner: UserId, id: NodeId) -> Result<Node, DbError> {
    let node = lock_node(conn, owner, id).await?;
    if node.kind == NodeKind::Folder {
        Ok(node)
    } else {
        Err(DbError::NotFound)
    }
}

/// Locks the nodes of a selection in the order given, without repeats and without nodes
/// that sit inside another selected node.
async fn lock_selection(
    conn: &mut PgConnection,
    owner: UserId,
    ids: &[NodeId],
) -> Result<Vec<Node>, DbError> {
    let mut seen = HashSet::new();
    let ids = ids
        .iter()
        .filter(|id| seen.insert(**id))
        .collect::<Vec<_>>();
    let raw = ids.iter().map(|id| id.0).collect::<Vec<Uuid>>();
    let mut found = sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes WHERE id = ANY($1) AND owner_id = $2 AND ",
        visible!("nodes"),
        " ORDER BY id FOR UPDATE"
    ))
    .bind(&raw)
    .bind(owner)
    .fetch_all(&mut *conn)
    .await?;
    if found.len() != ids.len() {
        return Err(DbError::NotFound);
    }
    let nested = sqlx::query_scalar::<_, NodeId>(
        "WITH RECURSIVE chain AS ( \
             SELECT id, parent_id FROM nodes WHERE id = ANY($1) AND owner_id = $2 \
             UNION ALL \
             SELECT c.id, n.parent_id FROM chain c JOIN nodes n ON n.id = c.parent_id \
         ) SELECT DISTINCT id FROM chain WHERE parent_id = ANY($1)",
    )
    .bind(&raw)
    .bind(owner)
    .fetch_all(conn)
    .await?;
    found.retain(|node| !nested.contains(&node.id));
    let order = ids
        .iter()
        .enumerate()
        .map(|(position, id)| (**id, position))
        .collect::<HashMap<_, _>>();
    found.sort_by_key(|node| order[&node.id]);
    Ok(found)
}

async fn lock_children(
    conn: &mut PgConnection,
    owner: UserId,
    parent: NodeId,
) -> Result<Vec<Node>, DbError> {
    let nodes = sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes WHERE owner_id = $1 AND parent_id = $2 ORDER BY kind, name FOR UPDATE"
    ))
    .bind(owner)
    .bind(parent)
    .fetch_all(conn)
    .await?;
    Ok(nodes)
}

pub(crate) async fn find_sibling(
    conn: &mut PgConnection,
    owner: UserId,
    parent: Option<NodeId>,
    kind: NodeKind,
    name: &str,
    exclude: Option<NodeId>,
) -> Result<Option<Node>, DbError> {
    let node = sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes \
         WHERE owner_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND kind = $3 AND name = $4 \
         AND id IS DISTINCT FROM $5 AND ",
        visible!("nodes"),
        " FOR UPDATE"
    ))
    .bind(owner)
    .bind(parent)
    .bind(kind)
    .bind(name)
    .bind(exclude)
    .fetch_optional(conn)
    .await?;
    Ok(node)
}

/// Where a file goes that must not become a version of a different one: `name`, or else
/// `name (1).ext`, `name (2).ext` and so on, up to the first free name. Identical content at any
/// of them is that same file arriving again, such as a retry after a lost response, and is
/// returned instead.
async fn place_beside(
    conn: &mut PgConnection,
    owner: UserId,
    parent: Option<NodeId>,
    name: &str,
    content: &NewContent,
) -> Result<(Option<Node>, String), DbError> {
    let mut candidate = name.to_owned();
    for number in 1u32.. {
        match find_sibling(conn, owner, parent, NodeKind::File, &candidate, None).await? {
            None => return Ok((None, candidate)),
            Some(node) if same_content(&node, content.size, Some(&content.sha256)) => {
                return Ok((Some(node), candidate));
            }
            Some(_) => candidate = numbered_name(name, number),
        }
    }
    Err(DbError::Conflict)
}

/// `name` with a number before its extension, still within the limit on names.
fn numbered_name(name: &str, number: u32) -> String {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= MAX_NUMBERED_EXTENSION => name.split_at(dot),
        _ => (name, ""),
    };
    let suffix = format!(" ({number}){extension}");
    let room = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
    let stem: String = stem.chars().take(room).collect();
    format!("{stem}{suffix}")
}

/// Records the device of this transaction as the source of `node`. Without a device nothing is
/// recorded.
async fn record_source_device(
    conn: &mut PgConnection,
    owner: UserId,
    node: NodeId,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO node_properties (node_id, source_device) \
         SELECT $1, name FROM devices \
         WHERE user_id = $2 AND id = NULLIF(current_setting($3, true), '')::uuid \
         ON CONFLICT (node_id) DO UPDATE SET source_device = EXCLUDED.source_device",
    )
    .bind(node)
    .bind(owner)
    .bind(DEVICE_SETTING)
    .execute(conn)
    .await?;
    Ok(())
}

async fn require_folder(
    conn: &mut PgConnection,
    owner: UserId,
    id: Option<NodeId>,
) -> Result<(), DbError> {
    match id {
        None => Ok(()),
        Some(id) => {
            let exists = sqlx::query_scalar::<_, bool>(concat!(
                "SELECT EXISTS (SELECT 1 FROM nodes \
                 WHERE id = $1 AND owner_id = $2 AND kind = 'folder' AND ",
                visible!("nodes"),
                ")"
            ))
            .bind(id)
            .bind(owner)
            .fetch_one(conn)
            .await?;
            if exists {
                Ok(())
            } else {
                Err(DbError::NotFound)
            }
        }
    }
}

pub(crate) async fn ensure_not_descendant(
    conn: &mut PgConnection,
    owner: UserId,
    ancestor: NodeId,
    candidate: NodeId,
) -> Result<(), DbError> {
    let is_descendant = sqlx::query_scalar::<_, bool>(
        "WITH RECURSIVE subtree AS ( \
             SELECT id FROM nodes WHERE id = $1 AND owner_id = $2 \
             UNION ALL \
             SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id \
         ) SELECT EXISTS (SELECT 1 FROM subtree WHERE id = $3)",
    )
    .bind(ancestor)
    .bind(owner)
    .bind(candidate)
    .fetch_one(conn)
    .await?;
    if is_descendant {
        Err(DbError::Cycle)
    } else {
        Ok(())
    }
}

async fn relocate(conn: &mut PgConnection, placement: &Placement) -> Result<Node, DbError> {
    let node = sqlx::query_as::<_, Node>(concat!(
        "UPDATE nodes SET name = $2, parent_id = $3, trashed_at = NULL, \
         updated_at = CASE WHEN $4 THEN now() ELSE updated_at END \
         WHERE id = $1 RETURNING ",
        node_columns!(),
        ""
    ))
    .bind(placement.node.id)
    .bind(&placement.name)
    .bind(placement.parent)
    .bind(placement.touch)
    .fetch_one(conn)
    .await?;
    Ok(node)
}

/// Moves several selected nodes into `target`, applying `on_conflict` to clashing files.
///
/// `sources` and `target` must be locked. Binned files can be moved too: they come out live.
/// Files skipped for a clash stay where they are, or go to the recycle bin `bin` when given.
pub(crate) async fn move_into(
    conn: &mut PgConnection,
    owner: UserId,
    sources: Vec<Node>,
    target: Option<Node>,
    on_conflict: OnConflict,
    bin: Option<NodeId>,
) -> Result<MoveBatch, DbError> {
    let parent = target.as_ref().map(|folder| folder.id);
    let mut batch = MoveBatch {
        target,
        ..MoveBatch::default()
    };
    if let Some(parent) = parent {
        for folder in sources.iter().filter(|node| node.kind == NodeKind::Folder) {
            ensure_not_descendant(conn, owner, folder.id, parent).await?;
        }
    }
    let conflicts = plan_conflicts(conn, owner, &sources, parent).await?;
    if on_conflict == OnConflict::Abort && !conflicts.is_empty() {
        batch.conflicts = conflicts;
        return Ok(batch);
    }
    let skip = match on_conflict {
        OnConflict::Skip => conflicts
            .iter()
            .map(|conflict| conflict.source.id)
            .collect(),
        OnConflict::Abort | OnConflict::Override => HashSet::new(),
    };
    let mut effects = Effects::default();
    for source in sources {
        let placement = Placement {
            name: source.name.clone(),
            node: source,
            parent,
            touch: true,
        };
        if let Some(node) = place(conn, owner, placement, &skip, bin, &mut effects).await? {
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

/// Finds the files of `sources` that would clash at `parent` without changing anything.
///
/// Folders merge into same-named folders (also into one an earlier source moves there), so
/// only the files inside them can clash. Conflicts are ordered by path.
async fn plan_conflicts(
    conn: &mut PgConnection,
    owner: UserId,
    sources: &[Node],
    parent: Option<NodeId>,
) -> Result<Vec<MoveConflict>, DbError> {
    struct Claim {
        node: Node,
        path: String,
        reported: bool,
    }
    let mut claims = HashMap::<(Option<NodeId>, NodeKind, String), Claim>::new();
    let mut conflicts = Vec::new();
    let mut queue = sources
        .iter()
        .map(|node| (node.clone(), parent, node.name.clone()))
        .collect::<VecDeque<_>>();
    while let Some((node, dest, path)) = queue.pop_front() {
        let existing =
            find_sibling(conn, owner, dest, node.kind, &node.name, Some(node.id)).await?;
        let claim = Claim {
            node: node.clone(),
            path: path.clone(),
            reported: false,
        };
        // The folder whose children `node` merges into, if it merges.
        let merge_into = match (node.kind, existing) {
            (NodeKind::File, Some(existing)) => {
                conflicts.push(MoveConflict {
                    source: node,
                    path,
                    existing: Some(existing),
                });
                continue;
            }
            (NodeKind::Folder, Some(existing)) => existing.id,
            (_, None) => match claims.entry((dest, node.kind, node.name.clone())) {
                Entry::Vacant(slot) => {
                    slot.insert(claim);
                    continue;
                }
                Entry::Occupied(mut slot) if node.kind == NodeKind::File => {
                    let first = slot.get_mut();
                    if !first.reported {
                        first.reported = true;
                        conflicts.push(MoveConflict {
                            source: first.node.clone(),
                            path: first.path.clone(),
                            existing: None,
                        });
                    }
                    conflicts.push(MoveConflict {
                        source: node,
                        path,
                        existing: None,
                    });
                    continue;
                }
                Entry::Occupied(slot) => slot.get().node.id,
            },
        };
        for child in lock_children(conn, owner, node.id).await? {
            let child_path = format!("{path}/{}", child.name);
            queue.push_back((child, Some(merge_into), child_path));
        }
    }
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(conflicts)
}

/// Moves nodes into place, merging with same-named nodes at the destination.
///
/// Files in `skip` stay where they are, and so do the folders that still hold them, unless
/// `bin` is given: then they go to that recycle bin and the emptied folders are removed.
/// Returns the final location of the first node, `None` when it was skipped.
async fn place(
    conn: &mut PgConnection,
    owner: UserId,
    first: Placement,
    skip: &HashSet<NodeId>,
    bin: Option<NodeId>,
    effects: &mut Effects,
) -> Result<Option<Node>, DbError> {
    let mut pending = vec![first];
    let mut emptied = Vec::new();
    let mut result = None;
    let mut is_first = true;
    while let Some(placement) = pending.pop() {
        let first_placement = is_first;
        is_first = false;
        if skip.contains(&placement.node.id) {
            if let Some(trash_id) = bin {
                bin_subtree(
                    conn,
                    owner,
                    placement.node.id,
                    placement.node.parent_id,
                    trash_id,
                )
                .await?;
                effects.deleted.push(placement.node.clone());
                effects.binned = true;
            }
            effects.skipped.push(placement.node);
            continue;
        }
        let target = find_sibling(
            conn,
            owner,
            placement.parent,
            placement.node.kind,
            &placement.name,
            Some(placement.node.id),
        )
        .await?;
        let placed = match target {
            None => relocate(conn, &placement).await?,
            Some(dest) if dest.kind == NodeKind::File => {
                merge_file(conn, &placement.node, dest, effects).await?
            }
            Some(dest) => {
                let children = lock_children(conn, owner, placement.node.id).await?;
                pending.extend(children.into_iter().map(|node| Placement {
                    name: node.name.clone(),
                    node,
                    parent: Some(dest.id),
                    touch: false,
                }));
                emptied.push(placement.node.clone());
                merge_folder(conn, &placement.node, dest).await?
            }
        };
        if first_placement || placed.kind == NodeKind::File {
            effects.updated.push(placed.clone());
        }
        if first_placement {
            result = Some(placed);
        }
    }
    for folder in emptied.into_iter().rev() {
        let removed = sqlx::query(
            "DELETE FROM nodes WHERE id = $1 \
             AND NOT EXISTS (SELECT 1 FROM nodes child WHERE child.parent_id = $1)",
        )
        .bind(folder.id)
        .execute(&mut *conn)
        .await?;
        if removed.rows_affected() > 0 {
            effects.deleted.push(folder);
        }
    }
    Ok(result)
}

async fn merge_folder(conn: &mut PgConnection, source: &Node, dest: Node) -> Result<Node, DbError> {
    let node = sqlx::query_as::<_, Node>(concat!(
        "UPDATE nodes SET created_at = $2, updated_at = $3 WHERE id = $1 RETURNING ",
        node_columns!(),
        ""
    ))
    .bind(dest.id)
    .bind(dest.created_at.min(source.created_at))
    .bind(dest.updated_at.max(source.updated_at))
    .fetch_one(conn)
    .await?;
    Ok(node)
}

async fn merge_file(
    conn: &mut PgConnection,
    source: &Node,
    dest: Node,
    effects: &mut Effects,
) -> Result<Node, DbError> {
    let identical = same_content(&dest, source.size, source.sha256.as_deref());
    let (offset, version) = if identical {
        effects.blob_ops.push(BlobOp::Remove { node: source.id });
        (dest.version - 1, dest.version + source.version - 1)
    } else {
        let archived = archive_current(conn, &dest).await?;
        effects.blob_ops.push(BlobOp::Archive {
            node: dest.id,
            version_id: archived,
        });
        effects.blob_ops.push(BlobOp::Promote {
            from: source.id,
            to: dest.id,
        });
        effects.content_changed.push(dest.id);
        (dest.version, dest.version + source.version)
    };
    sqlx::query("UPDATE file_versions SET node_id = $2, version = version + $3 WHERE node_id = $1")
        .bind(source.id)
        .bind(dest.id)
        .bind(offset)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(source.id)
        .execute(&mut *conn)
        .await?;
    effects.deleted.push(source.clone());
    let (size, sha256, mime) = if identical {
        (dest.size, dest.sha256.clone(), dest.mime.clone())
    } else {
        (source.size, source.sha256.clone(), source.mime.clone())
    };
    let node = sqlx::query_as::<_, Node>(concat!(
        "UPDATE nodes SET size = $2, sha256 = $3, mime = $4, version = $5, \
         created_at = $6, updated_at = $7 WHERE id = $1 RETURNING ",
        node_columns!(),
        ""
    ))
    .bind(dest.id)
    .bind(size)
    .bind(sha256)
    .bind(mime)
    .bind(version)
    .bind(dest.created_at.min(source.created_at))
    .bind(dest.updated_at.max(source.updated_at))
    .fetch_one(conn)
    .await?;
    Ok(node)
}

fn same_content(node: &Node, size: i64, sha256: Option<&[u8]>) -> bool {
    node.size == size && node.sha256.as_deref() == sha256
}

async fn archive_current(conn: &mut PgConnection, node: &Node) -> Result<Uuid, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO file_versions \
         (id, node_id, owner_id, version, size, sha256, mime, modified_at, edited_on) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(node.id)
    .bind(node.owner_id)
    .bind(node.version)
    .bind(node.size)
    .bind(node.sha256.clone().unwrap_or_default())
    .bind(&node.mime)
    .bind(node.updated_at)
    .bind(node.edited_on)
    .execute(conn)
    .await?;
    Ok(id)
}

/// Stores new content in `node`.
///
/// Identical content changes nothing. Content changed by the client before the current one
/// (`modified_at` older than `updated_at`) is kept as a version so neither side is lost while
/// the newer content stays current; otherwise it becomes current and the previous content a
/// version.
async fn apply_content(
    conn: &mut PgConnection,
    node: Node,
    content: NewContent,
) -> Result<StoreOutcome, DbError> {
    if same_content(&node, content.size, Some(&content.sha256)) {
        return Ok(StoreOutcome::Unchanged(node));
    }
    let modified_at = clamp_to_now(content.modified_at);
    if let Some(at) = modified_at.filter(|at| *at < node.updated_at) {
        return archive_older(conn, node, content, at).await;
    }
    let archived = archive_current(conn, &node).await?;
    let updated = sqlx::query_as::<_, Node>(concat!(
        "UPDATE nodes SET size = $2, sha256 = $3, mime = $4, version = version + 1, \
         updated_at = COALESCE($5, now()) WHERE id = $1 RETURNING ",
        node_columns!(),
        ""
    ))
    .bind(node.id)
    .bind(content.size)
    .bind(content.sha256)
    .bind(content.mime)
    .bind(modified_at)
    .fetch_one(conn)
    .await?;
    Ok(StoreOutcome::Versioned {
        node: updated,
        archived,
    })
}

/// Keeps content older than the current one as a version, taking the current version number
/// and moving the current content to the next one.
async fn archive_older(
    conn: &mut PgConnection,
    node: Node,
    content: NewContent,
    modified_at: OffsetDateTime,
) -> Result<StoreOutcome, DbError> {
    let archived = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO file_versions \
         (id, node_id, owner_id, version, size, sha256, mime, modified_at, edited_on) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULLIF(current_setting($9, true), '')::uuid)",
    )
    .bind(archived)
    .bind(node.id)
    .bind(node.owner_id)
    .bind(node.version)
    .bind(content.size)
    .bind(content.sha256)
    .bind(content.mime)
    .bind(modified_at)
    .bind(DEVICE_SETTING)
    .execute(&mut *conn)
    .await?;
    let updated = sqlx::query_as::<_, Node>(concat!(
        "UPDATE nodes SET version = version + 1 WHERE id = $1 RETURNING ",
        node_columns!(),
        ""
    ))
    .bind(node.id)
    .fetch_one(conn)
    .await?;
    Ok(StoreOutcome::Archived {
        node: updated,
        archived,
    })
}

/// A client time in the future would win every later conflict, so it counts as now.
fn clamp_to_now(at: Option<OffsetDateTime>) -> Option<OffsetDateTime> {
    at.map(|at| at.min(OffsetDateTime::now_utc()))
}

#[cfg(test)]
mod tests;
