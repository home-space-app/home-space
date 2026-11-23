//! The command line of `homespace-ctl`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "homespace-ctl", about = "Manage a Home Space server", version)]
pub struct Cli {
    /// The database to work on. Unset, the commands run inside the app container of the
    /// server `homespace-ctl install` set up here.
    #[arg(long, env = "DATABASE_URL", hide_env_values = true, global = true)]
    pub database_url: Option<String>,

    /// Password of the database user, when `DATABASE_URL` carries none.
    #[arg(
        long,
        env = "HOMESPACE_DB_PASSWORD",
        hide_env_values = true,
        global = true
    )]
    pub db_password: Option<String>,

    /// File holding the database password (e.g. a Docker/Podman secret).
    #[arg(long, env = "HOMESPACE_DB_PASSWORD_FILE", global = true)]
    pub db_password_file: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Set up a Home Space server on this machine with Docker or Podman, asking how.
    Install(InstallArgs),
    /// Manage users.
    #[command(subcommand)]
    User(UserCommand),
    /// Manage the search index.
    #[command(subcommand)]
    Search(SearchCommand),
}

#[derive(clap::Args)]
pub struct InstallArgs {
    /// Ask everything, then show the files and commands instead of changing anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Subcommand)]
pub enum SearchCommand {
    /// Index everything that is missing or outdated; `--all` drops the index first.
    Reindex {
        #[arg(long)]
        all: bool,
        #[arg(long, env = "HOMESPACE_STORAGE_ROOT", default_value = "./data")]
        storage_root: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum UserCommand {
    /// Create a user; the password is prompted for.
    Add {
        username: String,
        /// Make the user an administrator.
        #[arg(long)]
        admin: bool,
        /// Read the password from the first line of standard input instead of prompting, for
        /// scripts and tests.
        #[arg(long)]
        password_stdin: bool,
        #[command(flatten)]
        tenant: TenantArg,
    },
    /// List users.
    List {
        #[command(flatten)]
        tenant: TenantArg,
    },
    /// Make a user an administrator, or with `--revoke` a regular user again.
    Admin {
        username: String,
        #[arg(long)]
        revoke: bool,
        #[command(flatten)]
        tenant: TenantArg,
    },
    /// Change a user's password (revokes their sessions); the password is prompted for.
    Passwd {
        username: String,
        /// Read the password from the first line of standard input instead of prompting.
        #[arg(long)]
        password_stdin: bool,
        #[command(flatten)]
        tenant: TenantArg,
    },
}

#[derive(clap::Args)]
pub struct TenantArg {
    /// The tenant, by id or by one of its domains. Optional while there is only one tenant.
    #[arg(long)]
    pub tenant: Option<String>,
}
