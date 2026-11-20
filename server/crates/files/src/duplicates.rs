//! Files sharing their content: live copies of each other, and live files that hold an old
//! version of another file.

use std::collections::HashMap;

use db::{Node, UserId};
use protocol::{DuplicateFile, DuplicateGroup, DuplicatesReport, OutdatedMatch};

use crate::{FilesError, content::ContentStore};

/// The most groups of copies one report lists.
pub const MAX_GROUPS: usize = 1000;
/// The most outdated files one report lists.
pub const MAX_OUTDATED: usize = 1000;

/// Finds a user's files that share their content.
#[derive(Clone, Debug)]
pub struct DuplicateService {
    pub(crate) content: ContentStore,
}

impl DuplicateService {
    pub(crate) fn new(content: ContentStore) -> Self {
        Self { content }
    }

    /// The owner's groups of identical live files, largest waste first, and the live files
    /// matching a previous version of another file.
    pub async fn report(&self, owner: UserId) -> Result<DuplicatesReport, FilesError> {
        let db = &self.content.db;
        let copies = db.duplicate_nodes(owner).await?;
        let mut outdated = db.outdated_matches(owner).await?;
        outdated.sort_by_key(|row| std::cmp::Reverse(row.archived_at));
        outdated.truncate(MAX_OUTDATED);

        let mut groups = group(copies);
        groups.sort_by_key(|files| std::cmp::Reverse(waste(files)));
        groups.truncate(MAX_GROUPS);

        let matched = outdated
            .iter()
            .flat_map(|row| [row.file_id, row.source_id])
            .collect::<Vec<_>>();
        let nodes = db
            .nodes_by_ids(owner, &matched)
            .await?
            .into_iter()
            .map(|node| (node.id, node))
            .collect::<HashMap<_, _>>();
        let ids = groups
            .iter()
            .flatten()
            .map(|node| node.id)
            .chain(nodes.keys().copied())
            .collect::<Vec<_>>();
        let locations = db.list_locations(owner, &ids).await?;
        let located = |node: Node| DuplicateFile {
            location: locations.get(&node.id).cloned().unwrap_or_default(),
            node: node.into(),
        };

        let outdated = outdated
            .into_iter()
            .filter_map(|row| {
                let file = nodes.get(&row.file_id).cloned()?;
                let source = nodes.get(&row.source_id).cloned()?;
                Some(OutdatedMatch {
                    file: located(file),
                    source: located(source),
                    version: row.version,
                    archived_at: row.archived_at,
                })
            })
            .collect();
        let groups = groups
            .into_iter()
            .map(|files| {
                let files = files.into_iter().map(located).collect::<Vec<_>>();
                DuplicateGroup {
                    sha256: files[0].node.sha256.clone().unwrap_or_default(),
                    size: files[0].node.size,
                    files,
                }
            })
            .collect();
        Ok(DuplicatesReport { groups, outdated })
    }
}

/// Splits nodes ordered by content into runs sharing one content.
fn group(nodes: Vec<Node>) -> Vec<Vec<Node>> {
    let mut groups: Vec<Vec<Node>> = Vec::new();
    for node in nodes {
        match groups.last_mut() {
            Some(last) if last[0].sha256 == node.sha256 => last.push(node),
            _ => groups.push(vec![node]),
        }
    }
    groups.retain(|files| files.len() > 1);
    groups
}

fn waste(files: &[Node]) -> i64 {
    let extra = i64::try_from(files.len().saturating_sub(1)).unwrap_or(i64::MAX);
    files[0].size.saturating_mul(extra)
}
