//! Typed HTTP client for the home-space API, shared by all Rust clients.

mod accounts;
mod browser_login;
mod capabilities;
mod client;
mod devices;
mod duplicates;
mod error;
mod password;
mod pkce;
mod public_shares;
mod releases;
mod second_factor;
mod server_address;
mod shared;
mod shares;
mod storage;
mod transfers;
mod uploads;

pub use capabilities::{FALLBACK_UPLOAD_CHUNK_BYTES, STREAMING_NEEDS_UPDATE};
pub use client::{Client, TokenListener, user_agent};
pub use error::ApiError;
pub use password::Confirmation;
pub use pkce::{Pkce, VERIFIER_BYTES};
pub use releases::{Release, latest_release};
pub use second_factor::{SecondFactor, SecondFactorStep, SignIn};
pub use server_address::is_unprotected;
