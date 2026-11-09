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
}

/// A short-lived ticket passed in the query string where headers are unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketResponse {
    pub ticket: String,
    pub expires_in: i64,
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
}
