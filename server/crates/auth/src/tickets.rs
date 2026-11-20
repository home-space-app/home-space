//! Tickets: short-lived, single-use tokens for one request, for the URLs of downloads and the
//! events websocket, which cannot carry an `Authorization` header.
//!
//! A stream ticket ([`TicketRequest::Stream`], [`TicketRequest::SharedStream`]) is the one
//! exception: a video player fetches its file in many range requests and cannot ask for a
//! ticket before each, so a stream ticket opens that file's download URL until it expires. It
//! is bound to one file and one session, which still has to be signed in, and it lasts
//! [`STREAM_TTL_SECS`]; a player whose ticket expired asks for a new one and resumes.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use db::UserId;
use protocol::{NodeId, TicketRequest, TicketResponse};

use crate::{
    AuthError,
    token::{Keys, decode_ticket, encode_ticket},
};

const TICKET_TTL_SECS: i64 = 30;
const STREAM_TTL_SECS: i64 = 60 * 60;
const SCOPE_EVENTS: &str = "events";
const SCOPE_DOWNLOAD_PREFIX: &str = "download:";
const SCOPE_STREAM_PREFIX: &str = "stream:";
const SCOPE_SHARED_STREAM_PREFIX: &str = "shared-stream:";

/// Who a ticket was issued to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ticketed {
    pub user_id: UserId,
    pub session_id: Uuid,
}

/// Issues and redeems the tickets of one tenant.
#[derive(Clone, Debug)]
pub(crate) struct Tickets {
    keys: Keys,
    /// Tickets already used, by id, with the time they expire and can be forgotten. Kept in
    /// memory: after a restart a used ticket would be accepted again, for the rest of its
    /// [`TICKET_TTL_SECS`] at most.
    pub(crate) used: Arc<Mutex<HashMap<Uuid, i64>>>,
}

impl Tickets {
    pub(crate) fn new(keys: Keys) -> Self {
        Self {
            keys,
            used: Arc::default(),
        }
    }

    /// The tickets of another tenant, sharing the list of used ones: ticket ids are unique.
    pub(crate) fn for_tenant(&self, keys: Keys) -> Self {
        Self {
            keys,
            used: Arc::clone(&self.used),
        }
    }

    /// A ticket for `request`'s scope only.
    pub(crate) fn issue(
        &self,
        user_id: UserId,
        session_id: Uuid,
        request: &TicketRequest,
    ) -> Result<TicketResponse, AuthError> {
        let lifetime = if is_stream(request) {
            STREAM_TTL_SECS
        } else {
            TICKET_TTL_SECS
        };
        let ttl = Duration::seconds(lifetime);
        Ok(TicketResponse {
            ticket: encode_ticket(&self.keys, user_id, session_id, &scope_of(request), ttl)?,
            expires_in: lifetime,
        })
    }

    /// Whom a ticket for `request`'s scope was issued to. A ticket works once: it reaches the
    /// server in a URL, so a copy of it, from a proxy's log or the browser's history, must be
    /// worth nothing after the request it was made for. A download of a file also takes a
    /// stream ticket for that file, which works until it expires (see the module docs).
    pub(crate) fn redeem(
        &self,
        ticket: &str,
        request: &TicketRequest,
    ) -> Result<Ticketed, AuthError> {
        let grant = match decode_ticket(&self.keys, ticket, &scope_of(request)) {
            Ok(grant) => {
                if !self.use_once(grant.id, grant.expires_at) {
                    return Err(AuthError::InvalidCredentials);
                }
                grant
            }
            Err(err) => match stream_of(request) {
                Some(stream) => decode_ticket(&self.keys, ticket, &scope_of(&stream))?,
                None => return Err(err),
            },
        };
        Ok(Ticketed {
            user_id: grant.user_id,
            session_id: grant.session_id,
        })
    }

    /// Marks a ticket used; `false` when it already was. Only tickets that passed validation
    /// get here, and each is forgotten once it has expired, so the list stays as short as the
    /// tickets used within one lifetime.
    pub(crate) fn use_once(&self, id: Uuid, expires_at: i64) -> bool {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        used.retain(|_, expires_at| *expires_at > now);
        used.insert(id, expires_at).is_none()
    }
}

fn scope_of(request: &TicketRequest) -> String {
    match request {
        TicketRequest::Events => SCOPE_EVENTS.to_owned(),
        TicketRequest::Download { node_id } => download_scope(*node_id),
        TicketRequest::DownloadVersion { node_id, version } => {
            format!("{}:v{version}", download_scope(*node_id))
        }
        TicketRequest::TransferItem {
            transfer_id,
            item_id,
        } => format!("transfer:{transfer_id}:{item_id}"),
        TicketRequest::TransferArchive { transfer_id } => format!("transfer:{transfer_id}"),
        TicketRequest::SharedFile { share_id, node_id } => format!("shared:{share_id}:{node_id}"),
        TicketRequest::SharedArchive { share_id } => format!("shared:{share_id}"),
        TicketRequest::Stream { node_id } => format!("{SCOPE_STREAM_PREFIX}{node_id}"),
        TicketRequest::SharedStream { share_id, node_id } => {
            format!("{SCOPE_SHARED_STREAM_PREFIX}{share_id}:{node_id}")
        }
    }
}

fn is_stream(request: &TicketRequest) -> bool {
    matches!(
        request,
        TicketRequest::Stream { .. } | TicketRequest::SharedStream { .. }
    )
}

/// The stream ticket that also opens the download `request` asks for, if one does.
fn stream_of(request: &TicketRequest) -> Option<TicketRequest> {
    match *request {
        TicketRequest::Download { node_id } => Some(TicketRequest::Stream { node_id }),
        TicketRequest::SharedFile { share_id, node_id } => {
            Some(TicketRequest::SharedStream { share_id, node_id })
        }
        _ => None,
    }
}

fn download_scope(node_id: NodeId) -> String {
    format!("{SCOPE_DOWNLOAD_PREFIX}{node_id}")
}
