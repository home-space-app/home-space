mod cli;
mod commands;
mod install;
mod installation;
mod tenant;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use db::Db;

use cli::{Cli, Command};
use installation::Installation;

const DEFAULT_LOG_FILTER: &str = "warn";

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_LOG_FILTER.into()),
        )
        .init();

    let Cli {
        database_url,
        db_password,
        db_password_file,
        command,
    } = Cli::parse();
    let database = Database {
        url: database_url,
        password: db_password,
        password_file: db_password_file,
    };
    match command {
        Command::Install(args) => install::run(&args),
        Command::User(command) => commands::run_user(command, &database.connect().await?).await,
        Command::Search(command) => commands::run_search(command, database.connect().await?).await,
    }
}

/// Where the database commands work: the database given, or the installation on this machine.
struct Database {
    url: Option<String>,
    password: Option<String>,
    password_file: Option<PathBuf>,
}

impl Database {
    /// Connects to the database given. Without one the command is run inside the installation's
    /// app container instead, and this process exits with its status.
    async fn connect(self) -> Result<Db> {
        let Some(url) = self.url.filter(|url| !url.is_empty()) else {
            return forward_to_installation();
        };
        let password = auth::secrets::value_or_file(
            "HOMESPACE_DB_PASSWORD",
            self.password.as_deref(),
            "HOMESPACE_DB_PASSWORD_FILE",
            self.password_file.as_deref(),
        )?;
        Db::connect_with_password(&url, password.as_deref())
            .await
            .context("connecting to database")
    }
}

/// Without a database URL, runs this command inside the app container of the installation on
/// this machine, which reaches the database and the files, and exits with its status. It
/// returns only an error.
fn forward_to_installation<T>() -> Result<T> {
    let path = installation::config_path();
    let installation = Installation::load(&path)?.with_context(|| {
        format!(
            "DATABASE_URL is not set (or pass --database-url), and there is no installation \
             from `homespace-ctl install` here ({} is missing)",
            path.display()
        )
    })?;
    let status = installation.forward_command_line()?;
    std::process::exit(status.code().unwrap_or(1));
}
