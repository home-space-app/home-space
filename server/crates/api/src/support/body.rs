//! Reading request bodies.

use std::io;

use axum::{
    body::Body,
    http::{HeaderMap, header::CONTENT_TYPE},
};
use futures_util::TryStreamExt;

pub(crate) fn body_stream(
    body: Body,
) -> impl futures_util::Stream<Item = io::Result<axum::body::Bytes>> {
    body.into_data_stream().map_err(io::Error::other)
}

pub(crate) fn declared_type(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
}
