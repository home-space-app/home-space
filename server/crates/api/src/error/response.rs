//! Writing an [`ApiError`] as an `application/problem+json` response.

use axum::{
    body::Body,
    extract::Request,
    http::{
        HeaderValue, StatusCode,
        header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RETRY_AFTER},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};

use protocol::{PROBLEM_CONTENT_TYPE, Problem, ProblemType};

use super::ApiError;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let problem = self.problem();
        match problem.kind() {
            Some(kind) if kind.hides_detail() => {
                tracing::error!(error = %self, "request failed");
            }
            Some(ProblemType::BodyInterrupted) => {
                tracing::warn!(error = %self, "request body interrupted");
            }
            _ => {}
        }
        let mut response = problem_response(&problem);
        if let Self::RangeNotSatisfiable { size } = self
            && let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}"))
        {
            response.headers_mut().insert(CONTENT_RANGE, value);
        }
        // Kept so `with_instance` can add the request path once the response is on its way out.
        response.extensions_mut().insert(problem);
        response
    }
}

/// `problem` as a response with its status, media type and `Retry-After`.
fn problem_response(problem: &Problem) -> Response {
    let status = problem
        .status
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = serde_json::to_vec(problem).unwrap_or_default();
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(PROBLEM_CONTENT_TYPE));
    if let Some(seconds) = problem.retry_after {
        headers.insert(RETRY_AFTER, HeaderValue::from(seconds));
    }
    response
}

/// Middleware that fills in the `instance` of every problem with the path of the request that
/// failed. Handlers and extractors do not see the path, so it is added here, on the way out.
pub async fn with_instance(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    if let Some(mut problem) = response.extensions_mut().remove::<Problem>() {
        problem.instance = Some(path);
        if let Ok(body) = serde_json::to_vec(&problem) {
            response.headers_mut().remove(CONTENT_LENGTH);
            *response.body_mut() = Body::from(body);
        }
    }
    response
}
