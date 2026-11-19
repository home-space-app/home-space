use axum::{
    extract::{FromRequestParts, Query},
    http::request::Parts,
};
use protocol::Validate;
use serde::de::DeserializeOwned;

use crate::ApiError;

/// A query string that was read and passed its [`Validate`] rules, answering failures the
/// way [`ApiJson`](super::ApiJson) does.
#[derive(Debug)]
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state).await?;
        value.validate()?;
        Ok(Self(value))
    }
}
