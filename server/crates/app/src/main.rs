mod config;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use api::{AppState, ServerInfo};
use auth::{AuthConfig, AuthService, OidcService};
use db::Db;
use files::{BlobStore, FileService};
use notifications::Hub;
use search::SearchIndex;

use config::Config;

#[derive(Parser)]
#[command(name = "homespace", about = "Personal file storage server")]
struct Cli {
    #[command(flatten)]
    config: Config,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP server.
    Serve,
    /// Apply database migrations.
    Migrate,
    /// Manage users.
    #[command(subcommand)]
    User(UserCommand),
    /// Manage the search index.
    #[command(subcommand)]
    Search(SearchCommand),
}

#[derive(Subcommand)]
enum SearchCommand {
    /// Index everything that is missing or outdated; `--all` drops the index first.
    Reindex {
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum UserCommand {
    /// Create a user; the password is prompted for.
    Add {
        username: String,
        /// Make the user an administrator.
        #[arg(long)]
        admin: bool,
    },
    /// List users.
    List,
    /// Make a user an administrator, or with `--revoke` a regular user again.
    Admin {
        username: String,
        #[arg(long)]
        revoke: bool,
    },
    /// Change a user's password (revokes their sessions); the password is prompted for.
    Passwd { username: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();
    cli.config.validate()?;
    let db = Db::connect(&cli.config.database_url)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;

    match cli.command {
        Command::Migrate => {
            tracing::info!("migrations applied");
            Ok(())
        }
        Command::User(command) => run_user_command(command, &db, &cli.config).await,
        Command::Search(command) => run_search_command(command, db, &cli.config).await,
        Command::Serve => serve(db, &cli.config).await,
    }
}

fn auth_service(db: &Db, config: &Config) -> Result<AuthService> {
    let (jwt_secret, previous_jwt_secrets) = config.jwt_secrets()?;
    let auth_config = AuthConfig {
        jwt_secret,
        previous_jwt_secrets,
        access_ttl: time::Duration::seconds(config.access_ttl_secs),
        refresh_ttl: time::Duration::seconds(config.refresh_ttl_secs),
    };
    AuthService::new(db.clone(), auth_config).context("creating auth service")
}

async fn serve(db: Db, config: &Config) -> Result<()> {
    let store = BlobStore::open(&config.storage_root)
        .await
        .context("opening storage root")?;
    let (search, _worker) = SearchIndex::start(db.clone(), store.clone(), Vec::new());
    let mut auth = auth_service(&db, config)?;
    if let Some(oidc_config) = config.oidc()? {
        let oidc = OidcService::discover(oidc_config)
            .await
            .context("discovering the OpenID Connect provider")?;
        tracing::info!(issuer = oidc.issuer(), "single sign-on enabled");
        auth = auth.with_oidc(oidc, config.password_login);
    }
    let state = AppState {
        auth,
        files: FileService::new(db.clone(), store, config.max_upload_bytes)
            .with_chunk_bytes(config.upload_chunk_bytes)
            .with_transfers(
                std::time::Duration::from_secs(config.transfer_ttl_hours.saturating_mul(3600)),
                config.max_transfer_bytes,
            ),
        hub: Hub::new(),
        search: search.clone(),
        info: std::sync::Arc::new(ServerInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: time::OffsetDateTime::now_utc(),
            config: config.summary()?,
        }),
        db,
        trust_forwarded_for: config.trust_forwarded_for,
    };
    tokio::spawn(async move {
        match search.catch_up().await {
            Ok(report) => tracing::info!(
                names_indexed = report.names_indexed,
                content_queued = report.content_queued,
                "search index is up to date"
            ),
            Err(err) => tracing::error!(error = %err, "search index catch-up failed"),
        }
    });
    tokio::spawn(prune(
        state.db.clone(),
        config.changes_retention_days.map(time::Duration::days),
        state.auth.refresh_ttl(),
    ));
    tokio::spawn(expire_uploads(
        state.files.clone(),
        std::time::Duration::from_secs(config.upload_expiry_hours.saturating_mul(3600)),
    ));
    tokio::spawn(expire_transfers(state.files.clone(), state.hub.clone()));
    tokio::spawn(delete_users(state.files.clone()));
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
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("serving")
}

/// Compacts the change journal, drops its entries older than `retention` when there is one,
/// removes expired sign-in state and old failed sign-ins, and forgets devices signed out for longer than
/// `device_retention`; once at start and then daily.
async fn prune(db: Db, retention: Option<time::Duration>, device_retention: time::Duration) {
    let mut daily = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
    loop {
        daily.tick().await;
        match db.compact_changes().await {
            Ok(compacted) => tracing::info!(compacted, "change journal compacted"),
            Err(err) => tracing::error!(error = %err, "compacting the change journal failed"),
        }
        if let Some(retention) = retention {
            match db
                .prune_changes(time::OffsetDateTime::now_utc() - retention)
                .await
            {
                Ok(pruned) => tracing::info!(pruned, "change journal pruned"),
                Err(err) => tracing::error!(error = %err, "pruning the change journal failed"),
            }
        }
        if let Err(err) = db.prune_oidc().await {
            tracing::error!(error = %err, "pruning single sign-on state failed");
        }
        if let Err(err) = db.prune_sessions().await {
            tracing::error!(error = %err, "pruning expired sessions failed");
        }
        if let Err(err) = db
            .prune_login_attempts(time::OffsetDateTime::now_utc() - auth::LOGIN_ATTEMPTS_MEMORY)
            .await
        {
            tracing::error!(error = %err, "pruning failed sign-in attempts failed");
        }
        match db
            .prune_devices(time::OffsetDateTime::now_utc() - device_retention)
            .await
        {
            Ok(0) => {}
            Ok(pruned) => tracing::info!(pruned, "forgot signed-out devices"),
            Err(err) => tracing::error!(error = %err, "pruning signed-out devices failed"),
        }
    }
}

/// Ends resumable uploads idle for longer than `expiry`, once at start and then hourly.
async fn expire_uploads(files: FileService, expiry: std::time::Duration) {
    let mut hourly = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        hourly.tick().await;
        match files.expire_uploads(expiry).await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, "expired unfinished uploads"),
            Err(err) => tracing::error!(error = %err, "expiring unfinished uploads failed"),
        }
    }
}

/// Removes transfers to devices whose time ran out and tells both sides, once at start and
/// then hourly.
async fn expire_transfers(files: FileService, hub: Hub) {
    let mut hourly = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        hourly.tick().await;
        match files.expire_transfers().await {
            Ok(expired) => {
                if !expired.is_empty() {
                    tracing::info!(removed = expired.len(), "expired transfers");
                }
                for transfer in expired {
                    api::publish_transfer_expired(&hub, &transfer);
                }
            }
            Err(err) => tracing::error!(error = %err, "expiring transfers failed"),
        }
    }
}

/// Deletes users whose deletion an administrator scheduled and did not take back, with all
/// their files; once at start and then every minute, so a deletion happens close to the time
/// the administration shows for it.
async fn delete_users(files: FileService) {
    let mut every_minute = tokio::time::interval(std::time::Duration::from_secs(60));
    every_minute.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every_minute.tick().await;
        match files.delete_due_users().await {
            Ok(deleted) => {
                for user in deleted {
                    tracing::info!(%user, "deleted user and their files");
                }
            }
            Err(err) => tracing::error!(error = %err, "deleting users failed"),
        }
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

async fn run_search_command(command: SearchCommand, db: Db, config: &Config) -> Result<()> {
    match command {
        SearchCommand::Reindex { all } => {
            let store = BlobStore::open(&config.storage_root)
                .await
                .context("opening storage root")?;
            let (search, worker) = SearchIndex::start(db, store, Vec::new());
            let report = if all {
                search.rebuild().await
            } else {
                search.catch_up().await
            }
            .context("reindexing")?;
            drop(search);
            worker.await.context("waiting for content indexing")?;
            println!(
                "indexed {} names, extracted content of {} files",
                report.names_indexed, report.content_queued
            );
        }
    }
    Ok(())
}

async fn run_user_command(command: UserCommand, db: &Db, config: &Config) -> Result<()> {
    match command {
        UserCommand::Add { username, admin } => {
            let password = prompt_new_password()?;
            let user = auth_service(db, config)?
                .create_user(&username, &password)
                .await
                .context("creating user")?;
            if admin {
                db.set_admin(user.id, true)
                    .await
                    .context("making the user an administrator")?;
            }
            let role = if admin { " as an administrator" } else { "" };
            println!("created user {} ({}){role}", user.username, user.id);
        }
        UserCommand::List => {
            for user in db.list_users().await.context("listing users")? {
                let role = if user.is_admin { "\tadmin" } else { "" };
                let state = match (user.disabled_at, user.delete_at) {
                    (_, Some(at)) => format!("\tdeleted at {at}"),
                    (Some(_), None) => "\tdisabled".to_owned(),
                    (None, None) => String::new(),
                };
                println!("{}\t{}{role}{state}", user.id, user.username);
            }
        }
        UserCommand::Admin { username, revoke } => {
            let user = db
                .find_user_by_username(&username)
                .await
                .context("looking up user")?
                .with_context(|| format!("no such user: {username}"))?;
            db.set_admin(user.id, !revoke)
                .await
                .context("changing administrator rights")?;
            if revoke {
                println!("{username} is no longer an administrator");
            } else {
                println!("{username} is now an administrator");
            }
        }
        UserCommand::Passwd { username } => {
            let user = db
                .find_user_by_username(&username)
                .await
                .context("looking up user")?
                .with_context(|| format!("no such user: {username}"))?;
            let password = prompt_new_password()?;
            auth_service(db, config)?
                .set_password(user.id, &password)
                .await
                .context("changing password")?;
            println!("password updated for {username}");
        }
    }
    Ok(())
}

fn prompt_new_password() -> Result<String> {
    let password = rpassword::prompt_password("Password: ").context("reading password")?;
    let confirmation =
        rpassword::prompt_password("Confirm password: ").context("reading password")?;
    if password == confirmation {
        Ok(password)
    } else {
        Err(anyhow::anyhow!("passwords do not match"))
    }
}
