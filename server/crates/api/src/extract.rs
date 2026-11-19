//! Request extractors: which tenant a request was sent to, who sent it, from where, and the
//! body, query and path, each refused as a problem when it cannot be read or is not valid.

mod admin_user;
mod auth_user;
mod client_ip;
mod json;
mod path;
mod query;
mod tenant;

pub use admin_user::AdminUser;
pub use auth_user::AuthUser;
pub use client_ip::ClientIp;
pub use json::ApiJson;
pub use path::ApiPath;
pub use protocol::TicketQuery;
pub use query::ApiQuery;
pub use tenant::Tenant;

use axum::http::HeaderMap;

/// The last value of header `name`, when it is text. A proxy appends its own header after any
/// the client sent, so the last one is the one to believe.
fn last_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get_all(name).iter().next_back()?.to_str().ok()
}

/// The entry appended last to a comma-separated list such as `X-Forwarded-For`.
fn last_entry(list: &str) -> &str {
    list.rsplit(',').next().unwrap_or(list).trim()
}
