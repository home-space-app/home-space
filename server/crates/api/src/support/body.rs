//! Reading request bodies.

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, header::CONTENT_TYPE},
};
use futures_util::{Stream, StreamExt, TryStreamExt};

use files::BodyError;

/// The bytes of a request body. Any error reading it means the body stopped coming.
pub(crate) fn body_stream(body: Body) -> impl Stream<Item = Result<Bytes, BodyError>> {
    body.into_data_stream().map_err(|_| BodyError::Interrupted)
}

/// Like [`body_stream`], refusing the body with [`BodyError::TooLarge`] once more than
/// `limit` bytes of it came.
pub(crate) fn limited_body_stream(
    body: Body,
    limit: u64,
) -> impl Stream<Item = Result<Bytes, BodyError>> {
    let mut received = 0u64;
    body_stream(body).map(move |piece| {
        let piece = piece?;
        received = received.saturating_add(piece.len() as u64);
        if received > limit {
            Err(BodyError::TooLarge { limit })
        } else {
            Ok(piece)
        }
    })
}

pub(crate) fn declared_type(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
}
