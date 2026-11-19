//! `homespace serve`: wires the services together and runs the HTTP server.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};

use api::{AppState, ServerInfo};
use auth::{AuthConfig, AuthService};
use db::{Db, TenantId};
use files::{BlobStore, FileService};
use notifications::Hub;
use search::SearchIndex;

use crate::{config::Config, jobs, tenants};

const SECONDS_PER_HOUR: u64 = 3600;

/// The auth service of the default tenant, from the server configuration.
pub fn auth_service(db: &Db, config: &Config) -> Result<AuthService> {
    let (jwt_secret, previous_jwt_secrets) = config.jwt_secrets()?;
    let auth_config = AuthConfig {
        tenant: TenantId::DEFAULT,
        jwt_secret,
        previous_jwt_secrets,
        access_ttl: time::Duration::seconds(config.access_ttl_secs),
        refresh_ttl: time::Duration::seconds(config.refresh_ttl_secs),
    };
    AuthService::new(db.clone(), auth_config).context("creating auth service")
}

pub async fn serve(db: Db, config: &Config) -> Result<()> {
    let store = BlobStore::open(&config.storage_root)
        .await
        .context("opening storage root")?;
    let (search, _worker) = SearchIndex::start(db.clone(), store.clone(), Vec::new());
    let tenants = tenants::load(
        &db,
        config,
        auth_service(&db, config)?,
        &tenants::EnvSecrets,
    )
    .await?;
    let state = AppState {
        tenants,
        files: file_service(db.clone(), store, config),
        hub: Hub::new(),
        search,
        info: Arc::new(ServerInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: time::OffsetDateTime::now_utc(),
            config: config.summary()?,
        }),
        db,
        trust_forwarded_for: config.trust_forwarded_for,
    };
    jobs::spawn(&state, config);

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    tracing::info!(address = %config.bind, "listening");
    let router = match &config.web_root {
        Some(web_root) => api::router_with_web(state, web_root),
        None => api::router(state),
    };
    // With the peer's address, so that failed sign-ins are counted for where they come from.
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("serving")
}

fn file_service(db: Db, store: BlobStore, config: &Config) -> FileService {
    let transfer_ttl =
        Duration::from_secs(config.transfer_ttl_hours.saturating_mul(SECONDS_PER_HOUR));
    FileService::new(db, store, config.max_upload_bytes)
        .with_chunk_bytes(config.upload_chunk_bytes)
        .with_transfers(transfer_ttl, config.max_transfer_bytes)
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
