use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{NodeId, Patch};

/// Whether a node is a folder or a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "server",
    derive(sqlx::Type),
    sqlx(type_name = "text", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Folder,
    File,
}

/// A file or folder in a user's tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub parent_id: Option<NodeId>,
    pub kind: NodeKind,
    pub name: String,
    pub size: i64,
    pub sha256: Option<String>,
    pub mime: String,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// The device that wrote the content, or created the folder; `None` when it was not a
    /// registered device, or the server is older.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_on: Option<Uuid>,
}

/// Predefined properties of a node, from `GET /nodes/{id}/properties`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeProperties {
    /// The name of the device that last auto-uploaded the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_device: Option<String>,
}

/// A previous version of a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileVersion {
    pub version: i32,
    pub size: i64,
    pub sha256: String,
    pub mime: String,
    #[serde(with = "time::serde::rfc3339")]
    pub modified_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub archived_at: OffsetDateTime,
    /// The device that wrote this content, as for [`Node::edited_on`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_on: Option<Uuid>,
}

/// What to do when a file being moved or restored meets a live file of the same name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnConflict {
    /// Change nothing and report the conflicts.
    #[default]
    Abort,
    /// Merge into the existing file: its content becomes a version and the incoming one current.
    Override,
    /// Leave the existing file untouched and do not place the clashing files: a restore keeps
    /// them in the recycle bin, a move puts them there.
    Skip,
}

/// A file that clashes with a live file at its destination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub source: Node,
    /// Path of the file relative to what is moved, or its original path when restoring.
    pub path: String,
    /// The file in the way; `None` when two selected files share a name.
    pub existing: Option<Node>,
}

/// Body of `POST /nodes/move`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveRequest {
    pub ids: Vec<NodeId>,
    /// The destination folder; `null` for the root.
    pub parent_id: Option<NodeId>,
    #[serde(default)]
    pub on_conflict: OnConflict,
}

/// Result of moving or restoring several nodes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveResponse {
    /// The nodes at their final location, which is the destination when they were merged.
    pub moved: Vec<Node>,
    /// Files that were not placed because of [`OnConflict::Skip`]: still in the recycle bin after
    /// a restore, moved to it by a move.
    pub skipped: Vec<NodeId>,
    /// The clashes found; only filled for [`OnConflict::Abort`], in which case nothing changed.
    pub conflicts: Vec<Conflict>,
    /// The destination folder; `None` for the root.
    pub target: Option<Node>,
}

/// Query of `GET /nodes`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<NodeId>,
}

/// Body of `POST /folders`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateFolderRequest {
    pub parent_id: Option<NodeId>,
    pub name: String,
}

/// Query of `POST /files`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<NodeId>,
    pub name: String,
    /// When the content was last changed on the client; omitted means now.
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_at: Option<OffsetDateTime>,
}

/// Query of `PUT /files/{id}/content`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentQuery {
    /// When the content was last changed on the client; omitted means now.
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_at: Option<OffsetDateTime>,
}

/// Body of `PATCH /nodes/{id}`.
///
/// `parent_id` distinguishes absent (leave unchanged), `null` (move to root)
/// and a value (move into that folder).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// [`Patch::Clear`] moves the node to the root.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub parent_id: Patch<NodeId>,
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn on_conflict_uses_snake_case_and_defaults_to_abort() {
        assert_eq!(
            serde_json::to_string(&OnConflict::Override).expect("json"),
            r#""override""#
        );
        assert_eq!(OnConflict::default(), OnConflict::Abort);
    }

    #[test]
    fn move_request_defaults_to_abort_and_takes_a_null_parent() {
        let id = Uuid::nil();
        let request: MoveRequest =
            serde_json::from_str(&format!(r#"{{"ids":["{id}"],"parent_id":null}}"#))
                .expect("request");
        assert_eq!(request.on_conflict, OnConflict::Abort);
        assert_eq!(request.parent_id, None);
    }

    #[test]
    fn update_distinguishes_absent_null_and_value() {
        let absent: UpdateRequest = serde_json::from_str(r#"{"name":"a"}"#).expect("absent");
        let null: UpdateRequest = serde_json::from_str(r#"{"parent_id":null}"#).expect("null");
        let id = Uuid::nil();
        let value: UpdateRequest =
            serde_json::from_str(&format!(r#"{{"parent_id":"{id}"}}"#)).expect("value");
        assert_eq!(absent.parent_id, Patch::Keep);
        assert_eq!(null.parent_id, Patch::Clear);
        assert_eq!(value.parent_id, Patch::Set(NodeId(id)));
    }

    #[test]
    fn update_serialization_round_trips_move_to_root() {
        let request = UpdateRequest {
            name: None,
            parent_id: Patch::Clear,
        };
        let json = serde_json::to_string(&request).expect("json");
        assert_eq!(json, r#"{"parent_id":null}"#);
        assert_eq!(
            serde_json::from_str::<UpdateRequest>(&json).expect("back"),
            request
        );
    }

    #[test]
    fn node_json_uses_rfc3339_and_lowercase_kind() {
        let node = Node {
            id: NodeId(Uuid::nil()),
            parent_id: None,
            kind: NodeKind::File,
            name: "a.txt".to_owned(),
            size: 1,
            sha256: Some("0aff".to_owned()),
            mime: "text/plain".to_owned(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        };
        let json = serde_json::to_value(&node).expect("json");
        assert_eq!(json["kind"], "file");
        assert_eq!(json["created_at"], "1970-01-01T00:00:00Z");
        assert_eq!(serde_json::from_value::<Node>(json).expect("back"), node);
    }
}
