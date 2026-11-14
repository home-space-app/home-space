//! The page showing a file in a built-in viewer, picked by its MIME type in
//! [`client_core::viewers`].

mod image;
mod text;

use std::{path::PathBuf, sync::Arc};

use iced::{
    Alignment, Element, Length, Task,
    widget::{column, row},
};

use api_client::Client;
use client_core::{
    gallery::Gallery,
    text::TextDoc,
    viewers::{Viewer, ViewerKind},
};
use protocol::{Node, NodeId};

use super::{files::SyncedCopy, parts};
use crate::{icons::Icon, theme, viewers::viewers};

pub use self::text::Mode;

const SPACING: f32 = 12.0;
const LOADING: &str = "Loading…";
const EDIT: &str = "Edit";
const CANCEL: &str = "Cancel";
const SAVE: &str = "Save";
const SAVING: &str = "Saving…";
const NOT_UTF8: &str = "This file is not UTF-8 text, so it can only be viewed.";
const CHANGED_ELSEWHERE: &str = "This file was changed elsewhere. Reload drops your edits and \
                                 shows the latest version; Save keeps your edits and the other \
                                 change as a previous version.";
const RELOAD: &str = "Reload";
const PREVIOUS: &str = "Previous";
const NEXT: &str = "Next";
const NOT_LISTED: &str = "The other images of this folder could not be listed";

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Loaded, String>),
    Text(text::Message),
    Edit,
    Cancel,
    /// Drops the edits and shows the version that came from elsewhere.
    Reload,
    Save,
    Saved(Result<(Node, String), String>),
    /// The files of the folder that the viewer steps through.
    Gallery(Result<Gallery, String>),
    Previous,
    Next,
    Close,
}

/// A file's metadata and content, as fetched for the viewer.
#[derive(Debug, Clone)]
pub struct Loaded {
    node: Node,
    bytes: Arc<Vec<u8>>,
}

#[cfg(test)]
impl Loaded {
    pub fn new(node: Node, bytes: &[u8]) -> Self {
        Self {
            node,
            bytes: Arc::new(bytes.to_vec()),
        }
    }
}

#[cfg(test)]
impl Message {
    /// Typing `c` into the editor.
    pub fn typed(c: char) -> Self {
        use iced::widget::text_editor::{Action, Edit};
        Self::Text(text::Message::Action(Action::Edit(Edit::Insert(c))))
    }
}

pub enum Outcome {
    Task(Task<Message>),
    /// Shows this file of the gallery instead.
    Open(Box<Node>),
    Closed,
}

enum Body {
    Loading,
    Failed(String),
    Text(text::State),
    Image(image::State),
}

pub struct State {
    node: Node,
    viewer: Viewer,
    body: Body,
    saving: bool,
    error: Option<String>,
    changed_elsewhere: bool,
    /// The files of the folder that the viewer steps through, once they are listed.
    gallery: Option<Gallery>,
}

impl State {
    /// Opens `node` in `viewer`, reading the synced copy when it is the server's version.
    /// `gallery` holds the files to step through, when they are already listed.
    pub fn open(
        client: &Client,
        node: Node,
        viewer: Viewer,
        synced: Option<SyncedCopy>,
        gallery: Option<Gallery>,
    ) -> (Self, Task<Message>) {
        let task = load(client, node.clone(), viewer, synced);
        let state = Self {
            node,
            viewer,
            body: Body::Loading,
            saving: false,
            error: None,
            changed_elsewhere: false,
            gallery,
        };
        (state, task)
    }

    pub fn node_id(&self) -> NodeId {
        self.node.id
    }

    pub fn name(&self) -> &str {
        &self.node.name
    }

    /// Hands over the gallery, for the viewer of the next file to step through.
    pub fn take_gallery(&mut self) -> Option<Gallery> {
        self.gallery.take()
    }

    /// Applies a live event to the files the viewer steps through.
    pub fn gallery_changed(&mut self, event: &protocol::Event) {
        if let Some(gallery) = &mut self.gallery {
            gallery.apply_event(viewers(), event);
        }
    }

    /// Whether the editor is open.
    pub fn is_editing(&self) -> bool {
        matches!(&self.body, Body::Text(text) if text.mode() == Mode::Edit)
    }

    /// Whether the editor holds edits that are not saved, which leaving would lose.
    pub fn has_unsaved_edits(&self) -> bool {
        matches!(&self.body, Body::Text(text) if text.is_changed())
    }

    pub fn update(&mut self, client: &Client, message: Message) -> Outcome {
        match message {
            Message::Loaded(Ok(loaded)) => {
                self.node = loaded.node;
                self.body = match self.viewer.kind {
                    ViewerKind::Text => {
                        Body::Text(text::State::new(TextDoc::decode(&loaded.bytes)))
                    }
                    ViewerKind::Image => Body::Image(image::State::new(&loaded.bytes)),
                };
                self.changed_elsewhere = false;
            }
            Message::Loaded(Err(err)) => self.body = Body::Failed(err),
            Message::Text(message) => {
                if let Body::Text(text) = &mut self.body {
                    text.update(message);
                }
            }
            Message::Edit => {
                if let Body::Text(text) = &mut self.body
                    && self.viewer.is_editable()
                {
                    text.edit();
                    self.error = None;
                }
            }
            Message::Cancel => {
                if let Body::Text(text) = &mut self.body {
                    text.cancel();
                    self.error = None;
                    if self.changed_elsewhere {
                        return Outcome::Task(self.reload(client));
                    }
                }
            }
            Message::Reload => {
                if !self.saving {
                    self.discard();
                    self.error = None;
                    return Outcome::Task(self.reload(client));
                }
            }
            Message::Save => {
                if let Body::Text(text) = &self.body
                    && !self.saving
                {
                    self.saving = true;
                    self.error = None;
                    return Outcome::Task(save(client, self.node.id, text.text(), text.encoded()));
                }
            }
            Message::Saved(Ok((node, saved))) => {
                self.saving = false;
                self.changed_elsewhere = false;
                self.node = node;
                if let Body::Text(text) = &mut self.body {
                    text.saved(&saved);
                }
            }
            Message::Saved(Err(err)) => {
                self.saving = false;
                self.error = Some(err);
            }
            Message::Gallery(Ok(gallery)) => self.gallery = Some(gallery),
            Message::Gallery(Err(err)) => self.error = Some(format!("{NOT_LISTED}: {err}")),
            Message::Previous => {
                if let Some(node) = self.gallery.as_ref().and_then(Gallery::previous) {
                    return Outcome::Open(Box::new(node.clone()));
                }
            }
            Message::Next => {
                if let Some(node) = self.gallery.as_ref().and_then(Gallery::next) {
                    return Outcome::Open(Box::new(node.clone()));
                }
            }
            Message::Close => return Outcome::Closed,
        }
        Outcome::Task(Task::none())
    }

    /// Drops the unsaved edits, as leaving the viewer does.
    pub fn discard(&mut self) {
        if let Body::Text(text) = &mut self.body {
            text.cancel();
        }
    }

    /// The server says `node`, the file shown, changed: the viewer shows the new content, unless
    /// it is being edited.
    pub fn changed(&mut self, client: &Client, node: &Node) -> Task<Message> {
        if self.saving || node.sha256 == self.node.sha256 {
            self.node = node.clone();
            return Task::none();
        }
        if self.is_editing() {
            self.changed_elsewhere = true;
            Task::none()
        } else {
            self.reload(client)
        }
    }

    fn reload(&mut self, client: &Client) -> Task<Message> {
        self.body = Body::Loading;
        load(client, self.node.clone(), self.viewer, None)
    }

    /// The buttons at the top right of the page.
    pub fn action(&self) -> Element<'_, Message> {
        if self.viewer.kind.navigates() {
            return self.steps();
        }
        let Body::Text(text) = &self.body else {
            return row![].into();
        };
        match text.mode() {
            Mode::Read if self.viewer.is_editable() && text.is_editable() => {
                parts::outlined(Some(Icon::Rename), EDIT, Some(Message::Edit))
            }
            Mode::Read => row![].into(),
            Mode::Edit => {
                let idle = (!self.saving).then_some(());
                row![
                    parts::ghost(None, CANCEL, idle.map(|()| Message::Cancel)),
                    parts::primary(
                        Some(Icon::Check),
                        if self.saving { SAVING } else { SAVE },
                        idle.map(|()| Message::Save),
                    ),
                ]
                .spacing(SPACING)
                .into()
            }
        }
    }

    /// Previous, where the file is in the gallery, and Next; nothing for a single file.
    fn steps(&self) -> Element<'_, Message> {
        let Some(gallery) = &self.gallery else {
            return row![].into();
        };
        let Some((index, total)) = gallery.position().filter(|(_, total)| *total > 1) else {
            return row![].into();
        };
        row![
            parts::ghost(
                Some(Icon::ChevronLeft),
                PREVIOUS,
                gallery.previous().map(|_| Message::Previous),
            ),
            iced::widget::text(format!("{index} / {total}")).style(theme::muted_text),
            parts::ghost(
                Some(Icon::Chevron),
                NEXT,
                gallery.next().map(|_| Message::Next),
            ),
        ]
        .spacing(SPACING)
        .align_y(Alignment::Center)
        .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match &self.body {
            Body::Loading => parts::empty(LOADING),
            Body::Failed(err) => parts::alert_error(Icon::Error, err.as_str(), None),
            Body::Text(text) => text.view().map(Message::Text),
            Body::Image(image) => image.view(),
        };
        let not_utf8 = match &self.body {
            Body::Text(text) if !text.is_editable() => {
                Some(parts::alert_info(Icon::Info, NOT_UTF8, None))
            }
            _ => None,
        };
        let changed = self.changed_elsewhere.then(|| {
            let reload = parts::outlined(
                Some(Icon::Refresh),
                RELOAD,
                (!self.saving).then_some(Message::Reload),
            );
            parts::alert_info(Icon::Info, CHANGED_ELSEWHERE, Some(reload))
        });
        let error = self
            .error
            .as_deref()
            .map(|err| parts::alert_error(Icon::Error, err, None));
        column![]
            .push(error)
            .push(changed)
            .push(not_utf8)
            .push(content)
            .spacing(SPACING)
            .height(Length::Fill)
            .into()
    }
}

/// Fetches the current version of `node`: from the synced copy when it is that version, or
/// while the server can't be reached; otherwise from the server.
fn load(client: &Client, node: Node, viewer: Viewer, synced: Option<SyncedCopy>) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move {
            let current = match client.get(node.id).await {
                Ok(current) => current,
                Err(err) if err.is_unreachable() => {
                    return match synced {
                        Some(copy) => read_local(node, copy.path, viewer).await,
                        None => Err(err.to_string()),
                    };
                }
                Err(err) => return Err(err.to_string()),
            };
            match synced.filter(|copy| !copy.is_behind(&current)) {
                Some(copy) => read_local(current, copy.path, viewer).await,
                None => download(&client, current, viewer).await,
            }
        },
        Message::Loaded,
    )
}

async fn read_local(node: Node, path: PathBuf, viewer: Viewer) -> Result<Loaded, String> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|err| format!("{}: {err}", path.display()))?;
    within_limit(node, bytes, viewer)
}

async fn download(client: &Client, node: Node, viewer: Viewer) -> Result<Loaded, String> {
    let bytes = client
        .download(node.id)
        .await
        .map_err(|err| err.to_string())?
        .bytes()
        .await
        .map_err(|err| err.to_string())?;
    within_limit(node, bytes.to_vec(), viewer)
}

fn within_limit(node: Node, bytes: Vec<u8>, viewer: Viewer) -> Result<Loaded, String> {
    let fits = u64::try_from(bytes.len()).is_ok_and(|len| len <= viewer.max_bytes);
    if fits {
        Ok(Loaded {
            node,
            bytes: Arc::new(bytes),
        })
    } else {
        Err(format!(
            "{} is too large to show here; open it with the system app instead.",
            node.name
        ))
    }
}

fn save(client: &Client, id: NodeId, text: String, bytes: Vec<u8>) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { store(&client, id, bytes).await.map(|node| (node, text)) },
        Message::Saved,
    )
}

/// Stores `bytes` as the content of file `id`, which makes the previous content a version.
async fn store(client: &Client, id: NodeId, bytes: Vec<u8>) -> Result<Node, String> {
    client
        .replace_with_bytes(id, None, bytes)
        .await
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use protocol::{NodeKind, paths};
    use time::OffsetDateTime;
    use uuid::Uuid;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_bytes, method, path},
    };

    use super::*;

    const TEXT: &str = "hello\n";

    fn node(version: i32, sha256: &str) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(5)),
            parent_id: None,
            kind: NodeKind::File,
            name: "notes.txt".to_owned(),
            size: 6,
            sha256: Some(sha256.to_owned()),
            mime: "text/plain".to_owned(),
            version,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    fn text_viewer() -> Viewer {
        viewers()
            .viewer_for("text/plain", 0)
            .expect("text has a viewer")
    }

    fn loaded(client: &Client, bytes: &[u8]) -> State {
        let (mut state, _task) = State::open(client, node(1, "a"), text_viewer(), None, None);
        let _ = state.update(
            client,
            Message::Loaded(Ok(Loaded {
                node: node(1, "a"),
                bytes: Arc::new(bytes.to_vec()),
            })),
        );
        state
    }

    fn messages(state: &State, click: &str) -> Vec<Message> {
        let mut ui = simulator(state.action());
        ui.click(click).expect("the button is shown");
        ui.into_messages().collect()
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    async fn signed_in() -> (MockServer, Client) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(paths::LOGIN))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "token", "refresh_token": "refresh", "expires_in": 900
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(paths::ME))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": Uuid::nil(), "username": "alice", "lost_and_found_id": Uuid::nil()
            })))
            .mount(&server)
            .await;
        let client = Client::new(server.uri());
        client.login("alice", "pw").await.expect("login");
        (server, client)
    }

    #[test]
    fn a_text_file_opens_read_only_with_edit() {
        let state = loaded(&client(), TEXT.as_bytes());
        assert!(!state.is_editing());
        assert!(matches!(messages(&state, EDIT).as_slice(), [Message::Edit]));
    }

    #[test]
    fn editing_offers_cancel_and_save() {
        let client = client();
        let mut state = loaded(&client, TEXT.as_bytes());
        let _ = state.update(&client, Message::Edit);
        assert!(state.is_editing());
        assert!(matches!(
            messages(&state, CANCEL).as_slice(),
            [Message::Cancel]
        ));
        assert!(matches!(messages(&state, SAVE).as_slice(), [Message::Save]));
    }

    #[test]
    fn cancel_returns_to_reading() {
        let client = client();
        let mut state = loaded(&client, TEXT.as_bytes());
        let _ = state.update(&client, Message::Edit);
        let _ = state.update(&client, Message::Cancel);
        assert!(!state.is_editing());
    }

    #[test]
    fn content_that_is_not_utf8_offers_no_edit() {
        let state = loaded(&client(), b"\xFF");
        assert!(simulator(state.action()).find(EDIT).is_err());
    }

    #[test]
    fn a_saved_file_is_read_again_in_its_new_version() {
        let client = client();
        let mut state = loaded(&client, TEXT.as_bytes());
        let _ = state.update(&client, Message::Edit);
        let _ = state.update(&client, Message::Save);
        let _ = state.update(
            &client,
            Message::Saved(Ok((node(2, "b"), "bye".to_owned()))),
        );
        assert!(!state.is_editing());
        assert_eq!(state.node.version, 2);
    }

    #[test]
    fn a_failed_save_keeps_the_edits() {
        let client = client();
        let mut state = loaded(&client, TEXT.as_bytes());
        let _ = state.update(&client, Message::Edit);
        let _ = state.update(&client, Message::Save);
        let _ = state.update(&client, Message::Saved(Err("offline".to_owned())));
        assert!(state.is_editing());
        assert_eq!(state.error.as_deref(), Some("offline"));
    }

    #[test]
    fn a_change_elsewhere_while_editing_is_announced() {
        let client = client();
        let mut state = loaded(&client, TEXT.as_bytes());
        let _ = state.update(&client, Message::Edit);
        let _ = state.changed(&client, &node(2, "c"));
        assert!(state.changed_elsewhere);
        assert!(state.is_editing());
    }

    #[test]
    fn the_change_notice_offers_reload() {
        let client = client();
        let mut state = loaded(&client, TEXT.as_bytes());
        let _ = state.update(&client, Message::Edit);
        let _ = state.changed(&client, &node(2, "c"));

        let mut ui = simulator(state.view());
        ui.click(RELOAD).expect("Reload is offered");

        let messages: Vec<_> = ui.into_messages().collect();
        assert!(matches!(messages.as_slice(), [Message::Reload]));
    }

    #[test]
    fn reload_drops_the_edits_and_loads_the_latest_version() {
        let client = client();
        let mut state = loaded(&client, TEXT.as_bytes());
        let _ = state.update(&client, Message::Edit);
        let _ = state.update(&client, Message::typed('x'));
        let _ = state.changed(&client, &node(2, "c"));

        let _ = state.update(&client, Message::Reload);

        assert!(matches!(state.body, Body::Loading));
        assert!(!state.has_unsaved_edits());
    }

    #[test]
    fn a_change_elsewhere_while_reading_reloads() {
        let client = client();
        let mut state = loaded(&client, TEXT.as_bytes());
        let _ = state.changed(&client, &node(2, "c"));
        assert!(matches!(state.body, Body::Loading));
    }

    #[test]
    fn content_over_the_limit_is_refused() {
        let viewer = Viewer {
            max_bytes: 2,
            ..text_viewer()
        };
        assert!(within_limit(node(1, "a"), b"abc".to_vec(), viewer).is_err());
    }

    #[tokio::test]
    async fn saving_replaces_the_content_on_the_server() {
        let (server, client) = signed_in().await;
        let saved = node(2, "b");
        Mock::given(method("PUT"))
            .and(path(paths::file_content(saved.id)))
            .and(body_bytes(b"bye\r\n".to_vec()))
            .respond_with(ResponseTemplate::new(200).set_body_json(&saved))
            .expect(1)
            .mount(&server)
            .await;

        let stored = store(
            &client,
            saved.id,
            TextDoc::decode(b"hi\r\n").encode("bye\n"),
        )
        .await;

        assert_eq!(stored, Ok(saved));
    }

    #[tokio::test]
    async fn the_content_is_downloaded_from_the_server() {
        let (server, client) = signed_in().await;
        let file = node(1, "a");
        Mock::given(method("GET"))
            .and(path(paths::file_content(file.id)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(TEXT.as_bytes()))
            .mount(&server)
            .await;

        let loaded = download(&client, file, text_viewer())
            .await
            .expect("downloaded");

        assert_eq!(loaded.bytes.as_slice(), TEXT.as_bytes());
    }

    fn image(raw: u128, name: &str) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(raw)),
            name: name.to_owned(),
            mime: "image/png".to_owned(),
            ..node(1, "a")
        }
    }

    fn viewing_image(raw: u128) -> (Client, State) {
        let client = client();
        let listing = [image(1, "a.png"), image(2, "b.png"), image(3, "c.png")];
        let current = listing
            .iter()
            .find(|node| node.id == NodeId(Uuid::from_u128(raw)));
        let current = current.expect("the image is listed").clone();
        let viewer = viewers()
            .viewer_for_node(&current)
            .expect("images have a viewer");
        let gallery = Gallery::new(viewers(), &listing, &current, Default::default());
        let (state, _task) = State::open(&client, current, viewer, None, gallery);
        (client, state)
    }

    #[test]
    fn an_image_offers_previous_and_next_with_its_position() {
        let (_client, state) = viewing_image(2);
        let mut ui = simulator(state.action());
        assert!(ui.find("2 / 3").is_ok());
        ui.click(NEXT).expect("Next is offered");
        ui.click(PREVIOUS).expect("Previous is offered");
        let messages: Vec<_> = ui.into_messages().collect();
        assert!(matches!(
            messages.as_slice(),
            [Message::Next, Message::Previous]
        ));
    }

    #[test]
    fn next_opens_the_following_image() {
        let (client, mut state) = viewing_image(2);
        let outcome = state.update(&client, Message::Next);
        assert!(matches!(outcome, Outcome::Open(node) if node.name == "c.png"));
    }

    #[test]
    fn next_at_the_last_image_stays() {
        let (client, mut state) = viewing_image(3);
        let outcome = state.update(&client, Message::Next);
        assert!(matches!(outcome, Outcome::Task(_)));
    }

    #[test]
    fn previous_at_the_first_image_stays() {
        let (client, mut state) = viewing_image(1);
        let outcome = state.update(&client, Message::Previous);
        assert!(matches!(outcome, Outcome::Task(_)));
    }

    #[test]
    fn a_single_image_offers_no_steps() {
        let client = client();
        let current = image(1, "a.png");
        let viewer = viewers().viewer_for_node(&current).expect("image viewer");
        let gallery = Gallery::new(
            viewers(),
            std::slice::from_ref(&current),
            &current,
            Default::default(),
        );
        let (state, _task) = State::open(&client, current, viewer, None, gallery);
        assert!(simulator(state.action()).find(NEXT).is_err());
    }

    #[test]
    fn a_failed_listing_is_reported() {
        let (client, mut state) = viewing_image(1);
        let _ = state.update(&client, Message::Gallery(Err("offline".to_owned())));
        assert_eq!(
            state.error.as_deref(),
            Some("The other images of this folder could not be listed: offline")
        );
    }
}
