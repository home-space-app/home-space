mod cli;
mod commands;
mod tenant;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use db::Db;

use cli::{Cli, Command};

const DEFAULT_LOG_FILTER: &str = "warn";

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_LOG_FILTER.into()),
        )
        .init();

    let cli = Cli::parse();
    let database_url = cli
        .database_url
        .context("DATABASE_URL is not set (or pass --database-url)")?;
    let db = Db::connect(&database_url)
        .await
        .context("connecting to database")?;
    match cli.command {
        Command::User(command) => commands::run_user(command, &db).await,
        Command::Search(command) => commands::run_search(command, db).await,
    }
}
