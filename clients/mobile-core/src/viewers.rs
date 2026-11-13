//! The built-in viewers of [`client_core::viewers`] for the apps, and reading and saving the
//! content the text viewer shows.

use client_core::{
    text::{LineEnding, TextDoc},
    viewers::{self, Editing},
};
use protocol::Node;

use crate::{CoreClient, CoreError, Entry, files, off_ui_thread, parse_id};

/// Which built-in viewer shows a file; the apps have one screen for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ViewerKind {
    Text,
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

/// The built-in viewer for a file of type `mime` that is `size` bytes, if it has one.
#[uniffi::export]
pub fn viewer_for(mime: String, size: i64) -> Option<ViewerInfo> {
    viewers::viewer_for(&mime, size).map(|viewer| ViewerInfo {
        kind: match viewer.kind {
            viewers::ViewerKind::Text => ViewerKind::Text,
        },
        editable: viewer.is_editable(),
    })
}

#[uniffi::export]
impl CoreClient {
    /// The current content of text file `id`, from the offline copy when it is that version and
    /// otherwise from the server. Reading it does not keep the file on the phone.
    pub async fn read_text(&self, id: String) -> Result<TextFile, CoreError> {
        let id = parse_id(&id)?;
        let client = self.inner.clone();
        let local = self.files.clone();
        off_ui_thread(async move {
            let node = client.get(id).await?;
            let limit = viewers::viewer_for_node(&node)
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
                Ok(text_file(node, &bytes))
            } else {
                Err(too_large(&node))
            }
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
