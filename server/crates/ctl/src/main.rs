mod cli;
mod commands;
mod generate;
#[cfg(target_os = "linux")]
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
    let mut cli = Cli::parse();
    if let Some(path) = &cli.env_file {
        dotenvy::from_path(path).with_context(|| format!("reading {}", path.display()))?;
        cli = Cli::parse();
    }
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
        env_file,
    } = cli;
    let database = Database {
        env_file,
        url: database_url,
        password: db_password,
        password_file: db_password_file,
    };
    match command {
        Command::Install(args) => run_install(&args),
        Command::User(command) => commands::run_user(command, &database.connect().await?).await,
        Command::Search(command) => commands::run_search(command, database.connect().await?).await,
        Command::Secrets(command) => commands::run_secrets(command, database).await,
    }
}

#[cfg(target_os = "linux")]
fn run_install(args: &cli::InstallArgs) -> Result<()> {
    install::run(args)
}

#[cfg(not(target_os = "linux"))]
fn run_install(_: &cli::InstallArgs) -> Result<()> {
    anyhow::bail!("`homespace-ctl install` sets up a server on Linux only")
}

/// Where the database commands work: the database given, or the installation on this machine.
pub struct Database {
    /// The `--env-file` read, if any.
    env_file: Option<PathBuf>,
    url: Option<String>,
    password: Option<String>,
    password_file: Option<PathBuf>,
}

impl Database {
    /// Whether a database was given, or an `--env-file` that should name one.
    fn is_given(&self) -> bool {
        self.env_file.is_some() || self.url.as_deref().is_some_and(|url| !url.is_empty())
    }

    /// Connects to the database given. Without one the command is run inside the installation's
    /// app container instead, and this process exits with its status.
    async fn connect(self) -> Result<Db> {
        let Some(url) = self.url.filter(|url| !url.is_empty()) else {
            if let Some(path) = self.env_file {
                anyhow::bail!("{} sets no DATABASE_URL", path.display());
            }
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
    let path = installation::config_path()?;
    let installation = Installation::load(&path)?.with_context(|| {
        format!(
            "DATABASE_URL is not set (or pass --database-url or --env-file), and this user has \
             no installation from `homespace-ctl install` ({} is missing; run as the user who \
             installed the server)",
            path.display()
        )
    })?;
    let status = installation.forward_command_line()?;
    std::process::exit(status.code().unwrap_or(1));
}
