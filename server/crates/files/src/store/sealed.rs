//! The content of password folders, which the server keeps encrypted: an age file to the
//! folder's recipient in place of each blob. Encrypting needs only the recipient; only reading
//! and converting need the folder's identity.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use db::{NodeId, UserId};
use uuid::Uuid;
use vault_core::content::{self, Identity, Recipient};

use super::{BlobStore, StagedBlob, TMP_DIR};
use crate::FilesError;

impl BlobStore {
    /// Encrypts a staged blob to `recipient` into a new staged blob, which keeps the plain
    /// size and hash the file is listed with; the plain one is removed.
    pub(crate) async fn seal_staged(
        &self,
        staged: StagedBlob,
        recipient: Recipient,
    ) -> Result<StagedBlob, FilesError> {
        let target = self.root.join(TMP_DIR).join(NodeId::new().to_string());
        let source = staged.path.clone();
        let written = target.clone();
        let sealed =
            tokio::task::spawn_blocking(move || encrypt_file(&source, &written, &recipient))
                .await
                .map_err(|err| FilesError::Io(io::Error::other(err)))?;
        let _ = tokio::fs::remove_file(&staged.path).await;
        match sealed {
            Ok(()) => Ok(StagedBlob {
                path: target,
                size: staged.size,
                sha256: staged.sha256,
            }),
            Err(err) => {
                let _ = tokio::fs::remove_file(&target).await;
                Err(FilesError::Io(err))
            }
        }
    }

    /// Encrypts the current content of `node` in place, unless `identity` already opens it.
    /// Whether it did; a blob that is gone is left alone.
    pub(crate) async fn seal_blob(
        &self,
        owner: UserId,
        node: NodeId,
        recipient: &Recipient,
        identity: &Identity,
    ) -> io::Result<bool> {
        self.seal_in_place(self.blob_path(owner, node), recipient, identity)
            .await
    }

    /// Encrypts the stored content of a version in place, as [`Self::seal_blob`].
    pub(crate) async fn seal_version(
        &self,
        owner: UserId,
        version: Uuid,
        recipient: &Recipient,
        identity: &Identity,
    ) -> io::Result<bool> {
        self.seal_in_place(self.version_path(owner, version), recipient, identity)
            .await
    }

    async fn seal_in_place(
        &self,
        path: PathBuf,
        recipient: &Recipient,
        identity: &Identity,
    ) -> io::Result<bool> {
        let temp = self.root.join(TMP_DIR).join(NodeId::new().to_string());
        let recipient = recipient.clone();
        let identity = identity.clone();
        tokio::task::spawn_blocking(move || {
            let current = match fs::File::open(&path) {
                Ok(file) => file,
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(err) => return Err(err),
            };
            if content::opens_with(&identity, io::BufReader::new(current)) {
                return Ok(false);
            }
            if let Err(err) = encrypt_file(&path, &temp, &recipient) {
                let _ = fs::remove_file(&temp);
                return Err(err);
            }
            fs::rename(&temp, &path)?;
            Ok(true)
        })
        .await
        .map_err(io::Error::other)?
    }
}

/// Writes `source` encrypted to `recipient` into `target`, flushed to disk.
fn encrypt_file(source: &Path, target: &Path, recipient: &Recipient) -> io::Result<()> {
    let mut input = io::BufReader::new(fs::File::open(source)?);
    let output = io::BufWriter::new(fs::File::create(target)?);
    let mut writer = content::encryptor(recipient, output).map_err(io::Error::other)?;
    io::copy(&mut input, &mut writer)?;
    let output = writer.finish()?;
    let file = output
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?;
    file.sync_all()
}
