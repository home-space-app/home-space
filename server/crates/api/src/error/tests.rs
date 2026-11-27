use std::io;

use axum::{
    http::{HeaderValue, StatusCode, header::RETRY_AFTER},
    response::IntoResponse,
};
use protocol::ErrorCode;

use super::*;

#[test]
fn a_cut_off_body_is_a_bad_request_not_a_server_error() {
    let problem = ApiError::Files(files::BodyError::Interrupted.into()).problem();
    assert_eq!(problem.kind(), Some(ProblemType::BodyInterrupted));
    assert_eq!(problem.status, Some(400));
    assert_eq!(problem.detail(), "the request body was cut off");
}

#[test]
fn a_busy_server_asks_the_client_to_come_back() {
    let response = ApiError::Auth(AuthError::Busy { retry_after: 5 }).into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.headers().get(RETRY_AFTER),
        Some(&HeaderValue::from_static("5"))
    );
}

#[test]
fn a_change_that_needs_a_proof_says_which_ones_will_do() {
    let methods = vec![protocol::AuthMethod::Password];
    let problem = ApiError::Auth(AuthError::ReauthenticationRequired { methods }).problem();
    assert_eq!(problem.kind(), Some(ProblemType::ReauthenticationRequired));
    assert_eq!(problem.status, Some(403));
    assert_eq!(problem.methods, [protocol::AuthMethod::Password]);
    assert_eq!(
        problem.detail(),
        "Confirm it is you: enter your password again."
    );
    let methods = Vec::new();
    let problem = ApiError::Auth(AuthError::ReauthenticationRequired { methods }).problem();
    assert!(problem.methods.is_empty());
    assert_eq!(problem.detail(), "Sign in again to confirm it is you.");
}

#[test]
fn too_many_connections_says_when_to_come_back() {
    let response = ApiError::TooManyConnections.into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().contains_key(RETRY_AFTER));
}

#[test]
fn a_full_quota_is_insufficient_storage_with_the_numbers() {
    let problem = ApiError::Files(FilesError::QuotaExceeded {
        quota: 10,
        available: 2,
    })
    .problem();
    assert_eq!(problem.status, Some(507));
    assert_eq!(problem.code(), Some(ErrorCode::QuotaExceeded));
    assert_eq!((problem.quota, problem.available), (Some(10), Some(2)));
}

#[test]
fn a_server_error_does_not_tell_its_cause() {
    let disk = io::Error::from(io::ErrorKind::StorageFull);
    let problem = ApiError::Files(FilesError::Io(disk)).problem();
    assert_eq!(problem.kind(), Some(ProblemType::Internal));
    assert_eq!(problem.detail(), "Internal server error");
    let provider = ApiError::Auth(AuthError::Oidc("tls handshake to 10.0.0.5".to_owned()));
    assert_eq!(provider.problem().status, Some(502));
    assert!(!provider.problem().detail().contains("10.0.0.5"));
}

#[test]
fn conflicts_are_told_apart_by_type() {
    let kinds = [
        FilesError::Conflict,
        FilesError::LostAndFound,
        FilesError::NotReady,
        FilesError::OffsetMismatch { current: 7 },
    ]
    .map(|err| ApiError::Files(err).problem_type());
    assert_eq!(
        kinds,
        [
            ProblemType::NameTaken,
            ProblemType::LostAndFound,
            ProblemType::TransferNotReady,
            ProblemType::OffsetMismatch,
        ]
    );
    let offset = ApiError::Files(FilesError::OffsetMismatch { current: 7 }).problem();
    assert_eq!(offset.current_offset, Some(7));
}

#[test]
fn every_invalid_field_is_listed_and_summed_up() {
    let problem = ApiError::Invalid(vec![
        FieldError::new("/a", "a is wrong"),
        FieldError::new("/b", "b is wrong"),
    ])
    .problem();
    assert_eq!(problem.status, Some(422));
    assert_eq!(problem.errors.len(), 2);
    assert_eq!(problem.detail(), "a is wrong; b is wrong");
}

#[test]
fn the_body_is_a_problem_document() {
    let response = ApiError::NotFound.into_response();
    assert_eq!(
        response.headers().get("content-type"),
        Some(&HeaderValue::from_static("application/problem+json"))
    );
}

#[test]
fn a_range_outside_the_file_names_the_size() {
    let response = ApiError::RangeNotSatisfiable { size: 12 }.into_response();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        response.headers().get("content-range"),
        Some(&HeaderValue::from_static("bytes */12"))
    );
}
