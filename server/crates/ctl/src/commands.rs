//! The subcommands that work on the database and the storage root.

mod search;
mod user;

pub use search::run as run_search;
pub use user::run as run_user;
