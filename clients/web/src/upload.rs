use std::{cell::Cell, cell::RefCell, rc::Rc};

use gloo_timers::future::TimeoutFuture;
use serde::de::DeserializeOwned;
use tokio::sync::oneshot;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Blob, File, ProgressEvent, XmlHttpRequest};

use api_client::{ApiError, Client};
use protocol::{ChunkResult, CreateUpload, ErrorBody, ErrorCode, Node, NodeId, paths};

use crate::session::storage;

const CONTENT_TYPE: &str = "Content-Type";
const FALLBACK_MIME: &str = "application/octet-stream";
const STATUS_SUCCESS: std::ops::Range<u16> = 200..300;
const STATUS_UNAUTHORIZED: u16 = 401;
/// localStorage prefix of the ids of unfinished chunked uploads, so dropping the same file
/// again (even after a reload) continues where it stopped.
const RESUME_KEY_PREFIX: &str = "homespace.upload.";
/// How often one chunk is retried before the upload gives up.
const CHUNK_RETRIES: u32 = 5;

thread_local! {
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// A fresh identifier for an upload row.
pub fn next_id() -> u64 {
    NEXT_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    })
}

/// Progress callback receiving a fraction between 0 and 1.
pub type OnProgress = Rc<dyn Fn(f64)>;

/// Progress of one request: the bytes sent and the total.
type OnBytes = Rc<dyn Fn(f64, f64)>;

struct Outcome {
    status: u16,
    body: String,
}

/// One XHR to send; `body` goes out as is, so progress events track it.
struct Request<'a> {
    method: &'a str,
    url: String,
    headers: Vec<(&'a str, String)>,
    body: &'a Blob,
}

/// Uploads `file` into `parent` with progress. Files larger than one chunk go up in
/// resumable chunks, the rest in a single request.
pub async fn upload(
    client: &Client,
    file: &File,
    parent: Option<NodeId>,
    on_progress: &OnProgress,
) -> Result<Node, ApiError> {
    if file_size(file) > client.upload_chunk_bytes() {
        return upload_chunked(client, file, parent, on_progress).await;
    }
    let name = String::from(js_sys::encode_uri_component(&file.name()));
    let mut url = format!("{}?name={name}", client.url(paths::FILES));
    if let Some(parent) = parent {
        url.push_str(&format!("&parent_id={parent}"));
    }
    let request = Request {
        method: "POST",
        url,
        headers: vec![(CONTENT_TYPE, mime_of(file))],
        body: file,
    };
    let progress = Rc::clone(on_progress);
    let on_bytes: OnBytes = Rc::new(move |loaded, total| progress(loaded / total));
    let outcome = send_authed(client, &request, &on_bytes).await?;
    decode(&outcome)
}

async fn upload_chunked(
    client: &Client,
    file: &File,
    parent: Option<NodeId>,
    on_progress: &OnProgress,
) -> Result<Node, ApiError> {
    let size = file_size(file);
    let key = resume_key(file, parent);
    let session = match resumable(client, &key, size).await? {
        Some(session) => session,
        None => {
            let request = CreateUpload {
                parent_id: parent,
                name: file.name(),
                size,
                modified_at: None,
                mime: Some(mime_of(file)),
                node_id: None,
                keep_both: false,
                version_of: None,
                auto_upload: false,
                transfer_item_id: None,
            };
            let session = client.create_upload(&request).await?;
            remember(&key, Some(&session.id.to_string()));
            session
        }
    };
    let url = client.url(&paths::upload(session.id));
    let chunk_size = session.chunk_size.min(client.upload_chunk_bytes()).max(1);
    let fraction = move |done: u64| done as f64 / size as f64;
    let mut offset = session.offset;
    let mut failures = 0;
    on_progress(fraction(offset));
    loop {
        let end = offset.saturating_add(chunk_size).min(size);
        let chunk = file
            .slice_with_f64_and_f64(offset as f64, end as f64)
            .map_err(transport)?;
        let request = Request {
            method: "PATCH",
            url: url.clone(),
            headers: vec![(paths::UPLOAD_OFFSET, offset.to_string())],
            body: &chunk,
        };
        let progress = Rc::clone(on_progress);
        let start = offset;
        let on_bytes: OnBytes = Rc::new(move |loaded, _| {
            progress(fraction(start) + loaded / size as f64);
        });
        let sent = send_authed(client, &request, &on_bytes)
            .await
            .and_then(|outcome| decode::<ChunkResult>(&outcome));
        match sent {
            Ok(result) => {
                failures = 0;
                on_progress(fraction(result.offset));
                if let Some(node) = result.node {
                    remember(&key, None);
                    return Ok(node);
                }
                offset = result.offset;
            }
            Err(err) if err.is_transient() && failures < CHUNK_RETRIES => {
                failures += 1;
                TimeoutFuture::new(500 * 2u32.pow(failures - 1)).await;
                if let Ok(status) = client.upload_status(session.id).await {
                    offset = status.offset;
                    on_progress(fraction(offset));
                }
            }
            // Still resumable later: dropping the same file again continues from the server.
            Err(err) if err.is_transient() => return Err(err),
            Err(err) => {
                remember(&key, None);
                let _ = client.cancel_upload(session.id).await;
                return Err(err);
            }
        }
    }
}

/// The unfinished upload of this file saved under `key`, if the server still has it.
async fn resumable(
    client: &Client,
    key: &str,
    size: u64,
) -> Result<Option<protocol::UploadSession>, ApiError> {
    let Some(id) = saved(key).and_then(|id| id.parse::<uuid::Uuid>().ok()) else {
        return Ok(None);
    };
    match client.upload_status(id).await {
        Ok(session) if session.size == size => Ok(Some(session)),
        Ok(_)
        | Err(ApiError::Api {
            code: ErrorCode::NotFound,
            ..
        }) => {
            remember(key, None);
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

/// Identifies a local file across page loads: the same name, size and modification time
/// dropped into the same folder.
fn resume_key(file: &File, parent: Option<NodeId>) -> String {
    let folder = parent.map_or_else(|| "root".to_owned(), |id| id.to_string());
    format!(
        "{RESUME_KEY_PREFIX}{folder}:{}:{}:{}",
        file_size(file),
        file.last_modified(),
        file.name()
    )
}

fn saved(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

fn remember(key: &str, upload_id: Option<&str>) {
    if let Some(storage) = storage() {
        let _ = match upload_id {
            Some(id) => storage.set_item(key, id),
            None => storage.remove_item(key),
        };
    }
}

fn file_size(file: &File) -> u64 {
    file.size() as u64
}

fn mime_of(file: &File) -> String {
    let mime = file.type_();
    if mime.is_empty() {
        FALLBACK_MIME.to_owned()
    } else {
        mime
    }
}

fn decode<T: DeserializeOwned>(outcome: &Outcome) -> Result<T, ApiError> {
    if STATUS_SUCCESS.contains(&outcome.status) {
        serde_json::from_str(&outcome.body).map_err(|err| ApiError::Decode(err.to_string()))
    } else {
        Err(failure(outcome))
    }
}

fn failure(outcome: &Outcome) -> ApiError {
    if outcome.status == STATUS_UNAUTHORIZED {
        ApiError::Unauthorized
    } else {
        match serde_json::from_str::<ErrorBody>(&outcome.body) {
            Ok(body) => ApiError::Api {
                code: body.error,
                message: body.message,
            },
            // Status 0 is a request that got no response at all.
            Err(_) if outcome.status == 0 => {
                ApiError::Transport("the upload got no response".to_owned())
            }
            Err(_) => ApiError::Status {
                status: outcome.status,
                reason: String::new(),
            },
        }
    }
}

fn transport(err: JsValue) -> ApiError {
    ApiError::Transport(format!("{err:?}"))
}

/// Sends `request` with the session's token, refreshing the session and retrying once on a
/// 401. `on_progress` gets the bytes sent and the total.
async fn send_authed(
    client: &Client,
    request: &Request<'_>,
    on_progress: &OnBytes,
) -> Result<Outcome, ApiError> {
    let token = client.access_token().ok_or(ApiError::Unauthorized)?;
    let outcome = send(&token, request, on_progress).await?;
    if outcome.status != STATUS_UNAUTHORIZED {
        return Ok(outcome);
    }
    client.refresh_session(&token).await?;
    let token = client.access_token().ok_or(ApiError::Unauthorized)?;
    send(&token, request, on_progress).await
}

async fn send(
    token: &str,
    request: &Request<'_>,
    on_progress: &OnBytes,
) -> Result<Outcome, ApiError> {
    let xhr = XmlHttpRequest::new().map_err(transport)?;
    xhr.open_with_async(request.method, &request.url, true)
        .map_err(transport)?;
    xhr.set_request_header("Authorization", &format!("Bearer {token}"))
        .map_err(transport)?;
    for (name, value) in &request.headers {
        xhr.set_request_header(name, value).map_err(transport)?;
    }

    let (done_tx, done) = oneshot::channel::<bool>();
    let done_tx = Rc::new(RefCell::new(Some(done_tx)));
    let finish = |ok: bool| {
        let done_tx = Rc::clone(&done_tx);
        Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = done_tx.borrow_mut().take() {
                let _ = tx.send(ok);
            }
        })
    };
    let on_load = finish(true);
    let on_error = finish(false);
    let progress = Rc::clone(on_progress);
    let on_upload_progress =
        Closure::<dyn FnMut(ProgressEvent)>::new(move |event: ProgressEvent| {
            if event.length_computable() && event.total() > 0.0 {
                progress(event.loaded(), event.total());
            }
        });
    xhr.set_onload(Some(on_load.as_ref().unchecked_ref()));
    xhr.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    xhr.set_onabort(Some(on_error.as_ref().unchecked_ref()));
    xhr.upload()
        .map_err(transport)?
        .set_onprogress(Some(on_upload_progress.as_ref().unchecked_ref()));
    xhr.send_with_opt_blob(Some(request.body))
        .map_err(transport)?;

    match done.await {
        Ok(true) => Ok(Outcome {
            status: xhr.status().map_err(transport)?,
            body: xhr.response_text().map_err(transport)?.unwrap_or_default(),
        }),
        _ => Err(ApiError::Transport(
            "network error during upload".to_owned(),
        )),
    }
}
