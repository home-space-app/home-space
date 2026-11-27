//! Storing content in a file: a new file, a new version, or an older version kept beside the
//! current content.

use time::OffsetDateTime;

use db::{Db, NewContent, Node, NodeId, NodeKind, Tx, UserId};

use super::{
    StoreOptions, StoreOutcome, guard,
    rules::{ContentFate, not_after, numbered_file_name, same_content},
};
use crate::FilesError;

/// Stores `content` as the file `name` in `parent`.
///
/// A free name creates the file. A taken one gets the content as described for
/// [`ContentFate`], or, with [`StoreOptions::keep_both`], a free numbered name instead, unless
/// the file there is [`StoreOptions::version_of`] or holds the same content already.
pub(crate) async fn store_file(
    db: &Db,
    owner: UserId,
    parent: Option<NodeId>,
    name: &str,
    content: NewContent,
    options: StoreOptions,
) -> Result<StoreOutcome, FilesError> {
    let content = not_after(content, OffsetDateTime::now_utc());
    let mut tx = db.transaction(owner).await?;
    guard::hold(&mut tx).await?;
    tx.require_folder(parent).await?;
    guard::refuse_invalid_name(&mut tx, parent, name).await?;
    let existing = tx.find_sibling(parent, NodeKind::File, name, None).await?;
    let (existing, name) = match existing {
        Some(file) if options.keep_both && Some(file.id) != options.version_of => {
            place_beside(&mut tx, parent, name, &content).await?
        }
        existing => (existing, name.to_owned()),
    };
    let outcome = match existing {
        Some(file) => apply_content(&mut tx, file, &content).await?,
        None => StoreOutcome::Created(tx.insert_file(parent, &name, &content).await?),
    };
    if options.auto_upload
        && let StoreOutcome::Created(file) | StoreOutcome::Versioned { node: file, .. } = &outcome
    {
        tx.record_source_device(file.id).await?;
    }
    tx.commit().await?;
    Ok(outcome)
}

/// Stores `content` in the existing file `id`, as described for [`ContentFate`].
pub(crate) async fn replace_content(
    db: &Db,
    owner: UserId,
    id: NodeId,
    content: NewContent,
) -> Result<StoreOutcome, FilesError> {
    let content = not_after(content, OffsetDateTime::now_utc());
    let mut tx = db.transaction(owner).await?;
    let file = tx.lock_node(id).await?;
    if file.kind != NodeKind::File {
        return Err(FilesError::NotFound);
    }
    let outcome = apply_content(&mut tx, file, &content).await?;
    tx.commit().await?;
    Ok(outcome)
}

/// Where a file goes that must not become a version of a different one: `name`, or else
/// `name (1).ext`, `name (2).ext` and so on, up to the first free name. Identical content at any
/// of them is that same file arriving again, such as a retry after a lost response, and is
/// returned instead. Returns the name it goes under.
async fn place_beside(
    tx: &mut Tx,
    parent: Option<NodeId>,
    name: &str,
    content: &NewContent,
) -> Result<(Option<Node>, String), FilesError> {
    let mut candidate = name.to_owned();
    for number in 1u32.. {
        match tx
            .find_sibling(parent, NodeKind::File, &candidate, None)
            .await?
        {
            None => return Ok((None, candidate)),
            Some(file) if same_content(&file, content.size, Some(&content.sha256)) => {
                return Ok((Some(file), candidate));
            }
            Some(_) => {
                candidate = numbered_file_name(name, number);
            }
        }
    }
    Err(FilesError::Conflict)
}

async fn apply_content(
    tx: &mut Tx,
    file: Node,
    content: &NewContent,
) -> Result<StoreOutcome, FilesError> {
    let outcome = match ContentFate::of(&file, content) {
        ContentFate::Unchanged => StoreOutcome::Unchanged(file),
        ContentFate::Older(at) => {
            // The older content takes the current number; the current one moves up.
            let archived = tx
                .insert_version(file.id, file.version, content, at)
                .await?;
            let node = tx.bump_version(file.id).await?;
            StoreOutcome::Archived { node, archived }
        }
        ContentFate::Current => {
            let archived = tx.archive_current(&file).await?;
            let node = tx.set_content(file.id, content).await?;
            StoreOutcome::Versioned { node, archived }
        }
    };
    Ok(outcome)
}
