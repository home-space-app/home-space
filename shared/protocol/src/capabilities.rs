use serde::{Deserialize, Serialize};

/// What the server supports, from `GET /capabilities`; clients read it after signing in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// The largest request body a file upload may send. Files up to this size go up in one
    /// request; larger ones through `/uploads` in chunks of at most this size.
    pub upload_chunk_bytes: u64,
    /// Whether the server issues stream tickets (`TicketRequest::Stream`), which video players
    /// need. Servers from before them do not report it.
    #[serde(default)]
    pub stream_tickets: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_does_not_report_stream_tickets_has_none() {
        let capabilities: Capabilities =
            serde_json::from_str(r#"{"upload_chunk_bytes":1024}"#).expect("json");
        assert!(!capabilities.stream_tickets);
    }
}
