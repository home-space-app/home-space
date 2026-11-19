use std::io;

use axum::{
    http::{HeaderValue, StatusCode, header::RETRY_AFTER},
    response::IntoResponse,
};
use protocol::ErrorCode;

use super::*;

#[test]
fn a_cut_off_body_is_a_bad_request_not_a_server_error() {
    let cut = io::Error::other(axum::Error::new(io::Error::from(
        io::ErrorKind::ConnectionReset,
    )));
    let problem = ApiError::Files(FilesError::Io(cut)).problem();
    assert_eq!(problem.kind(), Some(ProblemType::BodyInterrupted));
    assert_eq!(problem.status, Some(400));
    assert_eq!(problem.detail(), INTERRUPTED_BODY);
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
fn database_errors_map_to_one_status_each() {
    assert!(matches!(
        ApiError::from(DbError::NotFound),
        ApiError::NotFound
    ));
    assert!(matches!(
        ApiError::from(DbError::Pruned),
        ApiError::ChangesPruned
    ));
    assert!(matches!(
        ApiError::from(DbError::Conflict),
        ApiError::Internal(_)
    ));
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
