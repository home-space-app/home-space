//! The files the encryption works with on this computer: a private folder, readable only by the
//! user, for decrypted copies opened with other apps and for the encrypted copies of uploads,
//! and streaming encryption and decryption between files.
//!
//! The private folder is wiped at start, on sign-out and when the app closes; a folder's opened
//! copies go when it locks. On Linux it is under `$XDG_RUNTIME_DIR`, which is in memory and
//! the user's alone.

use std::{
    fs, io,
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use client_core::format::sanitize_component;
use iced::futures::StreamExt;
use protocol::{Node, NodeId};
use tokio::io::AsyncWriteExt;
use vault_core::{Recipient, content};

use super::Inside;

const PRIVATE_FOLDER: &str = "home-space-protected";
const WORK: &str = "work";
const OPENED: &str = "opened";
const DECRYPT_FAILED: &str =
    "The file could not be decrypted: it is damaged, or not encrypted for this folder.";

/// The private folder, made readable only by the user.
pub fn private_dir() -> io::Result<PathBuf> {
    let dir = base_dir().join(PRIVATE_FOLDER);
    make_private(&dir)?;
    Ok(dir)
}

fn base_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(runtime);
    }
    std::env::temp_dir()
}

#[cfg(unix)]
fn make_private(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_private(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

/// Deletes the private folder and everything in it.
pub fn sweep() {
    let _ = fs::remove_dir_all(base_dir().join(PRIVATE_FOLDER));
}

/// A new path in the private folder for a file of passing work.
pub fn work_path() -> io::Result<PathBuf> {
    let dir = private_dir()?.join(WORK);
    fs::create_dir_all(&dir)?;
    Ok(dir.join(uuid::Uuid::new_v4().to_string()))
}

/// Where the decrypted copy of `node` of the protected folder `root` goes to be opened with
/// another app: a folder of its own, so the file keeps its name and the system picks the app
/// by its extension.
pub fn opened_path(root: NodeId, node: &Node) -> io::Result<PathBuf> {
    let dir = private_dir()?
        .join(OPENED)
        .join(root.0.to_string())
        .join(node.id.0.to_string());
    fs::create_dir_all(&dir)?;
    Ok(dir.join(sanitize_component(&node.name)))
}

/// Deletes the decrypted copies opened from the protected folder `root`.
pub fn forget_opened(root: NodeId) {
    let _ = fs::remove_dir_all(
        base_dir()
            .join(PRIVATE_FOLDER)
            .join(OPENED)
            .join(root.0.to_string()),
    );
}

/// Encrypts the file at `plain` into `sealed` for `recipient`; returns the encrypted size.
pub async fn encrypt_file(
    recipient: Recipient,
    plain: PathBuf,
    sealed: PathBuf,
) -> Result<u64, String> {
    blocking(move || {
        let input = BufReader::new(fs::File::open(&plain).map_err(|err| describe(&plain, &err))?);
        let mut output =
            BufWriter::new(fs::File::create(&sealed).map_err(|err| describe(&sealed, &err))?);
        content::encrypt(&recipient, input, &mut output).map_err(|err| err.to_string())?;
        output
            .into_inner()
            .map_err(|err| err.to_string())?
            .sync_all()
            .map_err(|err| describe(&sealed, &err))?;
        fs::metadata(&sealed)
            .map(|metadata| metadata.len())
            .map_err(|err| describe(&sealed, &err))
    })
    .await
}

/// Decrypts the file at `sealed` into `plain` with the keys of the folder it is in.
pub async fn decrypt_file(inside: Inside, sealed: PathBuf, plain: PathBuf) -> Result<(), String> {
    blocking(move || {
        let input = BufReader::new(fs::File::open(&sealed).map_err(|err| describe(&sealed, &err))?);
        let mut reader = content::decrypt(inside.keys().identity(), input)
            .map_err(|_| DECRYPT_FAILED.to_owned())?;
        let mut output =
            BufWriter::new(fs::File::create(&plain).map_err(|err| describe(&plain, &err))?);
        io::copy(&mut reader, &mut output).map_err(|_| DECRYPT_FAILED.to_owned())?;
        output.flush().map_err(|err| describe(&plain, &err))
    })
    .await
}

/// Decrypts a whole file's content in memory.
pub fn decrypt_bytes(inside: &Inside, sealed: &[u8]) -> Result<Vec<u8>, String> {
    let mut plain = Vec::with_capacity(sealed.len());
    content::decrypt(inside.keys().identity(), sealed)
        .and_then(|mut reader| reader.read_to_end(&mut plain).map_err(Into::into))
        .map_err(|_| DECRYPT_FAILED.to_owned())?;
    Ok(plain)
}

/// Encrypts a whole file's content in memory.
pub fn encrypt_bytes(inside: &Inside, plain: &[u8]) -> Result<Vec<u8>, String> {
    content::encrypt_bytes(&inside.recipient(), plain).map_err(|err| err.to_string())
}

/// Writes the body of `response` to a new file at `path`, telling `received` the size of each
/// chunk as it is written.
pub async fn save_response(
    response: reqwest::Response,
    path: &Path,
    mut received: impl FnMut(usize),
) -> Result<(), String> {
    let mut out = tokio::fs::File::create(path)
        .await
        .map_err(|err| describe(path, &err))?;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|err| err.to_string())?;
        out.write_all(&chunk)
            .await
            .map_err(|err| describe(path, &err))?;
        received(chunk.len());
    }
    out.flush().await.map_err(|err| describe(path, &err))
}

fn describe(path: &Path, err: &io::Error) -> String {
    format!("{}: {err}", path.display())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| err.to_string())?
}
