mod cli;
mod config;
mod jobs;
mod server;
mod tenants;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use db::Db;

use cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();
    cli.config.validate()?;
    let db_password = cli.config.db_password()?;
    let db = Db::connect_with_password(&cli.config.database_url, db_password.as_deref())
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;

    server::serve(db, &cli.config).await
}
