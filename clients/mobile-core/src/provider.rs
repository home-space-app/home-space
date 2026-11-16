//! Calls for a system file provider (the iOS File Provider extension): the change feed, one item
//! at a time, and uploads that go straight up rather than through the upload queue.

use std::path::PathBuf;

use protocol::{Change, CreateUpload};

use crate::uploads::MAX_CHUNK_BYTES;
use crate::{CoreClient, CoreError, Entry, off_ui_thread, parse_id};

/// One change of [`CoreClient::changes`].
#[derive(Debug, Clone, uniffi::Enum)]
pub enum EntryChange {
    /// Created or changed; this is how it is now.
    Upserted { entry: Entry },
    /// Deleted or moved to the recycle bin.
    Deleted { id: String },
}

/// A page of [`CoreClient::changes`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct ChangePage {
    pub changes: Vec<EntryChange>,
    /// The `since` of the next call.
    pub cursor: i64,
    /// Whether more changes follow `cursor`.
    pub more: bool,
}

#[uniffi::export]
impl CoreClient {
    /// Changes after cursor `since`, or none but the current cursor without it. Fails with code
    /// `gone` once the server no longer keeps the changes after `since`; the caller then lists
    /// everything again.
    pub async fn changes(&self, since: Option<i64>) -> Result<ChangePage, CoreError> {
        let inner = self.inner.clone();
        let page = off_ui_thread(async move { inner.changes(since).await }).await?;
        Ok(ChangePage {
            changes: page
                .changes
                .into_iter()
                .map(|change| match change {
                    Change::Upserted { node, .. } => EntryChange::Upserted { entry: node.into() },
                    Change::Deleted { id, .. } => EntryChange::Deleted { id: id.to_string() },
                })
                .collect(),
            cursor: page.cursor,
            more: page.more,
        })
    }

    /// One file or folder, as [`list`](Self::list) shows it, without the details panel's extras.
    pub async fn entry(&self, id: String) -> Result<Entry, CoreError> {
        let id = parse_id(&id)?;
        let inner = self.inner.clone();
        let node = off_ui_thread(async move { inner.get(id).await }).await?;
        Ok(node.into())
    }

    /// Uploads the file at `path` as `name` in `parent_id`, or Home when `None`, and returns it.
    /// A name that is taken gets a new version, or with `keep_both` a file of its own under the
    /// next free name, as `name (1).ext`. Identical content changes nothing either way.
    pub async fn upload_file(
        &self,
        parent_id: Option<String>,
        name: String,
        path: String,
        mime: Option<String>,
        keep_both: bool,
    ) -> Result<Entry, CoreError> {
        let parent = parent_id.as_deref().map(parse_id).transpose()?;
        self.send_file(path, move |size| CreateUpload {
            parent_id: parent,
            name,
            size,
            // Left out: content dated before the current file's would become an old version.
            modified_at: None,
            mime,
            node_id: None,
            keep_both,
            version_of: None,
            auto_upload: false,
            transfer_item_id: None,
        })
        .await
    }

    /// Replaces the content of file `id` with the file at `path`, wherever it has moved to.
    pub async fn replace_file(&self, id: String, path: String) -> Result<Entry, CoreError> {
        let id = parse_id(&id)?;
        self.send_file(path, move |size| CreateUpload {
            parent_id: None,
            name: String::new(),
            size,
            modified_at: None,
            mime: None,
            node_id: Some(id),
            keep_both: false,
            version_of: None,
            auto_upload: false,
            transfer_item_id: None,
        })
        .await
    }
}

impl CoreClient {
    async fn send_file(
        &self,
        path: String,
        request: impl FnOnce(u64) -> CreateUpload + Send + 'static,
    ) -> Result<Entry, CoreError> {
        let path = PathBuf::from(path);
        let inner = self.inner.clone();
        // All of it on the core's runtime: the caller's thread has none.
        off_ui_thread(async move {
            let size = tokio::fs::metadata(&path)
                .await
                .map_err(|err| CoreError::Failed {
                    code: "file".to_owned(),
                    detail: err.to_string(),
                })?
                .len();
            let request = request(size);
            let mut session = inner.create_upload(&request).await?;
            // Small chunks keep an extension's memory low.
            session.chunk_size = session.chunk_size.min(MAX_CHUNK_BYTES);
            let id = session.id;
            let sent = inner
                .continue_upload(
                    session,
                    size,
                    |offset, len| {
                        let path = path.clone();
                        async move { sync_core::read_range(&path, offset, len).await }
                    },
                    |_| {},
                )
                .await;
            match sent {
                Ok(node) => Ok(Entry::from(node)),
                Err(err) => {
                    let _ = inner.cancel_upload(id).await;
                    Err(err.into())
                }
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_partial_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::tests::{node, signed_in};

    const UPLOAD_ID: &str = "00000000-0000-0000-0000-0000000000ee";

    #[tokio::test]
    async fn changes_map_to_entries_and_deletions() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let file = node("a.txt", "file", 3);
        Mock::given(method("GET"))
            .and(path(protocol::paths::CHANGES))
            .and(query_param("since", "4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "changes": [
                    { "type": "upserted", "seq": 5, "node": file },
                    { "type": "deleted", "seq": 6, "id": "00000000-0000-0000-0000-000000000009" },
                ],
                "cursor": 6,
                "more": false,
            })))
            .mount(&server)
            .await;

        let page = client.changes(Some(4)).await.unwrap();

        assert_eq!(page.cursor, 6);
        assert!(!page.more);
        assert!(
            matches!(&page.changes[0], EntryChange::Upserted { entry } if entry.name == "a.txt")
        );
        assert!(matches!(
            &page.changes[1],
            EntryChange::Deleted { id } if id == "00000000-0000-0000-0000-000000000009"
        ));
    }

    #[tokio::test]
    async fn a_pruned_cursor_fails_as_gone() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        Mock::given(method("GET"))
            .and(path(protocol::paths::CHANGES))
            .respond_with(
                ResponseTemplate::new(410)
                    .set_body_json(serde_json::json!({ "error": "gone", "message": "pruned" })),
            )
            .mount(&server)
            .await;

        let err = client.changes(Some(1)).await.unwrap_err();

        assert!(
            matches!(&err, CoreError::Failed { code, .. } if code == "gone"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_file_goes_up_by_name_without_a_date() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"abcdef").unwrap();
        Mock::given(method("POST"))
            .and(path(protocol::paths::UPLOADS))
            .and(body_partial_json(
                serde_json::json!({ "name": "a.txt", "size": 6, "mime": "text/plain" }),
            ))
            .and(|request: &wiremock::Request| {
                let body: serde_json::Value = request.body_json().expect("json body");
                body.get("modified_at").is_none() && body.get("keep_both").is_none()
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": UPLOAD_ID, "offset": 0, "size": 6, "chunk_size": 1024,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
            .and(header(protocol::paths::UPLOAD_OFFSET, "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "offset": 6, "node": node("a.txt", "file", 6) }),
            ))
            .expect(1)
            .mount(&server)
            .await;

        let entry = client
            .upload_file(
                None,
                "a.txt".into(),
                file.to_string_lossy().into_owned(),
                Some("text/plain".into()),
                false,
            )
            .await
            .unwrap();

        assert_eq!(entry.name, "a.txt");
        assert_eq!(entry.size, 6);
    }

    #[tokio::test]
    async fn keeping_both_asks_for_a_name_of_its_own() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"abcdef").unwrap();
        Mock::given(method("POST"))
            .and(path(protocol::paths::UPLOADS))
            .and(body_partial_json(
                serde_json::json!({ "name": "a.txt", "keep_both": true }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": UPLOAD_ID, "offset": 0, "size": 6, "chunk_size": 1024,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "offset": 6, "node": node("a (1).txt", "file", 6) }),
            ))
            .mount(&server)
            .await;

        let entry = client
            .upload_file(
                None,
                "a.txt".into(),
                file.to_string_lossy().into_owned(),
                None,
                true,
            )
            .await
            .unwrap();

        assert_eq!(entry.name, "a (1).txt");
    }

    #[tokio::test]
    async fn replacing_names_the_file() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("b");
        std::fs::write(&file, b"xyz").unwrap();
        let id = "00000000-0000-0000-0000-000000000005";
        Mock::given(method("POST"))
            .and(path(protocol::paths::UPLOADS))
            .and(body_partial_json(
                serde_json::json!({ "node_id": id, "size": 3 }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": UPLOAD_ID, "offset": 0, "size": 3, "chunk_size": 1024,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({ "offset": 3, "node": node("b", "file", 3) }),
                ),
            )
            .mount(&server)
            .await;

        let entry = client
            .replace_file(id.into(), file.to_string_lossy().into_owned())
            .await
            .unwrap();

        assert_eq!(entry.size, 3);
    }

    /// Swift calls in from threads of its own, which have no Tokio runtime.
    #[test]
    fn an_upload_works_from_a_thread_without_a_runtime() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (_server, client) = runtime.block_on(async {
            let server = MockServer::start().await;
            let client = signed_in(&server).await;
            Mock::given(method("POST"))
                .and(path(protocol::paths::UPLOADS))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": UPLOAD_ID, "offset": 0, "size": 2, "chunk_size": 1024,
                })))
                .mount(&server)
                .await;
            Mock::given(method("PATCH"))
                .and(path(format!("{}/{UPLOAD_ID}", protocol::paths::UPLOADS)))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({ "offset": 2, "node": node("c", "file", 2) }),
                ))
                .mount(&server)
                .await;
            (server, client)
        });
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("c");
        std::fs::write(&file, b"hi").unwrap();

        let entry = futures::executor::block_on(client.upload_file(
            None,
            "c".into(),
            file.to_string_lossy().into_owned(),
            None,
            false,
        ))
        .unwrap();

        assert_eq!(entry.size, 2);
    }
}
