use iced::{
    Alignment, Element, Length, Padding, Task,
    keyboard::Modifiers,
    widget::{button, column, container, row, scrollable, space, text},
};

use api_client::{ApiError, Client};
use client_core::{
    format::human_size,
    labels::{LOST_AND_FOUND, files},
};
use protocol::{MoveResponse, Node, NodeId, OnConflict, TrashItem};

use super::{
    grid::check,
    parts::{
        CHECK_LIST_PADDING_WIDE, COLUMN_GAP, DATE_WIDTH, ROW_HEIGHT, ROW_SIZE, SECOND_LINE_SIZE,
        SIZE_WIDTH, alert_error, alert_info, divider, empty, ghost_danger, header_bar, list_box,
        muted_cell, page_action, small_outlined, small_primary, title,
    },
    selection::{Coverage, Selection},
};
use crate::{
    ellipsis::ellipsized,
    icons::{ICON_SIZE, Icon, Tint, node_icon},
    locale::short_date,
    theme,
};

const SPACING: f32 = 12.0;
const NAME_GAP: f32 = 12.0;
const NAME_PORTION: u16 = 3;
const PATH_PORTION: u16 = 2;
const BAR_PADDING: Padding = Padding {
    top: 4.0,
    right: 12.0,
    bottom: 4.0,
    left: 12.0,
};

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<TrashItem>, ApiError>),
    Click(NodeId),
    Toggle(NodeId),
    ToggleAll,
    Restore,
    Restored(Result<Box<MoveResponse>, ApiError>),
    Empty,
    OpenLostAndFound(NodeId),
}

/// What the browser has to do after a recycle bin update.
pub enum Outcome {
    Task(Task<Message>),
    /// Some files could not be restored; the user decides how to go on.
    Conflicts {
        ids: Vec<NodeId>,
        response: Box<MoveResponse>,
    },
    /// Ask for confirmation before permanently deleting this many files.
    ConfirmEmpty(usize),
    /// Show the folder with this id.
    OpenFolder(NodeId),
}

/// The outcome of the last restore, for the notice above the list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Restored {
    count: usize,
    folder: Option<NodeId>,
}

/// The recycle bin view: the binned files and what is selected.
#[derive(Default)]
pub struct State {
    items: Vec<TrashItem>,
    /// The nodes of `items`, in the same order, for the selection.
    nodes: Vec<Node>,
    selection: Selection,
    loading: bool,
    error: Option<String>,
    restoring: Vec<NodeId>,
    restored: Option<Restored>,
}

impl State {
    /// A bin view that is still waiting for its first listing.
    pub fn open() -> Self {
        Self {
            loading: true,
            ..Self::default()
        }
    }

    /// Fetches the binned files.
    pub fn load(&mut self, client: &Client) -> Task<Message> {
        self.loading = true;
        let client = client.clone();
        Task::perform(async move { client.trash().await }, Message::Loaded)
    }

    pub fn select_all(&mut self) {
        self.selection.select_all(&self.nodes);
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub fn update(&mut self, client: &Client, modifiers: Modifiers, message: Message) -> Outcome {
        match message {
            Message::Loaded(result) => {
                self.loading = false;
                match result {
                    Ok(items) => {
                        self.nodes = items.iter().map(|item| item.node.clone()).collect();
                        self.items = items;
                        self.selection.retain(&self.nodes);
                        self.error = None;
                    }
                    Err(err) => self.error = Some(err.to_string()),
                }
                Outcome::Task(Task::none())
            }
            Message::Click(id) => {
                self.selection.click(id, modifiers, &self.nodes);
                Outcome::Task(Task::none())
            }
            Message::Toggle(id) => {
                self.selection.toggle(id);
                Outcome::Task(Task::none())
            }
            Message::ToggleAll => {
                self.selection.toggle_all(&self.nodes);
                Outcome::Task(Task::none())
            }
            Message::Restore => {
                let ids = self.selected_ids();
                if ids.is_empty() {
                    return Outcome::Task(Task::none());
                }
                self.restored = None;
                self.restoring.clone_from(&ids);
                let client = client.clone();
                Outcome::Task(Task::perform(
                    async move {
                        client
                            .restore_trashed(&ids, OnConflict::Abort)
                            .await
                            .map(Box::new)
                    },
                    Message::Restored,
                ))
            }
            Message::Restored(Ok(response)) if !response.conflicts.is_empty() => {
                Outcome::Conflicts {
                    ids: std::mem::take(&mut self.restoring),
                    response,
                }
            }
            Message::Restored(Ok(response)) => {
                self.restored_with(&response);
                Outcome::Task(self.load(client))
            }
            Message::Restored(Err(err)) => {
                self.error = Some(err.to_string());
                Outcome::Task(self.load(client))
            }
            Message::Empty if self.items.is_empty() => Outcome::Task(Task::none()),
            Message::Empty => Outcome::ConfirmEmpty(self.items.len()),
            Message::OpenLostAndFound(id) => Outcome::OpenFolder(id),
        }
    }

    /// Records a finished restore: shows the notice and drops the selection.
    pub fn restored_with(&mut self, response: &MoveResponse) {
        self.restoring.clear();
        self.error = None;
        self.restored = Some(Restored {
            count: response.moved.len(),
            folder: response.target.as_ref().map(|folder| folder.id),
        });
        self.selection.clear();
    }

    fn selected_ids(&self) -> Vec<NodeId> {
        self.selection
            .selected(&self.nodes)
            .into_iter()
            .map(|node| node.id)
            .collect()
    }

    /// What the notice above the list says, if there is one.
    fn notice(&self) -> Option<String> {
        self.restored
            .as_ref()
            .map(|done| format!("Restored {} to {LOST_AND_FOUND}", files(done.count)))
    }

    /// What the page's top line offers.
    pub fn action(&self) -> Element<'_, Message> {
        page_action(
            self.loading,
            ghost_danger(
                Icon::EmptyBin,
                "Empty recycle bin",
                (!self.items.is_empty()).then_some(Message::Empty),
            ),
        )
    }

    pub fn view(&self) -> Element<'_, Message> {
        let mut page = column![].spacing(SPACING);
        let selected = self.selection.selected(&self.nodes).len();
        if selected > 0 {
            page = page.push(selection_bar(selected));
        }
        if let Some(error) = &self.error {
            page = page.push(alert_error(Icon::Error, error.as_str(), None));
        }
        if let Some(notice) = self.notice() {
            let open = self
                .restored
                .as_ref()
                .and_then(|done| done.folder)
                .map(|folder| {
                    small_outlined(
                        format!("Open {LOST_AND_FOUND}"),
                        Some(Message::OpenLostAndFound(folder)),
                    )
                });
            page = page.push(alert_info(Icon::Info, notice, open));
        }
        page.push(list_box(column![self.header(), self.rows()]))
            .into()
    }

    fn header(&self) -> Element<'_, Message> {
        header_bar(
            row![
                check(self.selection.coverage(&self.nodes), Message::ToggleAll),
                title("Name", Length::FillPortion(NAME_PORTION), Alignment::Start),
                title(
                    "Original path",
                    Length::FillPortion(PATH_PORTION),
                    Alignment::Start
                ),
                title("Size", SIZE_WIDTH, Alignment::End),
                title("Deleted", DATE_WIDTH, Alignment::End),
            ],
            CHECK_LIST_PADDING_WIDE,
        )
    }

    fn rows(&self) -> Element<'_, Message> {
        if self.items.is_empty() {
            return empty("The recycle bin is empty.");
        }
        let rows = self.items.iter().flat_map(|item| {
            [
                item_row(item, self.selection.contains(item.node.id)),
                divider(),
            ]
        });
        scrollable(column(rows)).height(Length::Fill).into()
    }
}

/// What can be done with the selected files, instead of the listing's toolbar.
fn selection_bar<'a>(count: usize) -> Element<'a, Message> {
    container(
        row![
            text(format!("{count} selected"))
                .font(theme::MEDIUM)
                .style(theme::accent_text),
            small_primary(Icon::Restore, "Restore", Some(Message::Restore)),
            space::horizontal(),
            text(format!("Restored files go to {LOST_AND_FOUND}"))
                .size(SECOND_LINE_SIZE)
                .style(theme::muted_text),
        ]
        .spacing(SPACING)
        .align_y(Alignment::Center),
    )
    .padding(BAR_PADDING)
    .width(Length::Fill)
    .style(theme::selection_bar)
    .into()
}

fn item_row(item: &TrashItem, selected: bool) -> Element<'_, Message> {
    let node = &item.node;
    let coverage = if selected {
        Coverage::All
    } else {
        Coverage::None
    };
    let icon = node_icon(node);
    let tint = match icon {
        Icon::Folder => Tint::Accent,
        _ => Tint::Text,
    };
    let cells = row![
        check(coverage, Message::Toggle(node.id)),
        row![
            icon.view(ICON_SIZE, tint),
            ellipsized(node.name.as_str(), ROW_SIZE, Alignment::Start).width(Length::Fill)
        ]
        .spacing(NAME_GAP)
        .align_y(Alignment::Center)
        .width(Length::FillPortion(NAME_PORTION)),
        muted_cell(
            item.original_path.as_str(),
            Length::FillPortion(PATH_PORTION),
            Alignment::Start
        ),
        muted_cell(human_size(node.size), SIZE_WIDTH, Alignment::End),
        muted_cell(short_date(item.deleted_at), DATE_WIDTH, Alignment::End),
    ]
    .spacing(COLUMN_GAP)
    .height(Length::Fill)
    .align_y(Alignment::Center);
    button(cells)
        .width(Length::Fill)
        .height(ROW_HEIGHT)
        .padding(CHECK_LIST_PADDING_WIDE)
        .style(theme::row(selected))
        .on_press(Message::Click(node.id))
        .into()
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use iced_test::simulator;
    use protocol::NodeKind;

    use super::*;

    fn id(raw: u128) -> NodeId {
        NodeId(Uuid::from_u128(raw))
    }

    fn item(name: &str, raw: u128) -> TrashItem {
        TrashItem {
            node: Node {
                id: id(raw),
                parent_id: None,
                kind: NodeKind::File,
                name: name.to_owned(),
                size: 2048,
                sha256: None,
                mime: String::new(),
                version: 1,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
                edited_on: None,
            },
            original_path: format!("/Photos/{name}"),
            deleted_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn client() -> Client {
        Client::new("http://localhost")
    }

    fn loaded() -> State {
        let mut state = State::open();
        let items = vec![item("a.jpg", 1), item("b.jpg", 2)];
        let _ = state.update(&client(), Modifiers::default(), Message::Loaded(Ok(items)));
        state
    }

    fn press(state: &mut State, message: Message) -> Outcome {
        state.update(&client(), Modifiers::default(), message)
    }

    #[test]
    fn the_listing_shows_name_original_path_and_size() {
        let state = loaded();
        let mut ui = simulator(state.view());

        assert!(ui.find("a.jpg").is_ok());
        assert!(ui.find("/Photos/b.jpg").is_ok());
        assert!(ui.find("2.0 KB").is_ok());
        assert!(ui.find("ORIGINAL PATH").is_ok());
        assert!(ui.find("DELETED").is_ok());
    }

    #[test]
    fn an_empty_bin_says_so() {
        let state = State::open();
        let mut ui = simulator(state.view());

        assert!(ui.find("The recycle bin is empty.").is_ok());
    }

    #[test]
    fn restore_needs_a_selection() {
        let mut state = loaded();
        assert!(simulator(state.view()).find("Restore").is_err());

        let _ = press(&mut state, Message::Click(id(1)));
        let mut ui = simulator(state.view());
        ui.click("Restore").expect("restore is shown");
        let messages: Vec<_> = ui.into_messages().collect();
        assert!(matches!(messages.as_slice(), [Message::Restore]));
    }

    #[test]
    fn ctrl_click_extends_the_selection() {
        let mut state = loaded();
        let _ = press(&mut state, Message::Click(id(1)));

        let _ = state.update(&client(), Modifiers::COMMAND, Message::Click(id(2)));

        assert_eq!(state.selected_ids().len(), 2);
    }

    #[test]
    fn restoring_the_selection_sends_a_request() {
        let mut state = loaded();
        let _ = press(&mut state, Message::ToggleAll);

        let outcome = press(&mut state, Message::Restore);

        assert!(matches!(outcome, Outcome::Task(_)));
        assert_eq!(state.restoring.len(), 2);
    }

    #[test]
    fn clashes_are_handed_to_the_browser_with_the_requested_ids() {
        let mut state = loaded();
        let _ = press(&mut state, Message::Click(id(1)));
        let _ = press(&mut state, Message::Restore);
        let response = MoveResponse {
            conflicts: vec![protocol::Conflict {
                source: item("a.jpg", 1).node,
                path: "/Photos/a.jpg".to_owned(),
                existing: None,
            }],
            ..MoveResponse::default()
        };

        let outcome = press(&mut state, Message::Restored(Ok(Box::new(response))));

        let Outcome::Conflicts { ids, response } = outcome else {
            panic!("expected the conflicts");
        };
        assert_eq!(ids, [id(1)]);
        assert_eq!(response.conflicts.len(), 1);
    }

    #[test]
    fn a_finished_restore_is_reported_with_a_link_to_lost_and_found() {
        let mut state = loaded();
        let _ = press(&mut state, Message::Click(id(1)));
        let response = MoveResponse {
            moved: vec![item("a.jpg", 1).node, item("b.jpg", 2).node],
            target: Some(item("Lost & Found", 9).node),
            ..MoveResponse::default()
        };

        let _ = press(&mut state, Message::Restored(Ok(Box::new(response))));
        let mut ui = simulator(state.view());

        assert!(state.selected_ids().is_empty());
        assert!(ui.find("Restored 2 files to Lost & Found").is_ok());
        ui.click("Open Lost & Found").expect("link is shown");
        let messages: Vec<_> = ui.into_messages().collect();
        assert!(
            matches!(messages.as_slice(), [Message::OpenLostAndFound(folder)] if *folder == id(9))
        );
    }

    #[test]
    fn emptying_asks_to_confirm_with_the_number_of_files() {
        let mut state = loaded();

        let outcome = press(&mut state, Message::Empty);

        assert!(matches!(outcome, Outcome::ConfirmEmpty(2)));
    }

    #[test]
    fn an_empty_bin_has_nothing_to_empty() {
        let mut state = State::open();

        let outcome = press(&mut state, Message::Empty);

        assert!(matches!(outcome, Outcome::Task(_)));
    }

    #[test]
    fn files_that_left_the_bin_leave_the_selection() {
        let mut state = loaded();
        let _ = press(&mut state, Message::ToggleAll);

        let _ = press(&mut state, Message::Loaded(Ok(vec![item("a.jpg", 1)])));

        assert_eq!(state.selected_ids(), [id(1)]);
    }
}
