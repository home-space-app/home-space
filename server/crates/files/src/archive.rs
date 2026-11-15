//! What goes into a zip of shared files and folders.

use std::{collections::HashSet, path::PathBuf};

use time::OffsetDateTime;

use db::{NodeId, NodeKind, ShareLink};

use crate::{FileService, FilesError};

/// The most entries one zip of a share may hold.
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;

/// One entry of a zip: a folder, or a file and where its bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Where the entry sits in the zip, `/`-separated.
    pub path: String,
    pub kind: NodeKind,
    pub size: i64,
    pub modified_at: Option<OffsetDateTime>,
    /// The bytes of a file; `None` for a folder.
    pub source: Option<PathBuf>,
}

impl FileService {
    /// Everything a visitor can reach through `share`, or inside its folder `folder`, as the
    /// entries of a zip. Shared items with the same name get a number, as `name (2)`.
    pub async fn share_archive(
        &self,
        share: &ShareLink,
        folder: Option<NodeId>,
    ) -> Result<Vec<ArchiveEntry>, FilesError> {
        let roots = match folder {
            Some(id) => {
                let node = self.db.shared_node(share, id).await?;
                if node.kind != NodeKind::Folder {
                    return Err(FilesError::NotFound);
                }
                vec![node]
            }
            None => self
                .db
                .share_items(&[share.id])
                .await?
                .remove(&share.id)
                .unwrap_or_default(),
        };
        let mut seen = HashSet::new();
        let mut names = HashSet::new();
        let mut entries = Vec::new();
        for root in roots {
            if seen.contains(&root.id) {
                continue;
            }
            let name = unique_name(&root.name, &names);
            names.insert(name.clone());
            for entry in self.db.subtree(share.owner_id, root.id).await? {
                let node = entry.node;
                if !seen.insert(node.id) {
                    continue;
                }
                let is_file = node.kind == NodeKind::File;
                entries.push(ArchiveEntry {
                    path: format!("{name}{}", entry.rel),
                    kind: node.kind,
                    size: if is_file { node.size } else { 0 },
                    modified_at: Some(node.updated_at),
                    source: is_file.then(|| self.store.blob_path(share.owner_id, node.id)),
                });
                if entries.len() > MAX_ARCHIVE_ENTRIES {
                    return Err(FilesError::TooManyItems {
                        limit: MAX_ARCHIVE_ENTRIES,
                    });
                }
            }
        }
        Ok(entries)
    }
}

/// `name`, or `name (2)`, `name (3)`… when it is already one of `taken`. A file keeps its
/// extension last, as `report (2).pdf`.
fn unique_name(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    (2..)
        .map(|number| match extension {
            Some(extension) => format!("{stem} ({number}).{extension}"),
            None => format!("{stem} ({number})"),
        })
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_free_name_is_kept() {
        assert_eq!(unique_name("Photos", &taken(&["Music"])), "Photos");
    }

    #[test]
    fn a_taken_name_gets_the_next_free_number() {
        assert_eq!(
            unique_name("Photos", &taken(&["Photos", "Photos (2)"])),
            "Photos (3)"
        );
    }

    #[test]
    fn a_taken_file_name_keeps_its_extension_last() {
        assert_eq!(
            unique_name("report.pdf", &taken(&["report.pdf"])),
            "report (2).pdf"
        );
    }

    #[test]
    fn a_dot_file_is_numbered_at_the_end() {
        assert_eq!(unique_name(".env", &taken(&[".env"])), ".env (2)");
    }
}
