//! Typed HTTP client for the home-space API, shared by all Rust clients.

mod capabilities;
mod client;
mod devices;
mod error;
mod password;
mod pkce;
mod storage;
mod uploads;

pub use capabilities::FALLBACK_UPLOAD_CHUNK_BYTES;
pub use client::{Client, TokenListener};
pub use error::ApiError;
pub use pkce::{Pkce, VERIFIER_BYTES};
