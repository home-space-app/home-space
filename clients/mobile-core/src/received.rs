//! "Shared with me": what other users shared with the signed-in user, read-only. The apps list
//! it, open its folders, save its files on the phone and save copies into the user's files.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use client_core::{
    format::human_size,
    icon,
    received::{SharedItem, shared_items, shared_viewer},
    shared_link::sort_shared,
    text::TextDoc,
};
use protocol::{NodeKind, SharedNode, TicketRequest};

use crate::{
    CoreClient, CoreError, EntryIcon, EntryKind, files::DownloadListener, files::storage,
    off_ui_thread, parse_id, viewers::app_viewers,
};

/// A file or folder shared with the user, at the top of a share or inside one of its folders.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedEntry {
    /// The share it is reached through.
    pub share_id: String,
    pub id: String,
    pub kind: EntryKind,
    pub icon: EntryIcon,
    pub name: String,
    pub size: i64,
    /// Empty for folders, otherwise e.g. `1.5 MB`.
    pub size_label: String,
    pub mime: String,
    pub updated_at: SystemTime,
    /// Who shared it.
    pub owner: String,
    /// When the share stops working; `None` when it never does.
    pub expires_at: Option<SystemTime>,
}

fn entry(
    share: Uuid,
    owner: &str,
    expires_at: Option<SystemTime>,
    node: SharedNode,
) -> SharedEntry {
    let kind = match node.kind {
        NodeKind::Folder => EntryKind::Folder,
        NodeKind::File => EntryKind::File,
    };
    let icon = match icon::kind_icon(node.kind, &node.mime) {
        icon::NodeIcon::Folder => EntryIcon::Folder,
        icon::NodeIcon::Image => EntryIcon::Image,
        icon::NodeIcon::Video => EntryIcon::Video,
        icon::NodeIcon::Document => EntryIcon::Document,
        icon::NodeIcon::Other => EntryIcon::Other,
    };
    SharedEntry {
        share_id: share.to_string(),
        id: node.id.to_string(),
        kind,
        icon,
        size_label: match kind {
            EntryKind::Folder => String::new(),
            EntryKind::File => human_size(node.size),
        },
        size: node.size,
        name: node.name,
        mime: node.mime,
        updated_at: node.updated_at.into(),
        owner: owner.to_owned(),
        expires_at,
    }
}

fn share_id(id: &str) -> Result<Uuid, CoreError> {
    id.parse()
        .map_err(|_| CoreError::InvalidId { id: id.to_owned() })
}

#[uniffi::export]
impl CoreClient {
    /// Everything shared with the user, one row per shared item: folders first, then by name.
    pub async fn shared_with_me(&self) -> Result<Vec<SharedEntry>, CoreError> {
        let inner = self.inner.clone();
        let shares = off_ui_thread(async move { inner.shared_with_me().await }).await?;
        Ok(shared_items(&shares)
            .into_iter()
            .map(
                |SharedItem {
                     share_id,
                     owner,
                     item,
                     expires_at,
                 }| {
                    entry(share_id, &owner.username, expires_at.map(Into::into), item)
                },
            )
            .collect())
    }

    /// What `folder`, a folder reached through a shared one, holds: folders first, then by name.
    /// Each row carries the share's owner and expiry from `folder`.
    pub async fn shared_children(
        &self,
        folder: SharedEntry,
    ) -> Result<Vec<SharedEntry>, CoreError> {
        let share = share_id(&folder.share_id)?;
        let parent = parse_id(&folder.id)?;
        let inner = self.inner.clone();
        let mut children =
            off_ui_thread(async move { inner.shared_children(share, parent).await }).await?;
        sort_shared(&mut children);
        Ok(children
            .into_iter()
            .map(|node| entry(share, &folder.owner, folder.expires_at, node))
            .collect())
    }

    /// Saves copies of `entries`, all of one share, into the user's folder `parent_id` (Home when
    /// `None`); folders are copied with everything inside.
    pub async fn copy_shared(
        &self,
        entries: Vec<SharedEntry>,
        parent_id: Option<String>,
    ) -> Result<(), CoreError> {
        let Some(first) = entries.first() else {
            return Ok(());
        };
        let share = share_id(&first.share_id)?;
        let ids = entries
            .iter()
            .map(|entry| parse_id(&entry.id))
            .collect::<Result<Vec<_>, _>>()?;
        let parent = parent_id.as_deref().map(parse_id).transpose()?;
        let inner = self.inner.clone();
        off_ui_thread(async move { inner.copy_shared(share, ids, parent).await }).await?;
        Ok(())
    }

    /// The text of shared `file`, for the read-only text viewer.
    pub async fn read_shared_text(&self, file: SharedEntry) -> Result<String, CoreError> {
        let bytes = self.read_shared(file).await?;
        Ok(TextDoc::decode(&bytes).text)
    }

    /// The encoded picture of shared `file`, for the image viewer.
    pub async fn read_shared_image(&self, file: SharedEntry) -> Result<Vec<u8>, CoreError> {
        self.read_shared(file).await
    }

    /// The document of shared `file`, for the PDF viewer.
    pub async fn read_shared_pdf(&self, file: SharedEntry) -> Result<Vec<u8>, CoreError> {
        self.read_shared(file).await
    }

    /// A URL the video player can stream shared `file` from, like [`CoreClient::stream_url`].
    pub async fn shared_stream_url(&self, file: SharedEntry) -> Result<String, CoreError> {
        let share = share_id(&file.share_id)?;
        let id = parse_id(&file.id)?;
        let inner = self.inner.clone();
        off_ui_thread(async move {
            let request = TicketRequest::SharedStream {
                share_id: share,
                node_id: id,
            };
            let ticket = inner.stream_ticket(&request).await?;
            Ok(inner.received_file_url(share, id, &ticket.ticket))
        })
        .await
    }

    /// Downloads a shared file to `dest_path`, which ends up complete or not at all.
    pub async fn save_shared_copy(
        &self,
        file: SharedEntry,
        dest_path: String,
        listener: Arc<dyn DownloadListener>,
    ) -> Result<(), CoreError> {
        let share = share_id(&file.share_id)?;
        let id = parse_id(&file.id)?;
        let inner = self.inner.clone();
        off_ui_thread(async move {
            let dest = PathBuf::from(dest_path);
            let parent = dest.parent().unwrap_or(Path::new("")).to_path_buf();
            fs::create_dir_all(&parent).await.map_err(storage)?;
            let part = parent.join(format!("{id}.part"));
            let written = write(&inner, share, &file, &part, listener.as_ref()).await;
            match written {
                Ok(()) => fs::rename(&part, &dest).await.map_err(storage),
                Err(err) => {
                    let _ = fs::remove_file(&part).await;
                    Err(err)
                }
            }
        })
        .await
    }
}

impl CoreClient {
    /// The content of shared `file`, refused when it has no viewer or is over its limit.
    async fn read_shared(&self, file: SharedEntry) -> Result<Vec<u8>, CoreError> {
        let share = share_id(&file.share_id)?;
        let id = parse_id(&file.id)?;
        let inner = self.inner.clone();
        off_ui_thread(async move {
            let limit = shared_viewer(&app_viewers(), &shared_node(&file, id))
                .map(|viewer| viewer.max_bytes)
                .ok_or_else(|| unviewable(&file, NO_VIEWER))?;
            let bytes = inner
                .download_shared(share, id)
                .await?
                .bytes()
                .await
                .map_err(|err| CoreError::Unreachable {
                    detail: crate::last_cause(&err.to_string()).to_owned(),
                })?;
            if u64::try_from(bytes.len()).is_ok_and(|len| len <= limit) {
                Ok(bytes.to_vec())
            } else {
                Err(unviewable(&file, TOO_LARGE))
            }
        })
        .await
    }
}

const NO_VIEWER: (&str, &str) = ("no_viewer", "cannot be shown here");
const TOO_LARGE: (&str, &str) = ("too_large", "is too large to show here");

fn unviewable(file: &SharedEntry, (code, reason): (&str, &str)) -> CoreError {
    CoreError::Failed {
        code: code.to_owned(),
        detail: format!("“{}” {reason}", file.name),
    }
}

fn shared_node(file: &SharedEntry, id: protocol::NodeId) -> SharedNode {
    SharedNode {
        id,
        kind: match file.kind {
            EntryKind::Folder => NodeKind::Folder,
            EntryKind::File => NodeKind::File,
        },
        name: file.name.clone(),
        size: file.size,
        mime: file.mime.clone(),
        updated_at: file.updated_at.into(),
    }
}

async fn write(
    client: &api_client::Client,
    share: Uuid,
    file: &SharedEntry,
    part: &Path,
    listener: &dyn DownloadListener,
) -> Result<(), CoreError> {
    let id = parse_id(&file.id)?;
    let mut response = client.download_shared(share, id).await?;
    let mut out = fs::File::create(part).await.map_err(storage)?;
    let mut done: i64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| CoreError::Unreachable {
            detail: crate::last_cause(&err.to_string()).to_owned(),
        })?
    {
        out.write_all(&chunk).await.map_err(storage)?;
        done = done.saturating_add(i64::try_from(chunk.len()).unwrap_or(i64::MAX));
        listener.progress(file.id.clone(), done, file.size);
    }
    out.flush().await.map_err(storage)
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use protocol::{NodeId, paths};

    use super::*;
    use crate::tests::signed_in;

    fn shared_file(mime: &str, size: i64) -> SharedEntry {
        let node = SharedNode {
            id: NodeId(Uuid::from_u128(1)),
            kind: NodeKind::File,
            name: "notes.txt".to_owned(),
            size,
            mime: mime.to_owned(),
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        entry(Uuid::from_u128(2), "alex", None, node)
    }

    #[tokio::test]
    async fn a_shared_text_file_reads_through_its_share() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        Mock::given(method("GET"))
            .and(path(paths::shared_file_content(
                Uuid::from_u128(2),
                NodeId(Uuid::from_u128(1)),
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"a\r\nb".to_vec()))
            .mount(&server)
            .await;

        let text = client
            .read_shared_text(shared_file("text/plain", 4))
            .await
            .expect("the text");

        assert_eq!(text, "a\nb");
    }

    #[tokio::test]
    async fn a_shared_file_without_a_viewer_is_refused() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;

        let result = client
            .read_shared_image(shared_file("application/zip", 4))
            .await;

        assert!(matches!(result, Err(CoreError::Failed { code, .. }) if code == "no_viewer"));
    }

    #[test]
    fn a_shared_file_reads_as_an_entry_with_its_owner() {
        let node = SharedNode {
            id: NodeId(Uuid::from_u128(1)),
            kind: NodeKind::File,
            name: "a.pdf".to_owned(),
            size: 2048,
            mime: "application/pdf".to_owned(),
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };

        let entry = entry(Uuid::from_u128(2), "alex", None, node);

        assert_eq!(entry.icon, EntryIcon::Document);
        assert_eq!(entry.size_label, "2.0 KB");
        assert_eq!(entry.owner, "alex");
        assert_eq!(entry.share_id, Uuid::from_u128(2).to_string());
    }
}
