//! Users and their devices: what a user sees of their own account, and what administrators
//! manage. Each service works within one tenant.

mod accounts;
mod devices;
mod error;

pub use accounts::{AccountService, StorageReport};
pub use devices::DeviceService;
pub use error::AccountError;
