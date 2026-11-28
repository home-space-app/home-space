//! Replacing what a folder holds with its encrypted form when it is protected, and with its
//! plain form when it is unprotected (`docs/todo/protected-folders.md`, "Protecting an existing
//! folder").
//!
//! Each file is copied and then its source deleted, so the work can stop at any point and run
//! again: a name that opens is done, and a file whose copy is there already only needs its
//! source deleted. Inside the protected folder the sources are deleted for good. The copies
//! pass through the private folder ([`super::files`]) and are deleted from it at once.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use iced::futures::{SinkExt, Stream, channel::mpsc};

use api_client::{ApiError, Client};
use protocol::{Node, NodeId, NodeKind, ProtectedFolder, TrashItem};

use super::{Inside, files};
use crate::transfers::{Meter, Uploaded, store_file};

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
    /// What is left to copy, counted before the first file: `files` files of `bytes` in all.
    Planned {
        files: usize,
        bytes: u64,
    },
    /// It works on the file `name` (its plain name), the `index`th of the planned ones from 1.
    File {
        name: String,
        index: usize,
        step: Step,
    },
    /// The bytes downloaded and uploaded so far: each file counts twice, down and then up.
    Moved(u64),
    Done(Result<Finished, Stopped>),
}

/// What is being done with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Downloading,
    /// Encrypting or decrypting it on this computer.
    Converting,
    Uploading,
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
            let reporter = Reporter::new(output.clone());
            let result = convert(&client, &inside, direction, &reporter).await;
            let _ = output.send(Progress::Done(result)).await;
        },
    )
}

/// Tells the interface how the conversion goes; the bytes moved only as often as a transfer
/// reports them.
#[derive(Clone)]
struct Reporter {
    output: mpsc::Sender<Progress>,
    meter: Arc<Mutex<Meter>>,
}

impl Reporter {
    fn new(output: mpsc::Sender<Progress>) -> Self {
        Self {
            output,
            meter: Arc::default(),
        }
    }

    async fn send(&self, progress: Progress) {
        let _ = self.output.clone().send(progress).await;
    }

    async fn file(&self, name: &str, index: usize, step: Step) {
        self.send(Progress::File {
            name: name.to_owned(),
            index,
            step,
        })
        .await;
    }

    /// Adds `bytes` to what was moved; a count missed for a full channel is made up by the next.
    fn moved(&self, bytes: u64) {
        let due = self
            .meter
            .lock()
            .ok()
            .and_then(|mut meter| meter.advance(usize::try_from(bytes).unwrap_or(usize::MAX)));
        if let Some(done) = due {
            let _ = self.output.clone().try_send(Progress::Moved(done));
        }
    }

    /// Hears how far an upload got, adding what it gained since it last told.
    fn uploads(&self) -> Uploaded {
        let reporter = self.clone();
        let told = AtomicU64::new(0);
        Arc::new(move |done| {
            let before = told.swap(done, Ordering::Relaxed);
            reporter.moved(done.saturating_sub(before));
        })
    }
}

async fn convert(
    client: &Client,
    inside: &Inside,
    direction: Direction,
    reporter: &Reporter,
) -> Result<Finished, Stopped> {
    let (files, bytes) = plan(client, inside, direction).await?;
    reporter.send(Progress::Planned { files, bytes }).await;
    convert_content(client, inside, direction, reporter).await?;
    match direction {
        Direction::Protect => {
            let folder = client.finish_protection(inside.root).await?;
            // Only an offer: the folder is protected whether or not the bin can be listed.
            let binned = binned_from(client, inside.root).await.unwrap_or_default();
            Ok(Finished {
                folder: Some(folder),
                binned,
            })
        }
        Direction::Unprotect => {
            client.remove_protection(inside.root).await?;
            Ok(Finished {
                folder: None,
                binned: Vec::new(),
            })
        }
    }
}

/// The name `child` of `folder` gets: sealed when protecting a plain name, opened when
/// unprotecting a sealed one; `None` when it has it already.
fn converted_name(
    inside: &Inside,
    direction: Direction,
    folder: NodeId,
    child: &Node,
) -> Result<Option<String>, Stopped> {
    Ok(match (direction, inside.name_of(child)) {
        (Direction::Protect, Some(_)) | (Direction::Unprotect, None) => None,
        (Direction::Protect, None) => Some(inside.seal(Some(folder), child.kind, &child.name)?),
        (Direction::Unprotect, Some(name)) => Some(name),
    })
}

/// Counts the files left to copy and their bytes: those whose converted copy is not there yet.
async fn plan(
    client: &Client,
    inside: &Inside,
    direction: Direction,
) -> Result<(usize, u64), Stopped> {
    let (mut files, mut bytes) = (0, 0);
    let mut pending = vec![inside.root];
    while let Some(folder) = pending.pop() {
        let children = client.list(Some(folder)).await?;
        let names = taken(&children);
        for child in &children {
            match child.kind {
                NodeKind::Folder => pending.push(child.id),
                NodeKind::File => {
                    if let Some(name) = converted_name(inside, direction, folder, child)?
                        && !names.contains(&(NodeKind::File, name))
                    {
                        files += 1;
                        bytes += u64::try_from(child.size).unwrap_or(0);
                    }
                }
            }
        }
    }
    Ok((files, bytes))
}

/// Gives every name below the folder its converted form and replaces every file with its
/// converted copy: encrypted when protecting, plain when unprotecting.
async fn convert_content(
    client: &Client,
    inside: &Inside,
    direction: Direction,
    reporter: &Reporter,
) -> Result<(), Stopped> {
    let mut index = 0;
    let mut pending = vec![inside.root];
    while let Some(folder) = pending.pop() {
        let children = client.list(Some(folder)).await?;
        let names = taken(&children);
        for child in children {
            match (
                child.kind,
                converted_name(inside, direction, folder, &child)?,
            ) {
                (NodeKind::Folder, Some(name)) => {
                    client.rename(child.id, &name).await?;
                    pending.push(child.id);
                }
                (NodeKind::Folder, None) => pending.push(child.id),
                (NodeKind::File, Some(name)) => {
                    if !names.contains(&(NodeKind::File, name.clone())) {
                        index += 1;
                        let copy = Copy {
                            node: &child,
                            folder,
                            name,
                            index,
                        };
                        copy.run(client, inside, direction, reporter).await?;
                    }
                    client.delete(child.id).await?;
                }
                (NodeKind::File, None) => {}
            }
        }
    }
    Ok(())
}

/// The converted copy of the file `node`, uploaded into `folder` as `name`.
struct Copy<'a> {
    node: &'a Node,
    folder: NodeId,
    name: String,
    /// Which of the planned files it is, from 1.
    index: usize,
}

impl Copy<'_> {
    /// Downloads the file, converts it in the private folder and uploads the result.
    async fn run(
        self,
        client: &Client,
        inside: &Inside,
        direction: Direction,
        reporter: &Reporter,
    ) -> Result<(), Stopped> {
        let shown = match direction {
            Direction::Protect => self.node.name.clone(),
            Direction::Unprotect => self.name.clone(),
        };
        let source = files::work_path()?;
        let target = files::work_path()?;
        let result = async {
            reporter.file(&shown, self.index, Step::Downloading).await;
            let response = client.download(self.node.id).await?;
            files::save_response(response, &source, |bytes| {
                reporter.moved(u64::try_from(bytes).unwrap_or(u64::MAX));
            })
            .await?;
            reporter.file(&shown, self.index, Step::Converting).await;
            let size = match direction {
                Direction::Protect => {
                    files::encrypt_file(inside.recipient(), source.clone(), target.clone()).await?
                }
                Direction::Unprotect => {
                    files::decrypt_file(inside.clone(), source.clone(), target.clone()).await?;
                    tokio::fs::metadata(&target).await?.len()
                }
            };
            reporter.file(&shown, self.index, Step::Uploading).await;
            let uploads = Some(reporter.uploads());
            store_file(client, Some(self.folder), self.name, &target, size, uploads).await?;
            Ok(())
        }
        .await;
        remove(&[source, target]).await;
        result
    }
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
