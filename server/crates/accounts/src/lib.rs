//! Users, their devices and their vault: what a user sees of their own account, and what administrators
//! manage. Each service works within one tenant.

mod accounts;
mod devices;
mod error;
mod vault;

pub use accounts::{AccountService, StorageReport};
pub use devices::{DeviceService, SERVER_VERSION};
pub use error::AccountError;
pub use vault::{Shown, VaultService};
