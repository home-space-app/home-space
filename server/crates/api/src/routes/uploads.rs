//! Resumable chunked uploads, for files larger than a proxy lets one request carry.

use std::{fmt, io};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{
        HeaderMap, HeaderName, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::StreamExt;
use uuid::Uuid;

use db::StoreOptions;
use files::{ChunkOutcome, FilesError, UploadStatus, UploadTarget};
use protocol::{
    ChunkResult, CreateUpload, UploadInfo, UploadSession, UploadsQuery, paths::UPLOAD_OFFSET,
};

use crate::{
    ApiError, AppState,
    extract::AuthUser,
    routes::{
        nodes::{body_stream, publish_stored},
        transfers::publish_ready,
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/uploads", get(list).post(create))
        .route(
            "/uploads/{id}",
            get(status)
                .patch(append)
                .delete(cancel)
                .layer(DefaultBodyLimit::disable()),
        )
}

fn offset_header() -> HeaderName {
    HeaderName::from_static("upload-offset")
}

/// Renders `body` with the upload's offset in a header, never cached by a proxy.
fn with_offset(status: StatusCode, offset: u64, body: impl IntoResponse) -> Response {
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(offset_header(), HeaderValue::from(offset));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn session(status: UploadStatus, chunk_size: u64) -> UploadSession {
    UploadSession {
        id: status.id,
        offset: status.offset,
        size: status.size,
        chunk_size,
    }
}

/// The user's unfinished uploads, or only this device's. A session without a device has none
/// of its own.
async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<UploadsQuery>,
) -> Result<Json<Vec<UploadInfo>>, ApiError> {
    let uploads = match (query.this_device, user.device_id) {
        (false, _) => state.files.uploads(user.id, None).await?,
        (true, Some(device)) => state.files.uploads(user.id, Some(device)).await?,
        (true, None) => Vec::new(),
    };
    Ok(Json(uploads.into_iter().map(upload_info).collect()))
}

fn upload_info(upload: db::Upload) -> UploadInfo {
    UploadInfo {
        id: upload.id,
        parent_id: upload.parent_id,
        node_id: upload.node_id,
        name: upload.name,
        size: u64::try_from(upload.size).unwrap_or_default(),
        device_id: upload.device_id,
        transfer_item_id: upload.transfer_item_id,
        updated_at: upload.updated_at,
    }
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateUpload>,
) -> Result<Response, ApiError> {
    let target = match (request.transfer_item_id, request.node_id) {
        (Some(item), _) => UploadTarget::TransferItem(item),
        (None, Some(id)) => UploadTarget::Replace(id),
        (None, None) => UploadTarget::New {
            parent: request.parent_id,
            name: request.name,
            options: StoreOptions {
                keep_both: request.keep_both,
                version_of: request.version_of,
                auto_upload: request.auto_upload,
            },
        },
    };
    let status = state
        .files
        .create_upload(
            user.id,
            user.device_id,
            target,
            request.size,
            request.mime.as_deref(),
            request.modified_at,
        )
        .await?;
    let body = Json(session(status, state.files.chunk_bytes()));
    Ok(with_offset(StatusCode::CREATED, status.offset, body))
}

async fn status(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let status = state.files.upload_status(user.id, id).await?;
    let body = Json(session(status, state.files.chunk_bytes()));
    Ok(with_offset(StatusCode::OK, status.offset, body))
}

/// A chunk body longer than the chunk size the server asked for.
#[derive(Debug)]
struct ChunkTooLarge;

impl fmt::Display for ChunkTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("chunk is larger than the chunk size")
    }
}

impl std::error::Error for ChunkTooLarge {}

async fn append(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Body,
) -> Result<Response, ApiError> {
    let offset = headers
        .get(UPLOAD_OFFSET)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .ok_or_else(|| {
            ApiError::BadRequest(format!("a numeric {UPLOAD_OFFSET} header is required"))
        })?;
    let limit = state.files.chunk_bytes();
    let too_large = ApiError::Files(FilesError::TooLarge { limit });
    let declared_len = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared_len.is_some_and(|len| len > limit) {
        return Err(too_large);
    }
    let mut received = 0u64;
    let chunk = body_stream(body).map(move |piece| {
        let piece = piece?;
        received += piece.len() as u64;
        if received > limit {
            Err(io::Error::other(ChunkTooLarge))
        } else {
            Ok(piece)
        }
    });
    match state
        .files
        .on_device(user.device_id)
        .append_chunk(user.id, id, offset, chunk)
        .await
    {
        Ok(ChunkOutcome::Partial { offset }) => Ok(with_offset(
            StatusCode::OK,
            offset,
            Json(ChunkResult { offset, node: None }),
        )),
        Ok(ChunkOutcome::Complete(stored)) => {
            let (status, Json(node)) = publish_stored(&state, user.id, stored).await;
            let offset = u64::try_from(node.size).unwrap_or_default();
            let body = Json(ChunkResult {
                offset,
                node: Some(node),
            });
            Ok(with_offset(status, offset, body))
        }
        Ok(ChunkOutcome::Transferred { size, ready }) => {
            if let Some(transfer) = ready {
                publish_ready(&state, transfer);
            }
            let body = Json(ChunkResult {
                offset: size,
                node: None,
            });
            Ok(with_offset(StatusCode::OK, size, body))
        }
        Err(FilesError::OffsetMismatch { current }) => {
            let error = ApiError::Files(FilesError::OffsetMismatch { current });
            Ok(with_offset(StatusCode::CONFLICT, current, error))
        }
        Err(FilesError::Io(err))
            if err
                .get_ref()
                .is_some_and(|inner| inner.is::<ChunkTooLarge>()) =>
        {
            Err(too_large)
        }
        Err(err) => Err(err.into()),
    }
}

async fn cancel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.files.cancel_upload(user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
