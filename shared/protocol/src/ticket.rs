use serde::{Deserialize, Serialize};

use uuid::Uuid;

use crate::NodeId;

/// What a short-lived ticket may be used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum TicketRequest {
    Events,
    Download {
        node_id: NodeId,
    },
    DownloadVersion {
        node_id: NodeId,
        version: i32,
    },
    /// A file of a transfer sent to the user.
    TransferItem {
        transfer_id: Uuid,
        item_id: Uuid,
    },
    /// Every file of a transfer sent to the user, as one zip.
    TransferArchive {
        transfer_id: Uuid,
    },
    /// A file reached through a share made for the user.
    SharedFile {
        share_id: Uuid,
        node_id: NodeId,
    },
    /// A share made for the user, or a folder inside it, as one zip.
    SharedArchive {
        share_id: Uuid,
    },
    /// A file played as it downloads, such as a video: the ticket opens the file's download URL
    /// for every range request the player makes until it expires, not just once.
    Stream {
        node_id: NodeId,
    },
    /// A file reached through a share made for the user, played as it downloads.
    SharedStream {
        share_id: Uuid,
        node_id: NodeId,
    },
}

/// A short-lived ticket passed in the query string where headers are unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketResponse {
    pub ticket: String,
    pub expires_in: i64,
}

/// Query carrying a short-lived ticket, for requests that cannot send headers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketQuery {
    pub ticket: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_scopes_are_tagged() {
        assert_eq!(
            serde_json::to_string(&TicketRequest::Events).expect("json"),
            r#"{"scope":"events"}"#
        );
        let download = TicketRequest::Download {
            node_id: NodeId(Uuid::nil()),
        };
        let json = serde_json::to_string(&download).expect("json");
        assert_eq!(
            serde_json::from_str::<TicketRequest>(&json).expect("back"),
            download
        );
    }

    #[test]
    fn stream_tickets_round_trip() {
        let requests = [
            TicketRequest::Stream {
                node_id: NodeId(Uuid::nil()),
            },
            TicketRequest::SharedStream {
                share_id: Uuid::nil(),
                node_id: NodeId(Uuid::nil()),
            },
        ];
        for request in requests {
            let json = serde_json::to_string(&request).expect("json");
            assert_eq!(
                serde_json::from_str::<TicketRequest>(&json).expect("back"),
                request
            );
        }
        assert_eq!(
            serde_json::to_string(&requests[0]).expect("json"),
            format!(r#"{{"scope":"stream","node_id":"{}"}}"#, Uuid::nil())
        );
    }
}
