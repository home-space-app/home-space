// Each integration test binary uses a different subset of these helpers.
#![allow(dead_code)]

pub mod idp;

use std::{sync::Arc, time::Duration};

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
use db::{Db, TenantDomain, TenantId};
use files::{BlobStore, FileService};
use notifications::Hub;
use search::SearchIndex;

pub const PASSWORD: &str = "password-123";
/// Chunk size of resumable uploads in tests, small so a short text spans several chunks.
pub const UPLOAD_CHUNK: u64 = 4;

/// The domain of tenant 1 in [`setup_two_tenants`].
pub const HOST_A: &str = "files.example.com";
/// The domain of the second tenant in [`setup_two_tenants`].
pub const HOST_B: &str = "cloud.other.org";
/// A domain whose tenant is recorded but not serving.
pub const HOST_UNAVAILABLE: &str = "closed.example.net";
const JWT_SECRET: &[u8] = b"integration-test-secret-integration";

/// The server as one tenant's clients reach it.
#[derive(Clone)]
pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    /// The auth service of the tenant this app's requests reach.
    pub auth: AuthService,
    /// The `Host` every request is sent to, unless it names its own; `None` sends none.
    pub host: Option<&'static str>,
    _storage: Arc<tempfile::TempDir>,
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
    let auth =
        configure(AuthService::new(db.clone(), auth_config(TenantId::DEFAULT)).expect("auth"));
    let tenants = api::Tenants::single(tenant(TenantId::DEFAULT, Vec::new(), &auth));
    build(db, tenants, auth, None).await
}

/// A server in host mode with two tenants: tenant 1 on [`HOST_A`] and another on
/// [`HOST_B`], returned as each one's clients reach it. Both are given the same signing
/// secret, which the server refuses at start-up, so that the tests show tokens are kept
/// apart by more than their signatures. [`HOST_UNAVAILABLE`] leads to a third tenant that
/// is not serving.
pub async fn setup_two_tenants(pool: PgPool) -> (TestApp, TestApp) {
    let other = add_tenant(&pool, "Other").await;
    let closed = add_tenant(&pool, "Closed").await;
    let db = Db::from_pool(pool);
    let a = AuthService::new(db.clone(), auth_config(TenantId::DEFAULT)).expect("auth");
    let b = a.for_tenant(auth_config(other));
    let domains = [
        domain(HOST_A, TenantId::DEFAULT),
        domain(HOST_B, other),
        domain(HOST_UNAVAILABLE, closed),
    ];
    let tenants = api::Tenants::new(
        api::TenantMode::Host,
        vec![
            tenant(TenantId::DEFAULT, vec![HOST_A.to_owned()], &a),
            tenant(other, vec![HOST_B.to_owned()], &b),
        ],
        &domains,
    );
    let app_a = build(db, tenants, a, Some(HOST_A)).await;
    let app_b = TestApp {
        auth: b,
        host: Some(HOST_B),
        ..app_a.clone()
    };
    (app_a, app_b)
}

async fn add_tenant(pool: &PgPool, name: &str) -> TenantId {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("tenant")
}

fn domain(domain: &str, tenant_id: TenantId) -> TenantDomain {
    TenantDomain {
        domain: domain.to_owned(),
        tenant_id,
        is_primary: true,
    }
}

fn auth_config(tenant: TenantId) -> AuthConfig {
    AuthConfig {
        tenant,
        jwt_secret: JWT_SECRET.to_vec(),
        previous_jwt_secrets: Vec::new(),
        access_ttl: time::Duration::minutes(15),
        refresh_ttl: time::Duration::days(30),
    }
}

fn tenant(id: TenantId, domains: Vec<String>, auth: &AuthService) -> api::TenantContext {
    api::TenantContext {
        id,
        name: format!("Tenant {id}"),
        public_url: domains.first().map(|domain| format!("https://{domain}")),
        domains,
        auth: auth.clone(),
    }
}

async fn build(
    db: Db,
    tenants: api::Tenants,
    auth: AuthService,
    host: Option<&'static str>,
) -> TestApp {
    let storage = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(storage.path()).await.expect("store");
    let (search, _worker) = SearchIndex::start(db.clone(), store.clone(), Vec::new());
    let state = AppState {
        tenants,
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
                transfer_ttl_hours: 24,
                max_transfer_bytes: None,
                changes_retention_days: None,
                user_deletion_delay_minutes: 10,
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
        trust_forwarded_for: true,
    };
    TestApp {
        router: api::router(state.clone()),
        state,
        auth,
        host,
        _storage: Arc::new(storage),
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
        let names_host = extra_headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"));
        if let Some(host) = self.host.filter(|_| !names_host) {
            request = request.header("host", host);
        }
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
        self.auth
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

    /// Signs `username` in from `device` (creating the user if needed), returning the access
    /// token.
    pub async fn sign_in_from(&self, username: &str, device: uuid::Uuid, name: &str) -> String {
        self.auth.create_user(username, PASSWORD).await.ok();
        let response = self
            .json(
                Method::POST,
                "/api/v1/auth/login",
                None,
                json!({
                    "username": username,
                    "password": PASSWORD,
                    "device": { "id": device, "name": name, "platform": "desktop-linux" },
                }),
            )
            .await;
        assert_eq!(response.status, StatusCode::OK);
        response.json()["access_token"]
            .as_str()
            .expect("access")
            .to_owned()
    }

    /// The id of an existing user.
    pub async fn user_id(&self, username: &str) -> db::UserId {
        self.state
            .db
            .find_user_by_username(db::TenantId::DEFAULT, username)
            .await
            .expect("lookup")
            .expect("user")
            .id
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
