//! `homespace serve`: wires the services together and runs the HTTP server.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};

use api::{AppState, EventDispatcher, ServerInfo, SharedServices};
use auth::{AuthConfig, AuthService};
use db::{Db, TenantId};
use events::Events;
use files::{BlobStore, FileServices, FilesConfig};
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
    let hub = Hub::new();
    let events = Events::new(EventDispatcher::new(hub.clone(), search.clone()));
    let files = FileServices::new(db.clone(), store, files_config(config), events.clone());
    let shared = SharedServices {
        db: db.clone(),
        nodes: files.nodes.clone(),
        uploads: files.uploads.clone(),
        hub: hub.clone(),
        events,
        deletion_delay: deletion_delay(config),
    };
    let tenants = tenants::load(
        &db,
        config,
        auth_service(&db, config)?,
        &tenants::EnvSecrets,
        &shared,
    )
    .await?;
    let state = AppState {
        tenants,
        files,
        hub,
        search,
        info: Arc::new(ServerInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: time::OffsetDateTime::now_utc(),
            config: config.summary()?,
        }),
        trust_forwarded_for: config.trust_forwarded_for,
    };
    jobs::spawn(&db, &state, config);

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

/// How long after an administrator deletes an account it is deleted.
fn deletion_delay(config: &Config) -> time::Duration {
    let minutes = i64::try_from(config.user_deletion_delay_minutes).unwrap_or(i64::MAX);
    time::Duration::minutes(minutes)
}

fn files_config(config: &Config) -> FilesConfig {
    FilesConfig {
        max_upload_bytes: config.max_upload_bytes,
        chunk_bytes: config.upload_chunk_bytes,
        transfer_ttl: Duration::from_secs(
            config.transfer_ttl_hours.saturating_mul(SECONDS_PER_HOUR),
        ),
        max_transfer_bytes: config.max_transfer_bytes,
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
