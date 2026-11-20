//! Clients read the server's upload chunk size after signing in.

mod common;

use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use sqlx::PgPool;

use common::{UPLOAD_CHUNK, setup};

const CAPABILITIES: &str = "/api/v1/capabilities";

#[sqlx::test(migrations = "../../migrations")]
async fn capabilities_report_the_upload_chunk_size(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;

    let response = app.get(CAPABILITIES, &token).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json()["upload_chunk_bytes"], UPLOAD_CHUNK);
    assert_eq!(response.json()["stream_tickets"], true);
}

#[sqlx::test(migrations = "../../migrations")]
async fn capabilities_need_a_session(pool: PgPool) {
    let app = setup(pool).await;

    let response = app
        .send(Method::GET, CAPABILITIES, None, Body::empty(), None)
        .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}
