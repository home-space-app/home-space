//! What the server supports (`/capabilities`), read once a session starts.

use protocol::{
    Capabilities, Compatibility, ErrorCode, Me, TicketRequest, TicketResponse, compat, paths,
};

use crate::{ApiError, Client};

/// The upload request size used when the server's capabilities could not be read. It stays
/// below nginx's default body limit of 1 MiB, so uploads still get through a proxy nobody
/// configured for them.
pub const FALLBACK_UPLOAD_CHUNK_BYTES: u64 = 512 * 1024;

/// What a video player says on a server from before stream tickets.
pub const STREAMING_NEEDS_UPDATE: &str = "Playing videos needs a newer version of the server. \
    Ask its administrator to update it, or download the video to watch it.";

impl Client {
    /// Reads what the server supports.
    pub async fn server_capabilities(&self) -> Result<Capabilities, ApiError> {
        self.send_json(|| self.http().get(self.url(paths::CAPABILITIES)))
            .await
    }

    /// The largest request body a file upload sends: files up to this size go up in one
    /// request, larger ones in chunks of at most this size. It is the server's
    /// [`Capabilities::upload_chunk_bytes`], or [`FALLBACK_UPLOAD_CHUNK_BYTES`] when the server
    /// did not report one.
    pub fn upload_chunk_bytes(&self) -> u64 {
        self.capabilities()
            .map_or(FALLBACK_UPLOAD_CHUNK_BYTES, |capabilities| {
                capabilities.upload_chunk_bytes
            })
            .max(1)
    }

    /// A stream ticket for `request` (`TicketRequest::Stream` or `SharedStream`). A server
    /// from before stream tickets fails it with [`ApiError::Unsupported`]: known from its
    /// capabilities when they were read, or else from it refusing the request it cannot parse.
    pub async fn stream_ticket(&self, request: &TicketRequest) -> Result<TicketResponse, ApiError> {
        if self
            .capabilities()
            .is_some_and(|capabilities| !capabilities.stream_tickets)
        {
            return Err(ApiError::Unsupported(STREAMING_NEEDS_UPDATE));
        }
        match self.ticket(request).await {
            Err(ApiError::Api {
                code: ErrorCode::BadRequest,
                ..
            }) if self.capabilities().is_none() => {
                Err(ApiError::Unsupported(STREAMING_NEEDS_UPDATE))
            }
            result => result,
        }
    }

    /// The server's version, as read on sign-in; `None` before that, when it could not be
    /// read, or for a server from before 0.13.0.
    pub fn server_version(&self) -> Option<String> {
        self.capabilities()?.version
    }

    /// Whether the server reported `feature`, one of [`protocol::features`]; `false` before
    /// its capabilities were read, or when they could not be.
    pub fn supports(&self, feature: &str) -> bool {
        self.capabilities().is_some_and(|capabilities| {
            capabilities
                .features
                .iter()
                .any(|offered| offered == feature)
        })
    }

    /// How a client of `version` that requires `requires` fits this server; `None` when the
    /// server's capabilities could not be read, so nothing is shown rather than a guess.
    pub fn compatibility(&self, version: &str, requires: &[&str]) -> Option<Compatibility> {
        let capabilities = self.capabilities()?;
        Some(compat::check(
            capabilities.version.as_deref(),
            &capabilities.features,
            version,
            requires,
        ))
    }

    /// Finishes a sign-in: reads the server's capabilities, then returns the user. A server
    /// that cannot report them leaves the client on the fallbacks instead of failing the
    /// sign-in.
    pub(crate) async fn signed_in(&self) -> Result<Me, ApiError> {
        let capabilities = self.server_capabilities().await.ok();
        self.set_capabilities(capabilities);
        self.me().await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    async fn server_signing_in() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(paths::LOGIN))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "token", "refresh_token": "refresh", "expires_in": 900
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(paths::ME))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "username": "alice",
                "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0"
            })))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn signing_in_reads_the_servers_upload_chunk_size() {
        let server = server_signing_in().await;
        Mock::given(method("GET"))
            .and(path(paths::CAPABILITIES))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "upload_chunk_bytes": 4096 })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::new(server.uri());

        client.login("alice", "pw").await.expect("login");

        assert_eq!(client.upload_chunk_bytes(), 4096);
    }

    #[tokio::test]
    async fn a_server_without_capabilities_leaves_the_fallback() {
        let server = server_signing_in().await;
        let client = Client::new(server.uri());

        client.login("alice", "pw").await.expect("login");

        assert_eq!(client.upload_chunk_bytes(), FALLBACK_UPLOAD_CHUNK_BYTES);
    }

    fn stream_request() -> TicketRequest {
        TicketRequest::Stream {
            node_id: protocol::NodeId(uuid::Uuid::nil()),
        }
    }

    #[tokio::test]
    async fn an_old_servers_capabilities_say_it_cannot_stream() {
        let server = server_signing_in().await;
        Mock::given(method("GET"))
            .and(path(paths::CAPABILITIES))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "upload_chunk_bytes": 4096 })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(paths::TICKETS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ticket": "t", "expires_in": 30
            })))
            .expect(0)
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.login("alice", "pw").await.expect("login");

        assert_eq!(
            client.stream_ticket(&stream_request()).await,
            Err(ApiError::Unsupported(STREAMING_NEEDS_UPDATE))
        );
    }

    #[tokio::test]
    async fn an_old_server_refusing_a_stream_ticket_needs_an_update() {
        // Signed in without reading capabilities, as a restored session may be.
        let server = server_signing_in().await;
        Mock::given(method("POST"))
            .and(path(paths::TICKETS))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "type": "about:blank",
                "title": "Bad request",
                "status": 422,
                "code": "bad_request",
                "detail": "unknown variant `stream`"
            })))
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.login("alice", "pw").await.expect("login");

        assert_eq!(
            client.stream_ticket(&stream_request()).await,
            Err(ApiError::Unsupported(STREAMING_NEEDS_UPDATE))
        );
    }

    #[tokio::test]
    async fn a_new_server_gives_a_stream_ticket() {
        let server = server_signing_in().await;
        Mock::given(method("GET"))
            .and(path(paths::CAPABILITIES))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "upload_chunk_bytes": 4096, "stream_tickets": true
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(paths::TICKETS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ticket": "t", "expires_in": 3600
            })))
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.login("alice", "pw").await.expect("login");

        let ticket = client
            .stream_ticket(&stream_request())
            .await
            .expect("ticket");
        assert_eq!(ticket.ticket, "t");
    }

    #[test]
    fn a_new_client_uses_the_fallback() {
        assert_eq!(
            Client::new("http://localhost").upload_chunk_bytes(),
            FALLBACK_UPLOAD_CHUNK_BYTES
        );
    }
}
