//! Where the file a viewer shows comes from: one of the user's own files, one shared with
//! them, or a file of a protected folder, decrypted in the page. The PDF and video viewers read
//! any of them the same way.

use protocol::{NodeId, TicketRequest};
use uuid::Uuid;

use super::{download_failed, fetch_bytes};
use crate::{
    protected,
    session::{self, Auth},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Own(NodeId),
    Shared {
        share: Uuid,
        node: NodeId,
    },
    /// File `node`, of `size` bytes as stored, in the unlocked protected folder `folder`.
    Protected {
        folder: NodeId,
        node: NodeId,
        size: i64,
    },
}

impl Source {
    /// The file's whole content, through a single-use download ticket.
    pub(super) async fn bytes(self, auth: Auth) -> Result<Vec<u8>, String> {
        let request = match self {
            Self::Own(node_id) => TicketRequest::Download { node_id },
            Self::Shared { share, node } => TicketRequest::SharedFile {
                share_id: share,
                node_id: node,
            },
            Self::Protected { folder, node, size } => {
                return protected::download_plain(protected::vault(), folder, node, size).await;
            }
        };
        let url = self.url(auth, &request).await?;
        fetch_bytes(&url).await.map_err(|err| download_failed(&err))
    }

    /// A URL a player can fetch the file from in as many range requests as it needs, until the
    /// stream ticket in it expires. A protected folder's file is decrypted whole into a `blob:`
    /// URL instead, revoked when the folder locks.
    pub(super) async fn stream_url(self, auth: Auth) -> Result<String, String> {
        let request = match self {
            Self::Own(node_id) => TicketRequest::Stream { node_id },
            Self::Shared { share, node } => TicketRequest::SharedStream {
                share_id: share,
                node_id: node,
            },
            Self::Protected { folder, .. } => {
                let plain = self.bytes(auth).await?;
                // Without a type: the player tells the format from the bytes.
                let url = protected::untyped_blob_url(&plain)
                    .ok_or_else(|| "The video could not be loaded.".to_owned())?;
                protected::vault().keep_url(folder, url.clone());
                return Ok(url);
            }
        };
        self.url(auth, &request).await
    }

    async fn url(self, auth: Auth, request: &TicketRequest) -> Result<String, String> {
        let client = session::client();
        let ticket = match request {
            TicketRequest::Stream { .. } | TicketRequest::SharedStream { .. } => {
                client.stream_ticket(request).await
            }
            _ => client.ticket(request).await,
        }
        .map_err(|err| auth.fail(&err))?;
        Ok(match self {
            Self::Own(node) | Self::Protected { node, .. } => {
                client.download_url(node, &ticket.ticket)
            }
            Self::Shared { share, node } => client.received_file_url(share, node, &ticket.ticket),
        })
    }
}

/// "… is too large to show here", for content over the viewer's limit.
pub(super) fn too_large(name: &str) -> String {
    format!("{name} is too large to show here. Download it to open it.")
}
