//! Files and folders sent to a device.

use std::{
    fmt::Write as _,
    fs::File,
    io::{self, BufWriter, SeekFrom, Write},
    path::PathBuf,
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
        },
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use time::PrimitiveDateTime;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::mpsc,
};
use tokio_util::io::ReaderStream;
use uuid::Uuid;
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

use notifications::Event;
use protocol::{
    CreateTransfer, CreatedTransfer, CreatedTransferItem, IncomingQuery, Node, NodeKind,
    SaveTransfer, TicketRequest, Transfer, TransferItem, TransferState, TransferSummary, UserRef,
};

use crate::{
    ApiError, AppState,
    extract::{AuthUser, TicketQuery},
    routes::nodes::{percent_encode, publish_stored},
};

const FALLBACK_MIME: &str = "application/octet-stream";
const ZIP_MIME: &str = "application/zip";
/// How much of an archive is gathered before it is sent on.
const ARCHIVE_CHUNK_BYTES: usize = 64 * 1024;
/// How many chunks of an archive may wait for a slow client.
const ARCHIVE_QUEUE: usize = 4;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/transfers", post(create))
        .route("/transfers/incoming", get(incoming))
        .route("/transfers/outgoing", get(outgoing))
        .route("/transfers/{id}", get(get_transfer).delete(cancel))
        .route("/transfers/{id}/items/{item}/content", get(content))
        .route("/transfers/{id}/archive", get(archive))
        .route("/transfers/{id}/decline", post(decline))
        .route("/transfers/{id}/received", post(received))
        .route("/transfers/{id}/save", post(save))
}

pub(crate) fn summary(transfer: db::Transfer) -> TransferSummary {
    TransferSummary {
        id: transfer.id,
        sender: UserRef {
            id: transfer.sender_id,
            username: transfer.sender_name,
        },
        sender_device: transfer.sender_device,
        sender_device_name: transfer.sender_device_name,
        recipient: UserRef {
            id: transfer.recipient_id,
            username: transfer.recipient_name,
        },
        recipient_device: transfer.recipient_device,
        recipient_device_name: transfer.recipient_device_name,
        state: transfer.state,
        total_bytes: u64::try_from(transfer.total_bytes).unwrap_or_default(),
        item_count: u32::try_from(transfer.item_count).unwrap_or_default(),
        file_count: u32::try_from(transfer.file_count).unwrap_or_default(),
        name: transfer.name,
        mime: transfer.mime,
        message: transfer.message,
        created_at: transfer.created_at,
        expires_at: transfer.expires_at,
    }
}

fn item(item: db::TransferItem) -> TransferItem {
    TransferItem {
        id: item.id,
        path: item.path,
        kind: item.kind,
        size: u64::try_from(item.size).unwrap_or_default(),
        sha256: item.sha256.as_deref().map(to_hex),
        mime: item.mime,
        modified_at: item.modified_at,
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Tells the recipient a transfer is ready, and the sender that it is.
pub(crate) fn publish_ready(state: &AppState, transfer: db::Transfer) {
    let (id, sender) = (transfer.id, transfer.sender_id);
    let recipient = transfer.recipient_id;
    state.hub.publish(
        recipient,
        Event::TransferReceived {
            transfer: summary(transfer),
        },
    );
    if sender != recipient {
        state.hub.publish(
            sender,
            Event::TransferChanged {
                id,
                state: TransferState::Ready,
            },
        );
    }
}

/// Tells both sides a transfer changed state.
pub(crate) fn publish_changed(
    state: &AppState,
    id: Uuid,
    sender: db::UserId,
    recipient: db::UserId,
    to: TransferState,
) {
    state
        .hub
        .publish(sender, Event::TransferChanged { id, state: to });
    if recipient != sender {
        state
            .hub
            .publish(recipient, Event::TransferChanged { id, state: to });
    }
}

fn publish_finished(state: &AppState, transfer: &db::Transfer) {
    publish_changed(
        state,
        transfer.id,
        transfer.sender_id,
        transfer.recipient_id,
        transfer.state,
    );
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateTransfer>,
) -> Result<(StatusCode, Json<CreatedTransfer>), ApiError> {
    let (transfer, items) = state
        .files
        .create_transfer(user.id, user.device_id, request)
        .await?;
    let created = CreatedTransfer {
        id: transfer.id,
        state: transfer.state,
        items: items
            .into_iter()
            .map(|item| CreatedTransferItem {
                upload_needed: item.kind == NodeKind::File && item.sha256.is_none(),
                id: item.id,
                path: item.path,
            })
            .collect(),
    };
    if transfer.state == TransferState::Ready {
        publish_ready(&state, transfer);
    }
    Ok((StatusCode::CREATED, Json(created)))
}

/// The transfers waiting for the caller: ready, or declined until they expire.
async fn incoming(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<IncomingQuery>,
) -> Result<Json<Vec<TransferSummary>>, ApiError> {
    let transfers = match (query.this_device, user.device_id) {
        (false, _) => state.files.incoming_transfers(user.id, None).await?,
        (true, Some(device)) => {
            state
                .files
                .incoming_transfers(user.id, Some(device))
                .await?
        }
        (true, None) => Vec::new(),
    };
    Ok(Json(transfers.into_iter().map(summary).collect()))
}

async fn outgoing(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<TransferSummary>>, ApiError> {
    let transfers = state.files.outgoing_transfers(user.id).await?;
    Ok(Json(transfers.into_iter().map(summary).collect()))
}

async fn get_transfer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Transfer>, ApiError> {
    let (transfer, items) = state.files.transfer(user.id, id).await?;
    Ok(Json(Transfer {
        summary: summary(transfer),
        items: items.into_iter().map(item).collect(),
    }))
}

async fn cancel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let transfer = state.files.cancel_transfer(user.id, id).await?;
    publish_finished(&state, &transfer);
    Ok(StatusCode::NO_CONTENT)
}

async fn decline(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let transfer = state.files.decline_transfer(user.id, id).await?;
    publish_finished(&state, &transfer);
    Ok(StatusCode::NO_CONTENT)
}

async fn received(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let transfer = state.files.confirm_transfer(user.id, id).await?;
    publish_finished(&state, &transfer);
    Ok(StatusCode::NO_CONTENT)
}

/// Saves the transfer into the caller's tree; answers the nodes created at the top.
async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    body: Option<Json<SaveTransfer>>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let parent = body.and_then(|Json(body)| body.parent_id);
    let saved = state
        .files
        .on_device(user.device_id)
        .save_transfer(user.id, id, parent)
        .await?;
    publish_finished(&state, &saved.transfer);
    let mut top = Vec::new();
    for folder in saved.folders {
        state.index_name(&folder).await;
        let is_top = folder.parent_id == parent;
        let node = Node::from(folder);
        state
            .hub
            .publish(user.id, Event::NodeCreated { node: node.clone() });
        if is_top {
            top.push(node);
        }
    }
    for outcome in saved.files {
        let (_, Json(node)) = publish_stored(&state, user.id, outcome).await;
        if node.parent_id == parent {
            top.push(node);
        }
    }
    Ok(Json(top))
}

/// A file of a ready transfer, for its recipient. Answers one `Range` of bytes when asked,
/// so an interrupted download can resume.
async fn content(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    Path((id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let caller = match query.ticket {
        Some(ticket) => {
            state
                .auth
                .verify_ticket(
                    &ticket,
                    &TicketRequest::TransferItem {
                        transfer_id: id,
                        item_id,
                    },
                )
                .await?
                .user_id
        }
        None => AuthUser::from_headers(&headers, &state).await?.id,
    };
    let (item, mut file) = state.files.open_transfer_item(caller, id, item_id).await?;
    let size = u64::try_from(item.size).unwrap_or_default();
    let name = item
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&item.path)
        .to_owned();
    let range = match headers.get(RANGE).and_then(|value| value.to_str().ok()) {
        Some(value) => match parse_range(value, size) {
            Some(range) => Some(range),
            None => {
                let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                if let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}")) {
                    response.headers_mut().insert(CONTENT_RANGE, value);
                }
                return Ok(response);
            }
        },
        None => None,
    };
    let (status, start, len) = match range {
        Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        None => (StatusCode::OK, 0, size),
    };
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|err| ApiError::Internal(err.to_string()))?;
    }
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file.take(len))));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(value) =
            HeaderValue::from_str(&format!("bytes {start}-{}/{size}", start + len - 1))
    {
        headers.insert(CONTENT_RANGE, value);
    }
    let mime = item.mime.as_deref().filter(|mime| !mime.is_empty());
    if let Ok(value) = HeaderValue::from_str(mime.unwrap_or(FALLBACK_MIME)) {
        headers.insert(CONTENT_TYPE, value);
    }
    let disposition = format!("attachment; filename*=UTF-8''{}", percent_encode(&name));
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

/// Every file and folder of a ready transfer as one zip, for its recipient. The zip is made
/// while it is sent, without compression, so it has no length up front and cannot resume.
async fn archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let caller = match query.ticket {
        Some(ticket) => {
            state
                .auth
                .verify_ticket(&ticket, &TicketRequest::TransferArchive { transfer_id: id })
                .await?
                .user_id
        }
        None => AuthUser::from_headers(&headers, &state).await?.id,
    };
    let (transfer, items) = state.files.transfer_archive(caller, id).await?;
    let (sender, receiver) = mpsc::channel::<io::Result<Bytes>>(ARCHIVE_QUEUE);
    tokio::task::spawn_blocking(move || {
        let out = BufWriter::with_capacity(ARCHIVE_CHUNK_BYTES, ChunkWriter(sender.clone()));
        if let Err(err) = write_archive(out, &items) {
            tracing::warn!(transfer = %id, error = %err, "transfer archive failed");
            // Ends the response with an error, so the client does not keep a cut-off zip.
            let _ = sender.blocking_send(Err(io::Error::other(err)));
        }
    });
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(ZIP_MIME));
    let disposition = format!(
        "attachment; filename*=UTF-8''{}",
        percent_encode(&archive_name(&transfer))
    );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

/// The file name of a transfer's zip: its only top-level item, or who sent it.
fn archive_name(transfer: &db::Transfer) -> String {
    let base = match &transfer.name {
        Some(name) => name.clone(),
        None => format!("From {}", transfer.sender_name),
    };
    format!("{base}.zip")
}

/// Hands what is written to the response, a chunk at a time.
struct ChunkWriter(mpsc::Sender<io::Result<Bytes>>);

impl Write for ChunkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(buf)))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes `items` into a zip on `out`: folders as entries of their own, files with the bytes
/// at their path.
fn write_archive(
    out: impl Write,
    items: &[(db::TransferItem, PathBuf)],
) -> zip::result::ZipResult<()> {
    let mut zip = ZipWriter::new_stream(out);
    for (item, path) in items {
        let mut options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(item.size >= i64::from(u32::MAX));
        if let Some(modified) = item
            .modified_at
            .and_then(|at| DateTime::try_from(PrimitiveDateTime::new(at.date(), at.time())).ok())
        {
            options = options.last_modified_time(modified);
        }
        match item.kind {
            NodeKind::Folder => zip.add_directory(item.path.as_str(), options)?,
            NodeKind::File => {
                zip.start_file(item.path.as_str(), options)?;
                io::copy(&mut File::open(path)?, &mut zip)?;
            }
        }
    }
    zip.finish()?.into_inner().flush()?;
    Ok(())
}

/// Parses a single `bytes=` range of a file of `size` bytes into inclusive bounds; `None`
/// when it cannot be satisfied.
fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let suffix = suffix.parse::<u64>().ok().filter(|&n| n > 0)?;
            (size.saturating_sub(suffix), size - 1)
        }
        (start, "") => (start.parse().ok()?, size - 1),
        (start, end) => (start.parse().ok()?, end.parse::<u64>().ok()?.min(size - 1)),
    };
    (start <= end && start < size).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_parsed_into_inclusive_bounds() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-500", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=5-1", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }
}
