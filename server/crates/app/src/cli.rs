//! The command line: global configuration and the subcommands.

use clap::{Parser, Subcommand};

use crate::config::Config;

#[derive(Parser)]
#[command(name = "homespace", about = "Personal file storage server")]
pub struct Cli {
    #[command(flatten)]
    pub config: Config,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
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
pub enum SearchCommand {
    /// Index everything that is missing or outdated; `--all` drops the index first.
    Reindex {
        #[arg(long)]
        all: bool,
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
    Passwd {
        username: String,
        /// Read the password from the first line of standard input instead of prompting.
        #[arg(long)]
        password_stdin: bool,
    },
}
