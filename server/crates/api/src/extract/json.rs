use axum::{
    Json,
    extract::{FromRequest, Request},
};
use protocol::Validate;
use serde::de::DeserializeOwned;

use crate::ApiError;

/// A JSON body that was read and passed its [`Validate`] rules. A body that cannot be read
/// answers `bad-request` (or `unsupported-media-type`), one that breaks a rule answers
/// `validation-failed` with every field at fault.
#[derive(Debug)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(request, state).await?;
        value.validate()?;
        Ok(Self(value))
    }
}
