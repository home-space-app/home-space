//! Replacing what a folder holds with its encrypted form when it is protected, and with its
//! plain form when it is unprotected (`docs/todo/protected-folders.md`, "Protecting an existing
//! folder").
//!
//! Each file is copied and then its source deleted, so the work can stop at any point and run
//! again: a name that opens is done, and a file whose copy is there already only needs its
//! source deleted. Inside the protected folder the sources are deleted for good. The copies
//! pass through the private folder ([`super::files`]) and are deleted from it at once.

use std::{collections::HashSet, path::PathBuf};

use iced::futures::{SinkExt, Stream, channel::mpsc};

use api_client::{ApiError, Client};
use protocol::{Node, NodeId, NodeKind, ProtectedFolder, TrashItem};

use super::{Inside, files};
use crate::transfers::store_file;

const CHANNEL_SIZE: usize = 16;

/// Which way a folder's content is converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Encrypts the plain files and seals the plain names, then marks the folder protected.
    Protect,
    /// Decrypts the files and opens the names, then removes the protection.
    Unprotect,
}

/// How a conversion goes.
#[derive(Debug, Clone)]
pub enum Progress {
    /// It works on the file of this name.
    File(String),
    Done(Result<Finished, Stopped>),
}

/// What a finished conversion leaves.
#[derive(Debug, Clone)]
pub struct Finished {
    /// The protection, now `protected`, after protecting; none after unprotecting.
    pub folder: Option<ProtectedFolder>,
    /// The files of the folder that were in the recycle bin before it was protected, still
    /// plain there.
    pub binned: Vec<TrashItem>,
}

/// Why a conversion stopped.
#[derive(Debug, Clone)]
pub enum Stopped {
    Api(ApiError),
    Failed(String),
}

impl From<ApiError> for Stopped {
    fn from(err: ApiError) -> Self {
        Self::Api(err)
    }
}

impl From<String> for Stopped {
    fn from(err: String) -> Self {
        Self::Failed(err)
    }
}

impl From<std::io::Error> for Stopped {
    fn from(err: std::io::Error) -> Self {
        Self::Failed(err.to_string())
    }
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(err) => err.fmt(f),
            Self::Failed(err) => f.write_str(err),
        }
    }
}

/// Converts the content of the folder `inside` is about, then finishes or removes its
/// protection.
pub fn run(client: Client, inside: Inside, direction: Direction) -> impl Stream<Item = Progress> {
    iced::stream::channel(
        CHANNEL_SIZE,
        async move |mut output: mpsc::Sender<Progress>| {
            let progress = output.clone();
            let report = move |name: &str| {
                let _ = progress.clone().try_send(Progress::File(name.to_owned()));
            };
            let result = match direction {
                Direction::Protect => protect(&client, &inside, &report).await,
                Direction::Unprotect => unprotect(&client, &inside, &report).await,
            };
            let _ = output.send(Progress::Done(result)).await;
        },
    )
}

async fn protect(
    client: &Client,
    inside: &Inside,
    report: &impl Fn(&str),
) -> Result<Finished, Stopped> {
    encrypt_content(client, inside, report).await?;
    let folder = client.finish_protection(inside.root).await?;
    // Only an offer: the folder is protected whether or not the bin can be listed.
    let binned = binned_from(client, inside.root).await.unwrap_or_default();
    Ok(Finished {
        folder: Some(folder),
        binned,
    })
}

async fn unprotect(
    client: &Client,
    inside: &Inside,
    report: &impl Fn(&str),
) -> Result<Finished, Stopped> {
    decrypt_content(client, inside, report).await?;
    client.remove_protection(inside.root).await?;
    Ok(Finished {
        folder: None,
        binned: Vec::new(),
    })
}

/// Seals every plain name below the folder and replaces every plain file with its encrypted
/// copy.
async fn encrypt_content(
    client: &Client,
    inside: &Inside,
    report: &impl Fn(&str),
) -> Result<(), Stopped> {
    let mut pending = vec![inside.root];
    while let Some(folder) = pending.pop() {
        let children = client.list(Some(folder)).await?;
        let names = taken(&children);
        for child in children {
            let sealed = match inside.name_of(&child) {
                Some(_) => None,
                None => Some(inside.seal(Some(folder), child.kind, &child.name)?),
            };
            match (child.kind, sealed) {
                (NodeKind::Folder, Some(sealed)) => {
                    client.rename(child.id, &sealed).await?;
                    pending.push(child.id);
                }
                (NodeKind::Folder, None) => pending.push(child.id),
                (NodeKind::File, Some(sealed)) => {
                    if !names.contains(&(NodeKind::File, sealed.clone())) {
                        report(&child.name);
                        copy_encrypted(client, inside, &child, folder, sealed).await?;
                    }
                    client.delete(child.id).await?;
                }
                (NodeKind::File, None) => {}
            }
        }
    }
    Ok(())
}

/// Opens every sealed name below the folder and replaces every encrypted file with its plain
/// copy, the reverse of [`encrypt_content`].
async fn decrypt_content(
    client: &Client,
    inside: &Inside,
    report: &impl Fn(&str),
) -> Result<(), Stopped> {
    let mut pending = vec![inside.root];
    while let Some(folder) = pending.pop() {
        let children = client.list(Some(folder)).await?;
        let names = taken(&children);
        for child in children {
            match (child.kind, inside.name_of(&child)) {
                (NodeKind::Folder, Some(name)) => {
                    client.rename(child.id, &name).await?;
                    pending.push(child.id);
                }
                (NodeKind::Folder, None) => pending.push(child.id),
                (NodeKind::File, Some(name)) => {
                    if !names.contains(&(NodeKind::File, name.clone())) {
                        report(&name);
                        copy_decrypted(client, inside, &child, folder, name).await?;
                    }
                    client.delete(child.id).await?;
                }
                (NodeKind::File, None) => {}
            }
        }
    }
    Ok(())
}

/// Uploads the encrypted copy of the plain file `node` into `folder` as `sealed`.
async fn copy_encrypted(
    client: &Client,
    inside: &Inside,
    node: &Node,
    folder: NodeId,
    sealed: String,
) -> Result<(), Stopped> {
    let plain = files::work_path()?;
    let encrypted = files::work_path()?;
    let result = async {
        files::save_response(client.download(node.id).await?, &plain).await?;
        let size =
            files::encrypt_file(inside.recipient(), plain.clone(), encrypted.clone()).await?;
        store_file(client, Some(folder), sealed, &encrypted, size, None).await?;
        Ok(())
    }
    .await;
    remove(&[plain, encrypted]).await;
    result
}

/// Uploads the plain copy of the encrypted file `node` into `folder` as `name`.
async fn copy_decrypted(
    client: &Client,
    inside: &Inside,
    node: &Node,
    folder: NodeId,
    name: String,
) -> Result<(), Stopped> {
    let encrypted = files::work_path()?;
    let plain = files::work_path()?;
    let result = async {
        files::save_response(client.download(node.id).await?, &encrypted).await?;
        files::decrypt_file(inside.clone(), encrypted.clone(), plain.clone()).await?;
        let size = tokio::fs::metadata(&plain).await?.len();
        store_file(client, Some(folder), name, &plain, size, None).await?;
        Ok(())
    }
    .await;
    remove(&[encrypted, plain]).await;
    result
}

async fn remove(paths: &[PathBuf]) {
    for path in paths {
        let _ = tokio::fs::remove_file(path).await;
    }
}

/// The names in a listing, by kind: the server allows a file and a folder of the same name.
fn taken(children: &[Node]) -> HashSet<(NodeKind, String)> {
    children
        .iter()
        .map(|child| (child.kind, child.name.clone()))
        .collect()
}

/// The files in the recycle bin that were inside folder `id` when they were deleted.
async fn binned_from(client: &Client, id: NodeId) -> Result<Vec<TrashItem>, ApiError> {
    let (ancestors, folder) = tokio::try_join!(client.ancestors(id), client.get(id))?;
    let path: String = ancestors
        .iter()
        .chain(std::iter::once(&folder))
        .map(|node| format!("/{}", node.name))
        .collect();
    let inside = format!("{path}/");
    Ok(client
        .trash()
        .await?
        .into_iter()
        .filter(|item| item.original_path.starts_with(&inside))
        .collect())
}
