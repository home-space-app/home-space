use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::Node;

/// A live file listed by `GET /duplicates`, with the names of its ancestors, root first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateFile {
    pub node: Node,
    pub location: Vec<String>,
}

/// Live files sharing one content; the first one is the oldest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub sha256: String,
    pub size: i64,
    pub files: Vec<DuplicateFile>,
}

impl DuplicateGroup {
    /// Bytes that removing all but one copy would free.
    pub fn wasted(&self) -> i64 {
        let extra = i64::try_from(self.files.len().saturating_sub(1)).unwrap_or(i64::MAX);
        self.size.saturating_mul(extra)
    }
}

/// A live file whose content is a previous version of another live file, which has changed
/// since: the file is likely an outdated copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutdatedMatch {
    pub file: DuplicateFile,
    pub source: DuplicateFile,
    /// The version of `source` holding the same content as `file`.
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub archived_at: OffsetDateTime,
}

/// Response of `GET /duplicates`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicatesReport {
    /// Largest waste first.
    pub groups: Vec<DuplicateGroup>,
    pub outdated: Vec<OutdatedMatch>,
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::{NodeId, NodeKind};

    use super::*;

    fn file(name: &str) -> DuplicateFile {
        DuplicateFile {
            node: Node {
                id: NodeId(Uuid::nil()),
                parent_id: None,
                kind: NodeKind::File,
                name: name.to_owned(),
                size: 4,
                sha256: Some("ab".to_owned()),
                mime: "text/plain".to_owned(),
                version: 1,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
                edited_on: None,
            },
            location: vec!["docs".to_owned()],
        }
    }

    #[test]
    fn report_round_trips_through_json() {
        let report = DuplicatesReport {
            groups: vec![DuplicateGroup {
                sha256: "ab".to_owned(),
                size: 4,
                files: vec![file("a.txt"), file("b.txt")],
            }],
            outdated: vec![OutdatedMatch {
                file: file("old.txt"),
                source: file("new.txt"),
                version: 2,
                archived_at: OffsetDateTime::UNIX_EPOCH,
            }],
        };
        let json = serde_json::to_string(&report).expect("json");
        assert_eq!(
            serde_json::from_str::<DuplicatesReport>(&json).expect("back"),
            report
        );
    }

    #[test]
    fn waste_counts_every_copy_but_one() {
        let group = DuplicateGroup {
            sha256: "ab".to_owned(),
            size: 10,
            files: vec![file("a"), file("b"), file("c")],
        };
        assert_eq!(group.wasted(), 20);
    }
}
