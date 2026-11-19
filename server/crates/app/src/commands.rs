//! The administration subcommands of the binary.

mod search;
mod user;

pub use search::run as run_search;
pub use user::run as run_user;
