use serde::{Deserialize, Serialize};

/// What the server supports, from `GET /capabilities`; clients read it after signing in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// The largest request body a file upload may send. Files up to this size go up in one
    /// request; larger ones through `/uploads` in chunks of at most this size.
    pub upload_chunk_bytes: u64,
    /// Whether the server issues stream tickets (`TicketRequest::Stream`), which video players
    /// need. Servers from before them do not report it.
    #[serde(default)]
    pub stream_tickets: bool,
    /// The server's version. Servers from before 0.13.0 do not report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The [`crate::features`] the server provides; empty for servers from before 0.13.0.
    #[serde(default)]
    pub features: Vec<String>,
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

    #[test]
    fn a_server_from_before_versions_reports_none_and_no_features() {
        let capabilities: Capabilities =
            serde_json::from_str(r#"{"upload_chunk_bytes":1024,"stream_tickets":true}"#)
                .expect("json");
        assert_eq!(capabilities.version, None);
        assert!(capabilities.features.is_empty());
    }
}
