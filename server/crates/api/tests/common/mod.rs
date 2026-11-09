// Each integration test binary uses a different subset of these helpers.
#![allow(dead_code)]

pub mod idp;

use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header::AUTHORIZATION},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use api::AppState;
use auth::{AuthConfig, AuthService};
use db::Db;
use files::{BlobStore, FileService};
use notifications::Hub;
use search::SearchIndex;

pub const PASSWORD: &str = "password-123";
/// Chunk size of resumable uploads in tests, small so a short text spans several chunks.
pub const UPLOAD_CHUNK: u64 = 4;

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    _storage: tempfile::TempDir,
}

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Response {
    /// The `Location` header of a redirect.
    pub fn location(&self) -> String {
        self.headers
            .get("location")
            .and_then(|value| value.to_str().ok())
            .expect("location header")
            .to_owned()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

pub async fn setup(pool: PgPool) -> TestApp {
    setup_with(pool, |auth| auth).await
}

/// Like [`setup`], letting `configure` adjust the auth service (e.g. to enable single sign-on).
pub async fn setup_with(
    pool: PgPool,
    configure: impl FnOnce(AuthService) -> AuthService,
) -> TestApp {
    let db = Db::from_pool(pool);
    let storage = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(storage.path()).await.expect("store");
    let (search, _worker) = SearchIndex::start(db.clone(), store.clone(), Vec::new());
    let config = AuthConfig {
        jwt_secret: b"integration-test-secret-integration".to_vec(),
        previous_jwt_secrets: Vec::new(),
        access_ttl: time::Duration::minutes(15),
        refresh_ttl: time::Duration::days(30),
    };
    let state = AppState {
        auth: configure(AuthService::new(db.clone(), config).expect("auth")),
        files: FileService::new(db.clone(), store, 1024 * 1024).with_chunk_bytes(UPLOAD_CHUNK),
        hub: Hub::new(),
        search,
        info: std::sync::Arc::new(api::ServerInfo {
            version: "0.0.0-test".to_owned(),
            started_at: time::OffsetDateTime::now_utc(),
            config: protocol::ServerConfig {
                max_upload_bytes: 1024 * 1024,
                upload_chunk_bytes: UPLOAD_CHUNK,
                upload_expiry_hours: 24,
                changes_retention_days: None,
                password_login: true,
                oidc: None,
                access_ttl_secs: 900,
                refresh_ttl_secs: 30 * 24 * 3600,
                previous_jwt_secret: false,
                public_url: None,
                bind: "127.0.0.1:0".to_owned(),
                published_port: None,
                storage: protocol::StorageBackend::Local,
            },
        }),
        db,
    };
    TestApp {
        router: api::router(state.clone()),
        state,
        _storage: storage,
    }
}

impl TestApp {
    pub async fn send(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Body,
        content_type: Option<&str>,
    ) -> Response {
        self.send_with(method, uri, token, body, content_type, &[])
            .await
    }

    /// Like [`Self::send`], with extra request headers.
    pub async fn send_with(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Body,
        content_type: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> Response {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in extra_headers {
            request = request.header(*name, *value);
        }
        if let Some(token) = token {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(body).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec();
        Response {
            status,
            headers,
            body,
        }
    }

    pub async fn json(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        payload: Value,
    ) -> Response {
        self.send(
            method,
            uri,
            token,
            Body::from(payload.to_string()),
            Some("application/json"),
        )
        .await
    }

    pub async fn get(&self, uri: &str, token: &str) -> Response {
        self.send(Method::GET, uri, Some(token), Body::empty(), None)
            .await
    }

    pub async fn user_token(&self, username: &str) -> String {
        self.state
            .auth
            .create_user(username, PASSWORD)
            .await
            .expect("create user");
        let response = self
            .json(
                Method::POST,
                "/api/v1/auth/login",
                None,
                json!({ "username": username, "password": PASSWORD }),
            )
            .await;
        response.json()["access_token"]
            .as_str()
            .expect("access token")
            .to_owned()
    }

    pub async fn upload(&self, token: &str, query: &str, content: &'static str) -> Response {
        self.upload_as(token, query, content, "application/octet-stream")
            .await
    }

    pub async fn upload_as(
        &self,
        token: &str,
        query: &str,
        content: &'static str,
        content_type: &str,
    ) -> Response {
        self.send(
            Method::POST,
            &format!("/api/v1/files?{query}"),
            Some(token),
            Body::from(content),
            Some(content_type),
        )
        .await
    }
}

pub const SHORT_WAIT: Duration = Duration::from_millis(300);
