//! The built-in viewers of [`client_core::viewers`] for the apps, reading the content the text
//! and image viewers show, saving text, and listing the images the image viewer steps through.

use std::sync::{LazyLock, PoisonError, RwLock};

use client_core::{
    gallery,
    listing::Sort,
    text::{LineEnding, TextDoc},
    viewers::{self, Editing, Viewers},
};
use protocol::Node;

use crate::{CoreClient, CoreError, Entry, files, off_ui_thread, parse_id};

/// The viewers of the app: the shared ones, plus the types the phone decodes, which the app
/// adds with [`add_viewer_types`] when it starts.
static VIEWERS: LazyLock<RwLock<Viewers>> = LazyLock::new(|| RwLock::new(Viewers::shared()));

/// Which built-in viewer shows a file; the apps have one screen for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ViewerKind {
    Text,
    Image,
}

impl From<viewers::ViewerKind> for ViewerKind {
    fn from(kind: viewers::ViewerKind) -> Self {
        match kind {
            viewers::ViewerKind::Text => Self::Text,
            viewers::ViewerKind::Image => Self::Image,
        }
    }
}

impl From<ViewerKind> for viewers::ViewerKind {
    fn from(kind: ViewerKind) -> Self {
        match kind {
            ViewerKind::Text => Self::Text,
            ViewerKind::Image => Self::Image,
        }
    }
}

/// The built-in viewer for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct ViewerInfo {
    pub kind: ViewerKind,
    /// Whether the viewer offers Edit.
    pub editable: bool,
}

/// How a text file is written, so saving it keeps that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TextFormat {
    pub crlf: bool,
    pub bom: bool,
}

/// A text file to show in the text viewer.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TextFile {
    /// The file as it was when its content was read.
    pub entry: Entry,
    /// The content, with `\n` line breaks.
    pub text: String,
    /// False for content that is not UTF-8, which is shown with replacement characters.
    pub editable: bool,
    pub format: TextFormat,
}

/// An image file to show in the image viewer.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ImageFile {
    /// The file as it was when its content was read.
    pub entry: Entry,
    /// The encoded picture, as stored.
    pub bytes: Vec<u8>,
}

/// The files of a folder the image viewer steps through.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Gallery {
    /// The files, in the order the folder lists them.
    pub entries: Vec<Entry>,
    /// Where the file asked for is in `entries`; `None` when it is not there.
    pub index: Option<u32>,
}

/// Shows files of `types` in the viewer of `kind` too, for types the phone can decode beyond
/// the ones every client shows (e.g. `image/heic`). Applies to every client of the app.
#[uniffi::export]
pub fn add_viewer_types(kind: ViewerKind, types: Vec<String>) {
    let mut viewers = VIEWERS.write().unwrap_or_else(PoisonError::into_inner);
    *viewers = viewers.clone().with_types(kind.into(), types);
}

/// The built-in viewer for a file of type `mime` that is `size` bytes, if it has one.
#[uniffi::export]
pub fn viewer_for(mime: String, size: i64) -> Option<ViewerInfo> {
    app_viewers()
        .viewer_for(&mime, size)
        .map(|viewer| ViewerInfo {
            kind: viewer.kind.into(),
            editable: viewer.is_editable(),
        })
}

fn app_viewers() -> Viewers {
    VIEWERS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[uniffi::export]
impl CoreClient {
    /// The current content of text file `id`, from the offline copy when it is that version and
    /// otherwise from the server. Reading it does not keep the file on the phone.
    pub async fn read_text(&self, id: String) -> Result<TextFile, CoreError> {
        let (node, bytes) = self.read_content(&id).await?;
        Ok(text_file(node, &bytes))
    }

    /// The current content of image file `id`, read like [`CoreClient::read_text`].
    pub async fn read_image(&self, id: String) -> Result<ImageFile, CoreError> {
        let (node, bytes) = self.read_content(&id).await?;
        Ok(ImageFile {
            entry: Entry::from(node),
            bytes,
        })
    }

    /// The files of the folder holding file `id` that its viewer steps through, by name. Empty
    /// when its viewer does not step between files.
    pub async fn gallery(&self, id: String) -> Result<Gallery, CoreError> {
        let id = parse_id(&id)?;
        let client = self.inner.clone();
        let lost_and_found = *self.lost_and_found.lock().expect("lost and found lock");
        off_ui_thread(async move {
            let node = client.get(id).await?;
            let listing = match (node.parent_id, lost_and_found) {
                (None, Some(found)) => client.list_home(found).await,
                (parent, _) => client.list(parent).await,
            }?;
            Ok(gallery_of(&app_viewers(), &listing, &node))
        })
        .await
    }

    /// Stores `text` as the content of file `id`, written in `format`. The previous content
    /// becomes a version of the file; the returned entry is the new current one.
    pub async fn save_text(
        &self,
        id: String,
        text: String,
        format: TextFormat,
    ) -> Result<Entry, CoreError> {
        let id = parse_id(&id)?;
        let client = self.inner.clone();
        off_ui_thread(async move {
            let bytes = doc_for(format).encode(&text);
            let node = client.replace_with_bytes(id, None, bytes).await?;
            Ok(Entry::from(node))
        })
        .await
    }
}

impl CoreClient {
    /// The current content of file `id`, from the offline copy when it is that version and
    /// otherwise from the server, refused when it has no viewer or is over the viewer's limit.
    async fn read_content(&self, id: &str) -> Result<(Node, Vec<u8>), CoreError> {
        let id = parse_id(id)?;
        let client = self.inner.clone();
        let local = self.files.clone();
        off_ui_thread(async move {
            let node = client.get(id).await?;
            let limit = app_viewers()
                .viewer_for_node(&node)
                .map(|viewer| viewer.max_bytes)
                .ok_or_else(|| no_viewer(&node))?;
            let copy = match local.offline(&client).await {
                Ok(offline) => offline
                    .store
                    .base(id)
                    .ok()
                    .flatten()
                    .filter(|base| {
                        base.sha256 == node.sha256 && !base.is_edited() && base.path.is_file()
                    })
                    .map(|base| base.path),
                Err(_) => None,
            };
            let bytes = match copy {
                Some(path) => tokio::fs::read(&path).await.map_err(files::storage)?,
                None => client
                    .download(id)
                    .await?
                    .bytes()
                    .await
                    .map_err(transport)?
                    .to_vec(),
            };
            if u64::try_from(bytes.len()).is_ok_and(|len| len <= limit) {
                Ok((node, bytes))
            } else {
                Err(too_large(&node))
            }
        })
        .await
    }
}

fn gallery_of(viewers: &Viewers, listing: &[Node], node: &Node) -> Gallery {
    match gallery::Gallery::new(viewers, listing, node, Sort::default()) {
        Some(found) => Gallery {
            index: found
                .position()
                .and_then(|(position, _)| u32::try_from(position - 1).ok()),
            entries: found.nodes().iter().cloned().map(Entry::from).collect(),
        },
        None => Gallery {
            entries: Vec::new(),
            index: None,
        },
    }
}

fn text_file(node: Node, bytes: &[u8]) -> TextFile {
    let doc = TextDoc::decode(bytes);
    TextFile {
        editable: doc.is_editable(),
        format: TextFormat {
            crlf: doc.line_ending == LineEnding::CrLf,
            bom: doc.bom,
        },
        text: doc.text,
        entry: Entry::from(node),
    }
}

fn doc_for(format: TextFormat) -> TextDoc {
    TextDoc {
        text: String::new(),
        line_ending: if format.crlf {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        },
        bom: format.bom,
        editing: Editing::Editable,
    }
}

fn no_viewer(node: &Node) -> CoreError {
    CoreError::Failed {
        code: "no_viewer".to_owned(),
        detail: format!("“{}” cannot be shown here", node.name),
    }
}

fn too_large(node: &Node) -> CoreError {
    CoreError::Failed {
        code: "too_large".to_owned(),
        detail: format!("“{}” is too large to show here", node.name),
    }
}

fn transport(err: reqwest::Error) -> CoreError {
    CoreError::Unreachable {
        detail: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_bytes, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::tests::{node, signed_in};

    const FILE_ID: &str = "00000000-0000-0000-0000-000000000009";

    fn text_node(mime: &str, size: i64) -> serde_json::Value {
        let mut json = node("notes.txt", "file", size);
        json["mime"] = mime.into();
        json
    }

    async fn mount_node(server: &MockServer, json: &serde_json::Value) {
        Mock::given(method("GET"))
            .and(path(format!("{}/{FILE_ID}", protocol::paths::NODES)))
            .respond_with(ResponseTemplate::new(200).set_body_json(json))
            .mount(server)
            .await;
    }

    fn content_path() -> String {
        format!("{}/{FILE_ID}/content", protocol::paths::FILES)
    }

    #[test]
    fn text_types_have_the_editable_text_viewer() {
        assert_eq!(
            viewer_for("text/plain".into(), 1),
            Some(ViewerInfo {
                kind: ViewerKind::Text,
                editable: true
            })
        );
    }

    #[test]
    fn image_types_have_the_read_only_image_viewer() {
        assert_eq!(
            viewer_for("image/jpeg".into(), 1),
            Some(ViewerInfo {
                kind: ViewerKind::Image,
                editable: false
            })
        );
    }

    #[test]
    fn added_types_open_in_their_viewer() {
        add_viewer_types(ViewerKind::Image, vec!["image/x-test-added".into()]);
        assert_eq!(
            viewer_for("image/x-test-added".into(), 1).map(|info| info.kind),
            Some(ViewerKind::Image)
        );
    }

    #[test]
    fn the_gallery_holds_the_images_of_the_folder_in_order() {
        let listing: Vec<Node> = [
            ("b.png", "image/png", 2),
            ("a.txt", "text/plain", 3),
            ("a.png", "image/png", 1),
        ]
        .into_iter()
        .map(|(name, mime, id)| Node {
            id: protocol::NodeId(uuid::Uuid::from_u128(id)),
            parent_id: None,
            kind: protocol::NodeKind::File,
            name: name.to_owned(),
            size: 1,
            sha256: None,
            mime: mime.to_owned(),
            version: 1,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        })
        .collect();

        let gallery = gallery_of(&Viewers::shared(), &listing, &listing[0]);

        let names: Vec<_> = gallery
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, ["a.png", "b.png"]);
        assert_eq!(gallery.index, Some(1));
    }

    #[test]
    fn text_files_have_an_empty_gallery() {
        let listing = [protocol::Node {
            mime: "text/plain".to_owned(),
            ..serde_json::from_value(node("a.txt", "file", 1)).unwrap()
        }];
        let gallery = gallery_of(&Viewers::shared(), &listing, &listing[0]);
        assert!(gallery.entries.is_empty());
        assert_eq!(gallery.index, None);
    }

    #[tokio::test]
    async fn read_image_downloads_the_bytes() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_node(&server, &text_node("image/png", 3)).await;
        Mock::given(method("GET"))
            .and(path(content_path()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"png".to_vec()))
            .mount(&server)
            .await;

        let file = client.read_image(FILE_ID.into()).await.unwrap();

        assert_eq!(file.bytes, b"png");
    }

    #[test]
    fn other_types_have_no_viewer() {
        assert_eq!(viewer_for("application/zip".into(), 1), None);
    }

    #[tokio::test]
    async fn read_text_downloads_and_decodes_the_content() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_node(&server, &text_node("text/plain", 7)).await;
        Mock::given(method("GET"))
            .and(path(content_path()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"a\r\nb\r\n".to_vec()))
            .mount(&server)
            .await;

        let file = client.read_text(FILE_ID.into()).await.unwrap();

        assert_eq!(file.text, "a\nb\n");
        assert!(file.editable);
        assert_eq!(
            file.format,
            TextFormat {
                crlf: true,
                bom: false
            }
        );
    }

    #[tokio::test]
    async fn read_text_refuses_files_without_a_viewer() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        mount_node(&server, &text_node("application/zip", 7)).await;

        let err = client.read_text(FILE_ID.into()).await.unwrap_err();

        assert!(matches!(err, CoreError::Failed { code, .. } if code == "no_viewer"));
    }

    #[tokio::test]
    async fn save_text_replaces_the_content_in_its_format() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let mut saved = text_node("text/plain", 5);
        saved["version"] = 2.into();
        Mock::given(method("PUT"))
            .and(path(content_path()))
            .and(header("authorization", "Bearer access"))
            .and(body_bytes(b"\xEF\xBB\xBFx\r\ny".to_vec()))
            .respond_with(ResponseTemplate::new(200).set_body_json(&saved))
            .expect(1)
            .mount(&server)
            .await;

        let entry = client
            .save_text(
                FILE_ID.into(),
                "x\ny".into(),
                TextFormat {
                    crlf: true,
                    bom: true,
                },
            )
            .await
            .unwrap();

        assert_eq!(entry.version, 2);
    }
}
