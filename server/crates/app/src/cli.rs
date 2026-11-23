//! The command line: the server takes no subcommands, only its configuration from flags or
//! environment. Administration is done with `homespace-ctl`.

use clap::Parser;

use crate::config::Config;

#[derive(Parser)]
#[command(name = "homespace", about = "Personal file storage server", version)]
pub struct Cli {
    #[command(flatten)]
    pub config: Config,
}
