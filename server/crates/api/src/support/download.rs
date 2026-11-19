//! Sending a file's bytes, one `Range` of them when asked, so an interrupted download can
//! resume.

use std::io::SeekFrom;

use axum::{
    body::Body,
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
        },
    },
    response::{IntoResponse, Response},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::ApiError;

const FALLBACK_MIME: &str = "application/octet-stream";
const BYTES: &str = "bytes";

/// What is sent as the downloaded file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Download<'a> {
    pub name: &'a str,
    pub mime: Option<&'a str>,
    pub size: u64,
}

/// `file` as an attachment called `download.name`, the whole of it or the one range the
/// request's `Range` header asks for. A range that cannot be satisfied answers 416.
pub(crate) async fn ranged(
    request: &HeaderMap,
    download: Download<'_>,
    mut file: tokio::fs::File,
) -> Result<Response, ApiError> {
    let size = download.size;
    let range = match request.get(RANGE).and_then(|value| value.to_str().ok()) {
        Some(value) => match parse_range(value, size) {
            Some(range) => Some(range),
            None => {
                let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                if let Ok(value) = HeaderValue::from_str(&format!("{BYTES} */{size}")) {
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
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static(BYTES));
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(value) =
            HeaderValue::from_str(&format!("{BYTES} {start}-{}/{size}", start + len - 1))
    {
        headers.insert(CONTENT_RANGE, value);
    }
    let mime = download.mime.filter(|mime| !mime.is_empty());
    if let Ok(value) = HeaderValue::from_str(mime.unwrap_or(FALLBACK_MIME)) {
        headers.insert(CONTENT_TYPE, value);
    }
    let disposition = format!(
        "attachment; filename*=UTF-8''{}",
        percent_encode(download.name)
    );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    Ok(response)
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

pub(crate) fn attachment(name: &str, mime: &str, size: i64, file: tokio::fs::File) -> Response {
    let content_type = if mime.is_empty() { FALLBACK_MIME } else { mime };
    let disposition = format!("attachment; filename*=UTF-8''{}", percent_encode(name));
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    let headers = response.headers_mut();
    headers.insert(CONTENT_LENGTH, HeaderValue::from(size.unsigned_abs()));
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    response
}

pub(crate) fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
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
