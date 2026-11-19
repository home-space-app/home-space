//! Read-only shares: the owner's links and shares with named users, what visitors of a link
//! see, and what users read and copy of the shares made for them.

mod error;
mod owner;
mod received;
mod service;
mod types;
mod visitor;

pub use error::ShareError;
pub use service::ShareService;
pub use types::{LinkView, ReceivedDetails, ShareDetails};
