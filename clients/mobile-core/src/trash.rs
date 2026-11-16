//! The recycle bin: the files the user deleted, which can be restored to Lost & Found or deleted
//! for good. Nothing empties it by itself.

use std::time::SystemTime;

use client_core::{format, labels};
use protocol::{OnConflict, TrashItem};

use crate::{
    CoreClient, CoreError, Entry, EntryIcon, MoveConflict, MoveMode, off_ui_thread, parse_id,
};

/// A file in the recycle bin.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TrashEntry {
    pub id: String,
    pub icon: EntryIcon,
    pub name: String,
    pub mime: String,
    pub size: i64,
    /// E.g. `412.3 KB`.
    pub size_label: String,
    /// The folder it was deleted from, e.g. `/Docs/Taxes`, or `Home` for the top level.
    pub folder: String,
    /// When it was deleted; the apps format this for the device's locale and time zone.
    pub deleted_at: SystemTime,
}

impl From<TrashItem> for TrashEntry {
    fn from(item: TrashItem) -> Self {
        let folder = folder_of(&item.original_path);
        let entry = Entry::from(item.node);
        Self {
            id: entry.id,
            icon: entry.icon,
            name: entry.name,
            mime: entry.mime,
            size: entry.size,
            size_label: entry.size_label,
            folder,
            deleted_at: item.deleted_at.into(),
        }
    }
}

/// `/Docs/Taxes/a.pdf` → `/Docs/Taxes`; a file at the top level → `Home`.
fn folder_of(original_path: &str) -> String {
    match original_path.trim_end_matches('/').rsplit_once('/') {
        Some((folder, _)) if !folder.is_empty() => folder.to_owned(),
        _ => labels::HOME.to_owned(),
    }
}

/// How a restore ended. With conflicts nothing changed, and it can be retried with a mode.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RestoreResult {
    /// How many files are now in Lost & Found.
    pub restored: u32,
    /// Lost & Found, to open it from the message.
    pub folder_id: Option<String>,
    /// E.g. `2 files already exist in “Lost & Found”`.
    pub conflicts_title: String,
    pub conflicts: Vec<MoveConflict>,
    /// E.g. `Restored 2 files to Lost & Found`; empty when nothing was restored.
    pub message: String,
}

#[uniffi::export]
impl CoreClient {
    /// The files in the recycle bin, most recently deleted first.
    pub async fn trash(&self) -> Result<Vec<TrashEntry>, CoreError> {
        let inner = self.inner.clone();
        let items = off_ui_thread(async move { inner.trash().await }).await?;
        Ok(items.into_iter().map(TrashEntry::from).collect())
    }

    /// Moves binned files to Lost & Found. `Skip` leaves the clashing ones in the bin, and
    /// `Override` makes the restored file the current version of the one there.
    pub async fn restore_trashed(
        &self,
        ids: Vec<String>,
        mode: MoveMode,
    ) -> Result<RestoreResult, CoreError> {
        let ids = ids
            .iter()
            .map(|id| parse_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        let on_conflict = match mode {
            MoveMode::Abort => OnConflict::Abort,
            MoveMode::Override => OnConflict::Override,
            MoveMode::Skip => OnConflict::Skip,
        };
        let inner = self.inner.clone();
        let moved =
            off_ui_thread(async move { inner.restore_trashed(&ids, on_conflict).await }).await?;
        let name = moved
            .target
            .as_ref()
            .map_or(labels::LOST_AND_FOUND, |target| target.name.as_str());
        let restored = if moved.conflicts.is_empty() {
            moved.moved.len()
        } else {
            0
        };
        Ok(RestoreResult {
            restored: restored as u32,
            folder_id: moved.target.as_ref().map(|target| target.id.to_string()),
            conflicts_title: format::conflicts_title(moved.conflicts.len(), &format!("“{name}”")),
            conflicts: moved.conflicts.iter().map(MoveConflict::from).collect(),
            message: match restored {
                0 => String::new(),
                count => format!("Restored {} to {name}", labels::files(count)),
            },
        })
    }

    /// Deletes binned files for good, one at a time. Stops at the first failure, with the ones
    /// before it already gone.
    pub async fn delete_forever(&self, ids: Vec<String>) -> Result<(), CoreError> {
        let ids = ids
            .iter()
            .map(|id| parse_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        let inner = self.inner.clone();
        off_ui_thread(async move {
            for id in ids {
                inner.purge_trashed(id).await?;
            }
            Ok(())
        })
        .await
    }

    /// Deletes every file in the recycle bin for good.
    pub async fn empty_trash(&self) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        Ok(off_ui_thread(async move { inner.empty_trash().await }).await?)
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::tests::{node, signed_in};

    const ID_A: &str = "00000000-0000-0000-0000-0000000000a1";
    const ID_B: &str = "00000000-0000-0000-0000-0000000000b2";
    const LOST: &str = "00000000-0000-0000-0000-0000000000f1";

    fn file(id: &str, name: &str, size: i64) -> serde_json::Value {
        let mut json = node(name, "file", size);
        json["id"] = id.into();
        json["mime"] = "application/pdf".into();
        json
    }

    fn lost_and_found() -> serde_json::Value {
        let mut json = node("Lost & Found", "folder", 0);
        json["id"] = LOST.into();
        json
    }

    #[test]
    fn the_folder_is_the_path_without_the_name() {
        assert_eq!(folder_of("/Docs/Taxes/a.pdf"), "/Docs/Taxes");
        assert_eq!(folder_of("/a.pdf"), "Home");
        assert_eq!(folder_of("a.pdf"), "Home");
    }

    #[tokio::test]
    async fn the_bin_lists_files_with_their_folder() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::TRASH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "node": file(ID_A, "return.pdf", 422_195),
                    "original_path": "/Docs/Taxes/return.pdf",
                    "deleted_at": "2026-11-14T09:00:00Z",
                },
                {
                    "node": file(ID_B, "b.pdf", 10),
                    "original_path": "/b.pdf",
                    "deleted_at": "2026-11-13T09:00:00Z",
                },
            ])))
            .mount(&server)
            .await;
        let entries = signed_in(&server).await.trash().await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, ID_A);
        assert_eq!(entries[0].name, "return.pdf");
        assert_eq!(entries[0].icon, EntryIcon::Document);
        assert_eq!(entries[0].folder, "/Docs/Taxes");
        assert_eq!(entries[0].size_label, "412.3 KB");
        assert_eq!(
            entries[0].deleted_at,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_794_646_800)
        );
        assert_eq!(entries[1].folder, "Home");
    }

    #[tokio::test]
    async fn a_restore_says_where_the_files_went() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::TRASH_RESTORE))
            .and(body_partial_json(
                serde_json::json!({"ids": [ID_A, ID_B], "on_conflict": "abort"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "moved": [file(ID_A, "a.pdf", 1), file(ID_B, "b.pdf", 1)],
                "skipped": [], "conflicts": [], "target": lost_and_found(),
            })))
            .mount(&server)
            .await;
        let result = signed_in(&server)
            .await
            .restore_trashed(vec![ID_A.into(), ID_B.into()], MoveMode::Abort)
            .await
            .unwrap();
        assert_eq!(result.restored, 2);
        assert_eq!(result.folder_id.as_deref(), Some(LOST));
        assert!(result.conflicts.is_empty());
        assert_eq!(result.message, "Restored 2 files to Lost & Found");
    }

    #[tokio::test]
    async fn a_restore_reports_its_conflicts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::TRASH_RESTORE))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "moved": [], "skipped": [], "target": lost_and_found(),
                "conflicts": [{
                    "source": file(ID_A, "a.pdf", 11),
                    "path": "a.pdf",
                    "existing": file(ID_B, "a.pdf", 20),
                }],
            })))
            .mount(&server)
            .await;
        let result = signed_in(&server)
            .await
            .restore_trashed(vec![ID_A.into()], MoveMode::Abort)
            .await
            .unwrap();
        assert_eq!(result.restored, 0);
        assert_eq!(result.message, "");
        assert_eq!(
            result.conflicts_title,
            "1 file already exists in “Lost & Found”"
        );
        assert_eq!(result.conflicts[0].name, "a.pdf");
        assert!(result.conflicts[0].existing.is_some());
    }

    #[tokio::test]
    async fn a_skipping_restore_counts_only_what_moved() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::TRASH_RESTORE))
            .and(body_partial_json(
                serde_json::json!({"on_conflict": "skip"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "moved": [file(ID_B, "b.pdf", 1)],
                "skipped": [ID_A], "conflicts": [], "target": lost_and_found(),
            })))
            .mount(&server)
            .await;
        let result = signed_in(&server)
            .await
            .restore_trashed(vec![ID_A.into(), ID_B.into()], MoveMode::Skip)
            .await
            .unwrap();
        assert_eq!(result.restored, 1);
        assert_eq!(result.message, "Restored 1 file to Lost & Found");
    }

    #[tokio::test]
    async fn deleting_forever_stops_at_the_first_failure() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(protocol::paths::trash_item(ID_A.parse().unwrap())))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": "internal", "message": "boom",
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(protocol::paths::trash_item(ID_B.parse().unwrap())))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
        let result = signed_in(&server)
            .await
            .delete_forever(vec![ID_A.into(), ID_B.into()])
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn emptying_the_bin_deletes_it_all() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(protocol::paths::TRASH))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        signed_in(&server).await.empty_trash().await.unwrap();
    }

    #[tokio::test]
    async fn an_expired_session_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::TRASH))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "unauthorized", "message": "expired",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(protocol::paths::REFRESH))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "unauthorized", "message": "expired",
            })))
            .mount(&server)
            .await;
        let err = signed_in(&server).await.trash().await.unwrap_err();
        assert!(matches!(err, CoreError::Unauthorized), "{err:?}");
    }
}
