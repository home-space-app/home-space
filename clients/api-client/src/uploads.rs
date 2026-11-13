//! Resumable chunked uploads (`/uploads`).

use protocol::{CreateUpload, UploadSession, paths};
use uuid::Uuid;

#[cfg(not(target_arch = "wasm32"))]
use protocol::{ChunkResult, Node};

use crate::{ApiError, Client, client::expect_success};

/// How often one chunk is retried before the upload gives up.
#[cfg(not(target_arch = "wasm32"))]
const CHUNK_RETRIES: u32 = 5;

impl Client {
    /// Starts a resumable upload.
    pub async fn create_upload(&self, request: &CreateUpload) -> Result<UploadSession, ApiError> {
        self.send_json(|| self.http().post(self.url(paths::UPLOADS)).json(request))
            .await
    }

    /// Where a resumable upload stands; the next chunk starts at its `offset`.
    pub async fn upload_status(&self, id: Uuid) -> Result<UploadSession, ApiError> {
        self.send_json(|| self.http().get(self.url(&paths::upload(id))))
            .await
    }

    /// Abandons a resumable upload and the bytes it has received.
    pub async fn cancel_upload(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http().delete(self.url(&paths::upload(id))))
            .await?;
        expect_success(response).await
    }

    /// Sends the chunk of a resumable upload that starts at `offset`.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn upload_chunk(
        &self,
        id: Uuid,
        offset: u64,
        chunk: Vec<u8>,
    ) -> Result<ChunkResult, ApiError> {
        self.send_json(|| {
            self.http()
                .patch(self.url(&paths::upload(id)))
                .header(paths::UPLOAD_OFFSET, offset)
                .body(chunk.clone())
        })
        .await
    }

    /// Stores `bytes` as the content of file `id`, changed now; the previous content becomes a
    /// version. Content up to the server's chunk size goes in one request, larger content in
    /// resumable chunks, so no request outgrows what a proxy in front of the server accepts.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn replace_with_bytes(
        &self,
        id: protocol::NodeId,
        mime: Option<&str>,
        bytes: Vec<u8>,
    ) -> Result<Node, ApiError> {
        let size = u64::try_from(bytes.len())
            .map_err(|_| ApiError::Transport("the content is too large".to_owned()))?;
        if size <= self.upload_chunk_bytes() {
            return self
                .replace_content(id, None, || reqwest::Body::from(bytes.clone()))
                .await;
        }
        let request = CreateUpload {
            parent_id: None,
            name: String::new(),
            size,
            modified_at: None,
            mime: mime.map(str::to_owned),
            node_id: Some(id),
            keep_both: false,
            version_of: None,
            auto_upload: false,
            transfer_item_id: None,
        };
        let bytes = &bytes;
        self.upload_resumable(
            &request,
            |offset, len| async move {
                let start = usize::try_from(offset).map_err(std::io::Error::other)?;
                let end = start.saturating_add(len).min(bytes.len());
                bytes
                    .get(start..end)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| std::io::Error::other("chunk outside the content"))
            },
            |_| {},
        )
        .await
    }

    /// Uploads `request.size` bytes in chunks, retrying each chunk from the server's offset
    /// after a failure, and returns the stored file.
    ///
    /// `read(offset, len)` returns the `len` bytes of the content starting at `offset`;
    /// `progress` gets the number of bytes the server has after every chunk.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn upload_resumable<R, Fut, P>(
        &self,
        request: &CreateUpload,
        read: R,
        progress: P,
    ) -> Result<Node, ApiError>
    where
        R: Fn(u64, usize) -> Fut,
        Fut: Future<Output = std::io::Result<Vec<u8>>>,
        P: Fn(u64),
    {
        let session = self.create_upload(request).await?;
        let result = self
            .continue_upload(session, request.size, read, progress)
            .await;
        if result.is_err() {
            let _ = self.cancel_upload(session.id).await;
        }
        result
    }

    /// Sends the rest of the `size` bytes of `session`, from its offset, retrying each chunk
    /// like [`Client::upload_resumable`], and returns the stored file. A failed upload is not
    /// cancelled, so it can be continued later from [`Client::upload_status`].
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn continue_upload<R, Fut, P>(
        &self,
        session: UploadSession,
        size: u64,
        read: R,
        progress: P,
    ) -> Result<Node, ApiError>
    where
        R: Fn(u64, usize) -> Fut,
        Fut: Future<Output = std::io::Result<Vec<u8>>>,
        P: Fn(u64),
    {
        self.send_chunks(session, size, &read, &progress, Ends::WithNode)
            .await?
            .ok_or_else(|| ApiError::Transport("the upload ended without a file".to_owned()))
    }

    /// Uploads the `size` bytes of the file `item` of a transfer the user is sending, in chunks
    /// retried like [`Client::upload_resumable`]. The file becomes part of the transfer, never
    /// a node; a failed upload is cancelled.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn upload_transfer_item<R, Fut, P>(
        &self,
        item: Uuid,
        name: &str,
        size: u64,
        read: R,
        progress: P,
    ) -> Result<(), ApiError>
    where
        R: Fn(u64, usize) -> Fut,
        Fut: Future<Output = std::io::Result<Vec<u8>>>,
        P: Fn(u64),
    {
        let request = CreateUpload {
            parent_id: None,
            name: name.to_owned(),
            size,
            modified_at: None,
            mime: None,
            node_id: None,
            keep_both: false,
            version_of: None,
            auto_upload: false,
            transfer_item_id: Some(item),
        };
        let session = self.create_upload(&request).await?;
        let result = self
            .send_chunks(session, size, &read, &progress, Ends::AtSize)
            .await;
        if result.is_err() {
            let _ = self.cancel_upload(session.id).await;
        }
        result.map(drop)
    }

    /// Sends chunks of `session` from its offset until the upload ends as `ends` says: with
    /// the stored node, or once the server has all `size` bytes (answering `None`).
    #[cfg(not(target_arch = "wasm32"))]
    async fn send_chunks<R, Fut, P>(
        &self,
        session: UploadSession,
        size: u64,
        read: &R,
        progress: &P,
        ends: Ends,
    ) -> Result<Option<Node>, ApiError>
    where
        R: Fn(u64, usize) -> Fut,
        Fut: Future<Output = std::io::Result<Vec<u8>>>,
        P: Fn(u64),
    {
        let chunk_size = session.chunk_size.min(self.upload_chunk_bytes()).max(1);
        let mut offset = session.offset;
        let mut failures = 0;
        loop {
            let len =
                usize::try_from(chunk_size.min(size.saturating_sub(offset))).unwrap_or(usize::MAX);
            let chunk = read(offset, len)
                .await
                .map_err(|err| ApiError::Transport(format!("reading the file: {err}")))?;
            match self.upload_chunk(session.id, offset, chunk).await {
                Ok(result) => {
                    failures = 0;
                    progress(result.offset);
                    if let Some(node) = result.node {
                        return Ok(Some(node));
                    }
                    if ends == Ends::AtSize && result.offset >= size {
                        return Ok(None);
                    }
                    offset = result.offset;
                }
                Err(err) if err.is_transient() && failures < CHUNK_RETRIES => {
                    failures += 1;
                    tokio::time::sleep(backoff(failures)).await;
                    if let Ok(status) = self.upload_status(session.id).await {
                        offset = status.offset;
                        progress(offset);
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }
}

/// How the server says a chunked upload is complete.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ends {
    /// The last chunk answers the stored node.
    WithNode,
    /// A file of a transfer never becomes a node: it is complete once all its bytes are in.
    AtSize,
}

#[cfg(not(target_arch = "wasm32"))]
fn backoff(failures: u32) -> std::time::Duration {
    std::time::Duration::from_millis(500 * 2u64.pow(failures.saturating_sub(1).min(5)))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use protocol::TokenPair;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, Request, Respond, ResponseTemplate,
        matchers::{method, path},
    };

    use protocol::ErrorCode;

    use super::*;

    const ID: &str = "0193a4a0-0000-7000-8000-000000000001";

    fn node_json(size: u64) -> serde_json::Value {
        json!({
            "id": "0193a4a0-0000-7000-8000-0000000000aa",
            "parent_id": null,
            "kind": "file",
            "name": "big.bin",
            "size": size,
            "sha256": null,
            "mime": "application/octet-stream",
            "version": 1,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        })
    }

    /// Accepts chunks in order, failing the second request once like a dropped connection.
    struct Chunks {
        size: u64,
        received: Arc<std::sync::Mutex<Vec<u8>>>,
        calls: AtomicUsize,
    }

    impl Respond for Chunks {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let offset: usize = request.headers["upload-offset"]
                .to_str()
                .expect("header")
                .parse()
                .expect("offset");
            let mut received = self.received.lock().expect("lock");
            if offset != received.len() {
                return ResponseTemplate::new(409)
                    .set_body_json(json!({ "error": "conflict", "message": "offset" }));
            }
            if call == 1 {
                return ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>");
            }
            received.extend_from_slice(&request.body);
            let offset = received.len() as u64;
            let node = (offset == self.size).then(|| node_json(self.size));
            ResponseTemplate::new(200).set_body_json(json!({ "offset": offset, "node": node }))
        }
    }

    #[tokio::test]
    async fn resumable_upload_retries_a_failed_chunk_and_returns_the_node() {
        let server = MockServer::start().await;
        let content: Vec<u8> = (0..10u8).collect();
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        Mock::given(method("POST"))
            .and(path(paths::UPLOADS))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "id": ID, "offset": 0, "size": 10, "chunk_size": 4 })),
            )
            .mount(&server)
            .await;
        let status_received = received.clone();
        Mock::given(method("GET"))
            .and(path(format!("{}/{ID}", paths::UPLOADS)))
            .respond_with(move |_: &Request| {
                let offset = status_received.lock().expect("lock").len();
                ResponseTemplate::new(200).set_body_json(
                    json!({ "id": ID, "offset": offset, "size": 10, "chunk_size": 4 }),
                )
            })
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("{}/{ID}", paths::UPLOADS)))
            .respond_with(Chunks {
                size: 10,
                received: received.clone(),
                calls: AtomicUsize::new(0),
            })
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.store(TokenPair {
            access_token: "token".to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_in: 900,
        });
        let request = CreateUpload {
            parent_id: None,
            name: "big.bin".to_owned(),
            size: 10,
            modified_at: None,
            mime: None,
            node_id: None,
            keep_both: false,
            version_of: None,
            auto_upload: false,
            transfer_item_id: None,
        };
        let source = content.clone();

        let node = client
            .upload_resumable(
                &request,
                |offset, len| {
                    let start = offset as usize;
                    let chunk = source[start..start + len].to_vec();
                    async move { Ok(chunk) }
                },
                |_| (),
            )
            .await
            .expect("upload");

        assert_eq!(node.size, 10);
        assert_eq!(*received.lock().expect("lock"), content);
    }

    #[tokio::test]
    async fn a_continued_upload_sends_only_what_the_server_lacks() {
        let server = MockServer::start().await;
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = sent.clone();
        Mock::given(method("PATCH"))
            .and(path(format!("{}/{ID}", paths::UPLOADS)))
            .respond_with(move |request: &Request| {
                let offset: u64 = request.headers["upload-offset"]
                    .to_str()
                    .expect("header")
                    .parse()
                    .expect("offset");
                seen.lock().expect("lock").push(offset);
                let offset = offset + request.body.len() as u64;
                let node = (offset == 10).then(|| node_json(10));
                ResponseTemplate::new(200).set_body_json(json!({ "offset": offset, "node": node }))
            })
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.store(TokenPair {
            access_token: "token".to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_in: 900,
        });
        let session = UploadSession {
            id: ID.parse().expect("id"),
            offset: 6,
            size: 10,
            chunk_size: 4,
        };

        let node = client
            .continue_upload(
                session,
                10,
                |_, len| async move { Ok(vec![0; len]) },
                |_| (),
            )
            .await
            .expect("upload");

        assert_eq!(node.size, 10);
        assert_eq!(*sent.lock().expect("lock"), [6]);
    }

    #[tokio::test]
    async fn chunks_stay_within_the_clients_upload_chunk_size() {
        const SIZE: u64 = crate::FALLBACK_UPLOAD_CHUNK_BYTES + 10;
        let server = MockServer::start().await;
        let lengths = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = lengths.clone();
        Mock::given(method("PATCH"))
            .and(path(format!("{}/{ID}", paths::UPLOADS)))
            .respond_with(move |request: &Request| {
                let offset: u64 = request.headers["upload-offset"]
                    .to_str()
                    .expect("header")
                    .parse()
                    .expect("offset");
                seen.lock().expect("lock").push(request.body.len());
                let offset = offset + request.body.len() as u64;
                let node = (offset == SIZE).then(|| node_json(SIZE));
                ResponseTemplate::new(200).set_body_json(json!({ "offset": offset, "node": node }))
            })
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.store(TokenPair {
            access_token: "token".to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_in: 900,
        });
        let session = UploadSession {
            id: ID.parse().expect("id"),
            offset: 0,
            size: SIZE,
            chunk_size: SIZE,
        };

        client
            .continue_upload(
                session,
                SIZE,
                |_, len| async move { Ok(vec![0; len]) },
                |_| (),
            )
            .await
            .expect("upload");

        let fallback = usize::try_from(crate::FALLBACK_UPLOAD_CHUNK_BYTES).expect("fits");
        assert_eq!(*lengths.lock().expect("lock"), [fallback, 10]);
    }

    #[tokio::test]
    async fn a_transfer_item_is_done_once_the_server_has_every_byte() {
        const ITEM: &str = "0193a4a0-0000-7000-8000-0000000000bb";
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(paths::UPLOADS))
            .and(wiremock::matchers::body_partial_json(
                json!({ "transfer_item_id": ITEM, "size": 5 }),
            ))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "id": ID, "offset": 0, "size": 5, "chunk_size": 3 })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let offsets = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = offsets.clone();
        Mock::given(method("PATCH"))
            .and(path(format!("{}/{ID}", paths::UPLOADS)))
            .respond_with(move |request: &Request| {
                let offset: u64 = request.headers["upload-offset"]
                    .to_str()
                    .expect("header")
                    .parse()
                    .expect("offset");
                seen.lock().expect("lock").push(offset);
                let offset = offset + request.body.len() as u64;
                ResponseTemplate::new(200).set_body_json(json!({ "offset": offset }))
            })
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.store(TokenPair {
            access_token: "token".to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_in: 900,
        });

        client
            .upload_transfer_item(
                ITEM.parse().expect("id"),
                "a.txt",
                5,
                |_, len| async move { Ok(vec![0; len]) },
                |_| (),
            )
            .await
            .expect("upload");

        assert_eq!(*offsets.lock().expect("lock"), [0, 3]);
    }

    #[test]
    fn only_passing_failures_are_retried() {
        assert!(ApiError::is_transient(&ApiError::Transport(
            "reset".to_owned()
        )));
        assert!(ApiError::is_transient(&ApiError::Api {
            code: ErrorCode::Conflict,
            message: String::new(),
        }));
        assert!(!ApiError::is_transient(&ApiError::Api {
            code: ErrorCode::PayloadTooLarge,
            message: String::new(),
        }));
        assert!(!ApiError::is_transient(&ApiError::Unauthorized));
    }
}
