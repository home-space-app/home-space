//! Tickets: short-lived, single-use tokens for one request, for the URLs of downloads and the
//! events websocket, which cannot carry an `Authorization` header.

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
const SCOPE_EVENTS: &str = "events";
const SCOPE_DOWNLOAD_PREFIX: &str = "download:";

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
        let ttl = Duration::seconds(TICKET_TTL_SECS);
        Ok(TicketResponse {
            ticket: encode_ticket(&self.keys, user_id, session_id, &scope_of(request), ttl)?,
            expires_in: TICKET_TTL_SECS,
        })
    }

    /// Whom a ticket for `request`'s scope was issued to. A ticket works once: it reaches the
    /// server in a URL, so a copy of it, from a proxy's log or the browser's history, must be
    /// worth nothing after the request it was made for.
    pub(crate) fn redeem(
        &self,
        ticket: &str,
        request: &TicketRequest,
    ) -> Result<Ticketed, AuthError> {
        let grant = decode_ticket(&self.keys, ticket, &scope_of(request))?;
        if self.use_once(grant.id, grant.expires_at) {
            Ok(Ticketed {
                user_id: grant.user_id,
                session_id: grant.session_id,
            })
        } else {
            Err(AuthError::InvalidCredentials)
        }
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
    }
}

fn download_scope(node_id: NodeId) -> String {
    format!("{SCOPE_DOWNLOAD_PREFIX}{node_id}")
}
