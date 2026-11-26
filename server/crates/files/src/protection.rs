//! Encrypting what a password folder already held, in place: the names of everything inside,
//! the current content of every file and every older version, so the version history is kept;
//! and decrypting it all again when the folder is unprotected. Safe to run again after a stop:
//! a name or blob already in the wanted form is left as it is.

use db::{DbError, FolderNode, NodeId, NodeKind, UserId};

use crate::{
    FilesError, Names, NodeService,
    tree::rules::{numbered_file_name, numbered_folder_name},
};

/// How many numbered names are tried for a name whose sealed form is taken.
const MAX_NUMBERED_NAMES: u32 = 1000;

impl NodeService {
    /// Seals every name and encrypts every blob in `folder` that is not yet sealed with its
    /// `names`; returns how many names and blobs it changed. Stores into the folder wait while
    /// a blob is being encrypted, and the other way round.
    pub async fn encrypt_folder(
        &self,
        owner: UserId,
        folder: NodeId,
        names: &Names,
    ) -> Result<usize, FilesError> {
        let Some(keys) = names.keys() else {
            return Ok(0);
        };
        let renamed = self.seal_names(owner, folder, names).await?;
        let recipient = keys.recipient();
        let identity = keys.identity();
        let store = &self.content.store;
        let mut sealed = 0;
        for file in self.content.db.folder_files(owner, folder).await? {
            {
                let _writing = self.content.sealing.write().await;
                if store
                    .seal_blob(owner, file.node_id, &recipient, identity)
                    .await?
                {
                    sealed += 1;
                }
            }
            for version in file.versions {
                let _writing = self.content.sealing.write().await;
                if store
                    .seal_version(owner, version, &recipient, identity)
                    .await?
                {
                    sealed += 1;
                }
            }
        }
        if renamed + sealed > 0 {
            tracing::info!(user = %owner, folder = %folder, names = renamed, blobs = sealed, "password folder encrypted");
        }
        Ok(renamed + sealed)
    }

    /// Seals the names inside `folder` that are still plain.
    async fn seal_names(
        &self,
        owner: UserId,
        folder: NodeId,
        names: &Names,
    ) -> Result<usize, FilesError> {
        let mut sealed = 0;
        for node in self.content.db.folder_nodes(owner, folder).await? {
            if !names.is_stored(node.kind, &node.name)
                && self.seal_name(owner, &node, names).await?
            {
                sealed += 1;
            }
        }
        Ok(sealed)
    }

    /// Replaces the plain name of `node` with its sealed form. A name too long to seal is
    /// shortened, keeping a file's extension, and one whose sealed form is taken (by a name that
    /// only differs in Unicode normalization) is numbered. `false` when the name changed
    /// meanwhile, which a later run picks up.
    async fn seal_name(
        &self,
        owner: UserId,
        node: &FolderNode,
        names: &Names,
    ) -> Result<bool, FilesError> {
        for number in 0..=MAX_NUMBERED_NAMES {
            let candidate = match (node.kind, number) {
                (_, 0) => node.name.clone(),
                (NodeKind::File, n) => numbered_file_name(&node.name, n),
                (NodeKind::Folder, n) => numbered_folder_name(&node.name, n as usize + 1),
            };
            let stored = sealed_fitting(names, node.kind, &candidate)?;
            match self
                .content
                .db
                .rename_node_from(owner, node.id, &node.name, &stored)
                .await
            {
                Ok(renamed) => {
                    if renamed && number > 0 {
                        tracing::warn!(node = %node.id, "a name in a password folder was numbered to keep it unique");
                    }
                    return Ok(renamed);
                }
                Err(DbError::Conflict) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Err(FilesError::Conflict)
    }
}

impl NodeService {
    /// Decrypts every blob and opens every name in `folder` that `names` still opens; returns
    /// how many it changed. A run that changed nothing has left the folder plain.
    pub async fn decrypt_folder(
        &self,
        owner: UserId,
        folder: NodeId,
        names: &Names,
    ) -> Result<usize, FilesError> {
        let Some(keys) = names.keys() else {
            return Ok(0);
        };
        let opened = self.unseal_blobs(owner, folder, keys.identity()).await?
            + self.open_names(owner, folder, names).await?;
        if opened > 0 {
            tracing::info!(user = %owner, folder = %folder, changed = opened, "password folder decrypted");
        }
        Ok(opened)
    }

    /// Decrypts in place every blob of the files in `folder` that `identity` opens.
    async fn unseal_blobs(
        &self,
        owner: UserId,
        folder: NodeId,
        identity: &vault_core::content::Identity,
    ) -> Result<usize, FilesError> {
        let store = &self.content.store;
        let mut opened = 0;
        for file in self.content.db.folder_files(owner, folder).await? {
            {
                let _writing = self.content.sealing.write().await;
                if store.unseal_blob(owner, file.node_id, identity).await? {
                    opened += 1;
                }
            }
            for version in file.versions {
                let _writing = self.content.sealing.write().await;
                if store.unseal_version(owner, version, identity).await? {
                    opened += 1;
                }
            }
        }
        Ok(opened)
    }

    /// Opens every name in `folder` that `names` still opens.
    async fn open_names(
        &self,
        owner: UserId,
        folder: NodeId,
        names: &Names,
    ) -> Result<usize, FilesError> {
        let mut opened = 0;
        for node in self.content.db.folder_nodes(owner, folder).await? {
            if names.is_stored(node.kind, &node.name) && self.open_name(owner, &node, names).await?
            {
                opened += 1;
            }
        }
        Ok(opened)
    }

    /// Replaces the sealed name of `node` with its plain form, numbered when that is taken by
    /// something stored meanwhile. `false` when the name changed meanwhile.
    async fn open_name(
        &self,
        owner: UserId,
        node: &FolderNode,
        names: &Names,
    ) -> Result<bool, FilesError> {
        let plain = names.show(node.kind, &node.name);
        for number in 0..=MAX_NUMBERED_NAMES {
            let candidate = match (node.kind, number) {
                (_, 0) => plain.clone(),
                (NodeKind::File, n) => numbered_file_name(&plain, n),
                (NodeKind::Folder, n) => numbered_folder_name(&plain, n as usize + 1),
            };
            match self
                .content
                .db
                .rename_node_from(owner, node.id, &node.name, &candidate)
                .await
            {
                Ok(renamed) => return Ok(renamed),
                Err(DbError::Conflict) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Err(FilesError::Conflict)
    }
}

/// `name` sealed with `names`, shortened first when it is too long to seal: a file keeps its
/// extension and loses characters from the end of the rest.
fn sealed_fitting(names: &Names, kind: NodeKind, name: &str) -> Result<String, FilesError> {
    let (mut stem, extension) = match (kind, name.rfind('.')) {
        (NodeKind::File, Some(dot)) if dot > 0 && name.len() - dot <= 16 => {
            (name[..dot].to_owned(), &name[dot..])
        }
        _ => (name.to_owned(), ""),
    };
    loop {
        match names.store(kind, &format!("{stem}{extension}")) {
            Err(FilesError::NameTooLongToProtect) if stem.chars().count() > 1 => {
                stem.pop();
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use vault_core::FolderSecret;

    use super::*;

    #[test]
    fn a_long_name_is_shortened_to_fit_and_keeps_its_extension() {
        let names = Names::sealed(NodeId::new(), FolderSecret::from_raw([3; 32]).keys());
        let long = format!("{}.pdf", "é".repeat(120));
        let stored = sealed_fitting(&names, NodeKind::File, &long).unwrap();
        let shown = names.show(NodeKind::File, &stored);
        assert!(shown.ends_with(".pdf"));
        assert!(shown.len() <= 159);
        assert!(long.starts_with(shown.trim_end_matches(".pdf")));

        let short = sealed_fitting(&names, NodeKind::Folder, "Taxes").unwrap();
        assert_eq!(names.show(NodeKind::Folder, &short), "Taxes");
    }
}
