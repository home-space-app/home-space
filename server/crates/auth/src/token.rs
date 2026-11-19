use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use db::{TenantId, UserId};

use crate::AuthError;

const REFRESH_TOKEN_BYTES: usize = 32;
const KEY_ID_BYTES: usize = 6;
/// The `typ` claim of an access token and of a ticket. Both are signed with the same key, so
/// each names what it is and is refused when presented as the other.
const TYPE_ACCESS: &str = "access";
const TYPE_TICKET: &str = "ticket";
/// The `typ` claim of the access token of an unlocked share link.
const TYPE_SHARE: &str = "share";

mod access;
mod keys;
mod share;
mod ticket;

pub use access::{Claims, decode_access, encode_access};
pub use keys::Keys;
pub use share::{ShareGrant, decode_share_access, encode_share_access};
pub use ticket::{decode_ticket, encode_ticket};
#[cfg(test)]
use {access::AccessClaims, ticket::TicketClaims};

/// Generates a random opaque refresh token.
pub fn new_refresh_token() -> String {
    let mut bytes = [0u8; REFRESH_TOKEN_BYTES];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generates the random secret of a share link: 32 bytes, base64url.
pub fn new_share_token() -> String {
    new_refresh_token()
}

/// Hash under which a refresh token is stored.
pub fn hash_refresh_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests;
