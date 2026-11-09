//! Resumable chunked uploads (`/uploads`).
//!
//! A client creates an upload with [`CreateUpload`], sends the bytes in order with
//! `PATCH /uploads/{id}` and an [`UPLOAD_OFFSET`](crate::paths::UPLOAD_OFFSET) header naming
//! where the chunk starts, and asks `GET /uploads/{id}` where to continue after an error.
//! The chunk that brings the offset to `size` stores the file.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Node, NodeId};

/// Body of `POST /uploads`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateUpload {
    /// Folder of a new file (the root when `None`); ignored when `node_id` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<NodeId>,
    /// Name of a new file; ignored when `node_id` is set.
    #[serde(default)]
    pub name: String,
    /// Total size of the content in bytes.
    pub size: u64,
    /// When the content was last changed on the client; omitted means now.
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_at: Option<OffsetDateTime>,
    /// Declared MIME type of the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// The existing file whose content the upload replaces, like `PUT /files/{id}/content`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
    /// When different content already has this name, store the file as `name (1).ext` or the next
    /// free name, rather than as a new version; identical content at any of those names changes
    /// nothing. Ignored when `node_id` is set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_both: bool,
    /// With `keep_both`: the file this content is a new version of, as long as that file still has
    /// the name in the folder. Any other file with the name is kept. Lets a client that sent a file
    /// earlier send an edit without taking over a different file that got its name since. Ignored
    /// when `node_id` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_of: Option<NodeId>,
    /// Records the calling device as the file's source (`NodeProperties::source_device`). Ignored
    /// when `node_id` is set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_upload: bool,
    /// The item of a transfer whose bytes this is (see `POST /transfers`). `parent_id`,
    /// `node_id`, `keep_both`, `version_of` and `auto_upload` are then ignored, and `size`
    /// must match the item's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_item_id: Option<Uuid>,
}

/// An upload in progress, returned by `POST /uploads` and `GET /uploads/{id}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadSession {
    pub id: Uuid,
    /// Bytes received so far; the next chunk starts here.
    pub offset: u64,
    pub size: u64,
    /// The chunk size the server asks for; a larger chunk is rejected.
    pub chunk_size: u64,
}

/// Response of `PATCH /uploads/{id}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChunkResult {
    /// Bytes received so far.
    pub offset: u64,
    /// The stored file, once the last chunk has arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<Node>,
}

/// Query of `GET /uploads`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadsQuery {
    /// Only the uploads started from the device that asks.
    #[serde(default)]
    pub this_device: bool,
}

/// An unfinished upload, from `GET /uploads`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadInfo {
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
    pub name: String,
    pub size: u64,
    /// The device that started it; `None` for one started before devices were known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<Uuid>,
    /// The transfer item it uploads, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_item_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_upload_defaults_optional_fields() {
        let request: CreateUpload =
            serde_json::from_str(r#"{"name":"a.bin","size":3}"#).expect("json");
        assert_eq!(request.parent_id, None);
        assert_eq!(request.node_id, None);
        assert_eq!(request.size, 3);
    }
}
