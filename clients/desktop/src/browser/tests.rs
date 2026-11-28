use protocol::{Node, SearchHit};
use time::OffsetDateTime;
use uuid::Uuid;

use iced::{Point, Vector};
use iced_test::simulator;

use client_core::labels::LOST_AND_FOUND;

use super::*;

const FOLDER_NAME: &str = "Folder B";
const FILE_NAME: &str = "report.pdf";
const OTHER_FILE_NAME: &str = "notes.txt";
const FOLDER_ID: u128 = 1;
const FILE_ID: u128 = 2;
const OTHER_FILE_ID: u128 = 3;
const FOUND_ID: u128 = 8;
const RESTORED_ID: u128 = 7;
const SEARCH_PLACEHOLDER_TEXT: &str = "Search files and folders";

fn id(raw: u128) -> NodeId {
    NodeId(Uuid::from_u128(raw))
}

fn node(name: &str, kind: NodeKind, raw: u128) -> Node {
    Node {
        id: id(raw),
        parent_id: None,
        kind,
        name: name.to_owned(),
        size: 2048,
        sha256: None,
        mime: String::new(),
        version: 1,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
        edited_on: None,
    }
}

fn state_with_listing() -> State {
    let me = Me {
        id: protocol::UserId(Uuid::nil()),
        username: "alice".to_owned(),
        lost_and_found_id: NodeId(Uuid::nil()),
        is_admin: false,
        external: false,
        second_factors: protocol::SecondFactors::default(),
    };
    let (mut state, _task) = State::open(Client::new("http://localhost"), me);
    let listing = Listing {
        path: Vec::new(),
        nodes: vec![
            node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID),
            node(FILE_NAME, NodeKind::File, FILE_ID),
            node(OTHER_FILE_NAME, NodeKind::File, OTHER_FILE_ID),
        ],
    };
    let _ = state.update(Message::Loaded(None, Ok(listing)));
    state
}

#[test]
fn files_returns_from_another_page_to_the_folder_it_left() {
    let mut state = state_with_listing();
    open_deep_path(&mut state, 2);
    let folder = state.folder.id;
    let _ = state.update(Message::OpenTrash);

    let _ = state.update(Message::OpenFiles);

    assert!(matches!(state.page, Page::Folder));
    assert_eq!(state.folder.id, folder);
}

#[test]
fn files_goes_home_when_already_on_the_files() {
    let mut state = state_with_listing();
    open_deep_path(&mut state, 2);

    let _ = state.update(Message::OpenFiles);

    assert_eq!(state.folder.id, None);
}

#[test]
fn the_account_menu_offers_sign_out() {
    let mut state = state_with_listing();
    assert!(simulator(state.view()).find("Sign out").is_err());

    let _ = state.update(Message::AccountMenu(Placement::cursor(Point::ORIGIN)));

    let mut ui = simulator(state.view());
    ui.click("Sign out").expect("the entry is shown");
    assert!(
        ui.into_messages()
            .any(|message| matches!(message, Message::SignOut))
    );
}

#[test]
fn the_sidebar_counts_what_failed_to_sync() {
    let mut state = state_with_listing();
    assert!(simulator(state.view()).find("1").is_err());

    let mut status = sync::Status::default();
    status.problems.push((
        PathBuf::from("/tmp/broken.txt"),
        sync::Fault::now("502", "Bad gateway"),
    ));
    let _ = state.update(Message::Sync(syncing::Message::Engine(
        sync::Update::Status(std::sync::Arc::new(status)),
    )));

    assert!(simulator(state.view()).find("1").is_ok());
}

#[test]
fn a_narrow_window_keeps_the_places_as_icons() {
    let mut state = state_with_listing();
    assert!(simulator(state.view()).find("Home Space").is_ok());

    let _ = state.update(Message::Resized(Size::new(600.0, 600.0)));

    assert!(simulator(state.view()).find("Home Space").is_err());
}

#[test]
fn the_selection_bar_replaces_the_toolbar_and_can_be_cleared() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Toggle(id(FILE_ID)));
    let _ = state.update(Message::Toggle(id(OTHER_FILE_ID)));

    let mut ui = simulator(state.view());
    assert!(ui.find("2 selected").is_ok());
    assert!(ui.find("Upload").is_err());
    ui.click("Clear selection").expect("the button is shown");
    assert!(
        ui.into_messages()
            .any(|message| matches!(message, Message::ClearSelection))
    );
}

#[test]
fn the_chip_says_where_the_folder_syncs() {
    let mut state = state_with_listing();
    assert!(simulator(state.view()).find("Syncs to /sync").is_err());

    let status = sync::Status {
        locations: std::collections::HashMap::from([(None, PathBuf::from("/sync"))]),
        ..sync::Status::default()
    };
    let update = sync::Update::Status(std::sync::Arc::new(status));
    let _ = state.update(Message::Sync(syncing::Message::Engine(update)));

    assert!(simulator(state.view()).find("Syncs to /sync").is_ok());
}

#[test]
fn the_sync_settings_are_a_page_that_escape_leaves() {
    let mut state = state_with_listing();
    let _ = state.update(Message::OpenSyncSettings);
    assert!(matches!(state.page, Page::SyncSettings));
    assert!(simulator(state.view()).find("Local folder").is_ok());

    let _ = state.update(Message::Shortcut(Shortcut::Escape));

    assert!(matches!(state.page, Page::Folder));
}

#[test]
fn the_status_card_opens_the_sync_settings() {
    let state = state_with_listing();
    let mut ui = simulator(state.view());

    ui.click("Sync").expect("the card is shown");

    assert!(
        ui.into_messages()
            .any(|message| matches!(message, Message::OpenSyncSettings))
    );
}

#[test]
fn the_search_shortcut_returns_to_the_files_where_the_box_is() {
    let mut state = state_with_listing();
    let _ = state.update(Message::OpenSyncQueue);
    assert!(
        simulator(state.view())
            .find(SEARCH_PLACEHOLDER_TEXT)
            .is_err()
    );

    let _ = state.update(Message::Shortcut(Shortcut::Search));

    assert!(matches!(state.page, Page::Folder));
}

#[test]
fn a_device_menu_opens_from_its_row_and_closes_when_it_is_used() {
    let mut state = state_with_listing();
    let _ = state.update(Message::OpenDevices);
    let device = Uuid::from_u128(7);

    let _ = state.update(Message::Devices(devices::Message::Menu(
        device,
        Placement::cursor(Point::ORIGIN),
    )));
    assert!(matches!(state.menu, Some(Menu::Device(id, _)) if id == device));
    assert!(simulator(state.view()).find("Rename").is_ok());

    let _ = state.update(Message::Devices(devices::Message::Rename(device)));
    assert!(state.menu.is_none());
}

#[test]
fn clicking_a_row_selects_it() {
    let state = state_with_listing();
    let mut ui = simulator(state.view());

    ui.click(FILE_NAME).expect("file row is shown");

    let messages: Vec<_> = ui.into_messages().collect();
    assert!(matches!(messages.as_slice(), [Message::Click(clicked)] if *clicked == id(FILE_ID)));
}

#[test]
fn clicking_a_folder_name_follows_its_link() {
    let state = state_with_listing();
    let mut ui = simulator(state.view());

    ui.click(FOLDER_NAME).expect("folder row is shown");

    let messages: Vec<_> = ui.into_messages().collect();
    assert!(
        matches!(messages.as_slice(), [Message::OpenFolderLink(clicked)] if *clicked == id(FOLDER_ID))
    );
}

/// What a left click at `position` on the browser sends.
fn click_at(state: &State, position: Point) -> Vec<Message> {
    let mut ui = simulator(state.view());
    ui.point_at(position);
    let _ = ui.simulate(simulator::click());
    ui.into_messages().collect()
}

#[test]
fn clicking_a_folder_icon_follows_its_link() {
    let state = state_with_listing();
    let name = simulator(state.view())
        .find(FOLDER_NAME)
        .expect("folder row is shown")
        .visible_bounds()
        .expect("folder name is in view");

    let messages = click_at(&state, Point::new(name.x - 16.0, name.center_y()));

    assert!(
        matches!(messages.as_slice(), [Message::OpenFolderLink(clicked)] if *clicked == id(FOLDER_ID))
    );
}

#[test]
fn clicking_right_of_a_folder_name_selects_the_row() {
    let state = state_with_listing();
    let name = simulator(state.view())
        .find(FOLDER_NAME)
        .expect("folder row is shown")
        .visible_bounds()
        .expect("folder name is in view");

    let messages = click_at(
        &state,
        Point::new(name.x + name.width + 40.0, name.center_y()),
    );

    assert!(matches!(messages.as_slice(), [Message::Click(clicked)] if *clicked == id(FOLDER_ID)));
}

/// A browser listing enough files to scroll, named so that they sort in the order of their ids.
fn state_with_long_listing() -> State {
    let mut state = state_with_listing();
    let nodes = (0..60)
        .map(|raw| node(&format!("file-{raw:02}"), NodeKind::File, 100 + raw))
        .collect();
    let listing = Listing {
        path: Vec::new(),
        nodes,
    };
    let _ = state.update(Message::Loaded(None, Ok(listing)));
    state
}

#[test]
fn rows_are_as_far_apart_as_the_scrolling_assumes() {
    let state = state_with_long_listing();
    let mut ui = simulator(state.view());
    let first = ui.find("file-00").expect("first row is shown");
    let second = ui.find("file-01").expect("second row is shown");

    let step =
        second.visible_bounds().expect("in view").y - first.visible_bounds().expect("in view").y;

    assert_eq!(step, parts::ROW_HEIGHT + parts::DIVIDER);
}

/// Scrolls the rows of `state` down with the wheel; returns what they report.
fn scrolled_viewport(state: &State) -> Viewport {
    let mut ui = simulator(state.view());
    let row = ui
        .find("file-00")
        .expect("first row is shown")
        .visible_bounds()
        .expect("in view");
    ui.point_at(row.center());
    let _ = ui.simulate([iced::Event::Mouse(iced::mouse::Event::WheelScrolled {
        delta: iced::mouse::ScrollDelta::Pixels { x: 0.0, y: -120.0 },
    })]);
    ui.into_messages()
        .find_map(|message| match message {
            Message::GridScrolled(viewport) => Some(viewport),
            _ => None,
        })
        .expect("the rows report their viewport")
}

#[test]
fn the_browser_learns_where_the_rows_are_scrolled() {
    let viewport = scrolled_viewport(&state_with_long_listing());

    assert_eq!(viewport.absolute_offset().y, 120.0);
    assert!(viewport.bounds().height > 0.0);
}

#[test]
fn opening_a_folder_forgets_where_the_rows_were_scrolled() {
    let mut state = state_with_long_listing();
    let viewport = scrolled_viewport(&state);
    let _ = state.update(Message::GridScrolled(viewport));
    assert!(state.folder.viewport.is_some());

    let _ = state.update(Message::Jump(Some(id(FOLDER_ID))));

    assert!(state.folder.viewport.is_none());
}

#[test]
fn a_folder_link_opens_the_folder() {
    let mut state = state_with_listing();

    let _ = state.update(Message::OpenFolderLink(id(FOLDER_ID)));

    assert_eq!(state.folder.id, Some(id(FOLDER_ID)));
}

#[test]
fn a_folder_link_with_a_modifier_selects_the_folder() {
    for modifiers in [Modifiers::COMMAND, Modifiers::SHIFT] {
        let mut state = state_with_listing();
        let _ = state.update(Message::Modifiers(modifiers));

        let _ = state.update(Message::OpenFolderLink(id(FOLDER_ID)));

        assert_eq!(state.folder.id, None);
        assert!(state.folder.selection.contains(id(FOLDER_ID)));
    }
}

#[test]
fn the_second_click_of_a_double_click_stays_in_the_opened_folder() {
    let mut state = state_with_listing();
    let _ = state.update(Message::OpenFolderLink(id(FOLDER_ID)));
    let inner = Listing {
        path: vec![node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID)],
        nodes: vec![node("Inner", NodeKind::Folder, OTHER_FILE_ID)],
    };
    let _ = state.update(Message::Loaded(Some(id(FOLDER_ID)), Ok(inner)));

    let _ = state.update(Message::OpenFolderLink(id(OTHER_FILE_ID)));

    assert_eq!(state.folder.id, Some(id(FOLDER_ID)));
}

#[test]
fn arrow_keys_move_the_selection() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));

    let _ = state.update(Message::Shortcut(Shortcut::SelectNext));
    assert!(state.folder.selection.contains(id(FILE_ID)));
    assert_eq!(state.folder.selected().len(), 1);

    let _ = state.update(Message::Shortcut(Shortcut::SelectPrevious));
    assert!(state.folder.selection.contains(id(OTHER_FILE_ID)));
    assert_eq!(state.folder.selected().len(), 1);
}

#[test]
fn shift_arrows_extend_and_shrink_the_selection() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Click(id(FOLDER_ID)));

    let _ = state.update(Message::Shortcut(Shortcut::ExtendNext));
    let _ = state.update(Message::Shortcut(Shortcut::ExtendNext));
    assert_eq!(state.folder.selected().len(), 3);

    let _ = state.update(Message::Shortcut(Shortcut::ExtendPrevious));
    let _ = state.update(Message::Shortcut(Shortcut::ExtendPrevious));
    let _ = state.update(Message::Shortcut(Shortcut::ExtendPrevious));
    assert_eq!(state.folder.selected().len(), 1);
    assert!(state.folder.selection.contains(id(FOLDER_ID)));
}

#[test]
fn space_opens_the_selected_folder() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Click(id(FOLDER_ID)));

    let _ = state.update(Message::Shortcut(Shortcut::Open));

    assert_eq!(state.folder.id, Some(id(FOLDER_ID)));
}

#[test]
fn the_open_details_panel_follows_the_arrow_keys() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));
    let _ = state.update(Message::Act(Action::Details));

    let _ = state.update(Message::Shortcut(Shortcut::SelectNext));

    assert_eq!(
        state.details.as_ref().map(details::State::node_id),
        Some(id(FILE_ID))
    );
}

#[test]
fn a_click_selects_only_that_row() {
    let mut state = state_with_listing();

    let _ = state.update(Message::Click(id(FILE_ID)));

    assert_eq!(state.folder.selected().len(), 1);
    assert!(state.folder.selection.contains(id(FILE_ID)));
}

#[test]
fn the_open_details_panel_follows_the_selected_row() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Click(id(FILE_ID)));
    let _ = state.update(Message::Act(Action::Details));
    assert_eq!(
        state.details.as_ref().map(details::State::node_id),
        Some(id(FILE_ID))
    );

    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));

    assert_eq!(
        state.details.as_ref().map(details::State::node_id),
        Some(id(OTHER_FILE_ID))
    );
}

#[test]
fn the_details_panel_opens_its_folder() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Click(id(FOLDER_ID)));
    let _ = state.update(Message::Act(Action::Details));

    let _ = state.update(Message::Details(details::Message::Open));

    assert_eq!(state.folder.id, Some(id(FOLDER_ID)));
    assert!(state.details.is_none());
}

#[test]
fn selecting_a_row_does_not_open_a_closed_details_panel() {
    let mut state = state_with_listing();

    let _ = state.update(Message::Click(id(FILE_ID)));

    assert!(state.details.is_none());
}

#[test]
fn double_clicking_a_folder_opens_it() {
    let mut state = state_with_listing();

    let _ = state.update(Message::Click(id(FOLDER_ID)));
    let _ = state.update(Message::Click(id(FOLDER_ID)));

    assert_eq!(state.folder.id, Some(id(FOLDER_ID)));
    assert!(state.folder.loading);
}

#[test]
fn rename_is_disabled_for_two_selected_items() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Toggle(id(FILE_ID)));
    let _ = state.update(Message::Toggle(id(OTHER_FILE_ID)));
    let _ = state.update(Message::SelectionMenu(Placement::cursor(Point::ORIGIN)));
    let mut ui = simulator(state.view());

    ui.click("Rename").expect("rename is shown");

    // The press may reach the backdrop, which closes the menu, but never renames.
    assert!(
        ui.into_messages()
            .all(|message| !matches!(message, Message::Act(Action::Rename)))
    );
}

#[test]
fn several_selected_items_keep_their_actions_in_the_count_menu() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Toggle(id(FILE_ID)));
    let _ = state.update(Message::Toggle(id(OTHER_FILE_ID)));
    let mut ui = simulator(state.view());

    assert!(ui.find("2 selected").is_ok());
    assert!(ui.find("Download").is_err());
}

fn inside_lost_and_found() -> State {
    let mut state = state_with_listing();
    state.me.lost_and_found_id = id(FOLDER_ID);
    state.folder.id = Some(id(FOLDER_ID));
    state
}

/// A browser whose Lost & Found holds a restored file, showing Home.
fn with_restored_file() -> State {
    let mut state = state_with_listing();
    state.me.lost_and_found_id = id(FOUND_ID);
    state.lost_and_found = LostAndFound::new(id(FOUND_ID));
    let mut restored = node(FILE_NAME, NodeKind::File, RESTORED_ID);
    restored.parent_id = Some(id(FOUND_ID));
    let _ = state.update(Message::LostAndFoundLoaded(Ok(vec![restored])));
    state
}

#[test]
fn the_sidebar_offers_lost_and_found_only_with_restored_files() {
    let mut empty = state_with_listing();
    assert!(simulator(empty.view()).find(LOST_AND_FOUND).is_err());
    let _ = empty.update(Message::LostAndFoundLoaded(Ok(Vec::new())));
    assert!(simulator(empty.view()).find(LOST_AND_FOUND).is_err());

    let state = with_restored_file();
    let mut ui = simulator(state.view());
    ui.click(LOST_AND_FOUND).expect("the place is shown");
    assert!(
        ui.into_messages()
            .any(|message| matches!(message, Message::OpenLostAndFound))
    );
}

#[test]
fn the_sidebar_drops_lost_and_found_when_its_last_file_leaves() {
    let mut state = with_restored_file();
    let mut moved_out = node(FILE_NAME, NodeKind::File, RESTORED_ID);
    moved_out.parent_id = None;

    let _ = state.update(Message::Server(protocol::Event::NodeUpdated {
        node: moved_out,
    }));

    assert!(simulator(state.view()).find(LOST_AND_FOUND).is_err());
}

#[test]
fn lost_and_found_opens_from_the_sidebar_and_goes_back() {
    let mut state = with_restored_file();

    let _ = state.update(Message::OpenLostAndFound);
    assert_eq!(state.folder.id, Some(id(FOUND_ID)));

    let _ = state.update(Message::Shortcut(Shortcut::Back));
    assert_eq!(state.folder.id, None);
}

#[test]
fn files_goes_home_from_lost_and_found_even_on_another_page() {
    let mut state = with_restored_file();
    let _ = state.update(Message::OpenLostAndFound);
    let _ = state.update(Message::OpenTrash);

    let _ = state.update(Message::OpenFiles);

    assert!(matches!(state.page, Page::Folder));
    assert_eq!(state.folder.id, None);
}

#[test]
fn inside_a_folder_of_lost_and_found_counts_as_within_it() {
    let mut state = with_restored_file();
    let inner = id(FOLDER_ID);
    let _ = state.update(Message::Jump(Some(inner)));
    assert!(!state.within_lost_and_found());

    let listing = Listing {
        path: vec![
            node(LOST_AND_FOUND, NodeKind::Folder, FOUND_ID),
            node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID),
        ],
        nodes: Vec::new(),
    };
    let _ = state.update(Message::Loaded(Some(inner), Ok(listing)));

    assert!(state.within_lost_and_found());
}

#[test]
fn home_never_lists_lost_and_found() {
    let client = Client::new("http://localhost");
    let me = Me {
        id: protocol::UserId(Uuid::nil()),
        username: "alice".to_owned(),
        lost_and_found_id: id(FOUND_ID),
        is_admin: false,
        external: false,
        second_factors: protocol::SecondFactors::default(),
    };
    let (mut state, _task) = State::open(client, me);
    let found = node(LOST_AND_FOUND, NodeKind::Folder, FOUND_ID);

    let listing = Listing {
        path: Vec::new(),
        nodes: vec![
            found.clone(),
            node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID),
        ],
    };
    let _ = state.update(Message::Loaded(None, Ok(listing)));
    assert!(state.folder.node(id(FOUND_ID)).is_none());
    assert!(state.folder.node(id(FOLDER_ID)).is_some());

    let _ = state.update(Message::Server(protocol::Event::NodeUpdated {
        node: found,
    }));
    assert!(state.folder.node(id(FOUND_ID)).is_none());
}

#[test]
fn nothing_can_be_created_inside_lost_and_found() {
    let mut state = inside_lost_and_found();

    let _ = state.update(Message::NewFolder);
    let _ = state.update(Message::Shortcut(Shortcut::NewFolder));
    let _ = state.update(Message::Upload);
    let _ = state.update(Message::FileHovered);

    assert!(state.dialog.is_none());
    assert!(!state.dropping);
}

#[test]
fn folders_and_uploads_still_work_elsewhere() {
    let mut creating = state_with_listing();
    let _ = creating.update(Message::NewFolder);
    assert!(creating.dialog.is_some());

    let mut hovered = state_with_listing();
    let _ = hovered.update(Message::FileHovered);
    assert!(hovered.dropping);
}

#[test]
fn delete_does_nothing_for_lost_and_found() {
    let mut state = state_with_listing();
    state.me.lost_and_found_id = id(FOLDER_ID);
    let _ = state.update(Message::Toggle(id(FOLDER_ID)));
    let _ = state.update(Message::Toggle(id(FILE_ID)));

    let _ = state.update(Message::Shortcut(Shortcut::Delete));
    let _ = state.update(Message::Act(Action::Delete));

    assert!(state.dialog.is_none());
}

#[test]
fn context_menu_delete_opens_the_delete_dialog() {
    let mut state = state_with_listing();

    let _ = state.update(Message::ContextMenu(
        id(FILE_ID),
        Placement::cursor(Point::ORIGIN),
    ));
    let _ = state.update(Message::Act(Action::Delete));

    assert!(state.menu.is_none());
    assert!(state.dialog.is_some());
}

#[test]
fn escape_closes_the_dialog() {
    let mut state = state_with_listing();
    let _ = state.update(Message::NewFolder);

    let _ = state.update(Message::Shortcut(Shortcut::Escape));

    assert!(state.dialog.is_none());
}

#[test]
fn creating_a_folder_with_an_empty_name_closes_the_dialog() {
    let mut state = state_with_listing();
    let _ = state.update(Message::NewFolder);

    let _ = state.update(Message::Dialog(dialogs::Message::Submit));

    assert!(state.dialog.is_none());
}

#[test]
fn backspace_goes_to_the_parent_folder() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Jump(Some(id(FOLDER_ID))));
    let listing = Listing {
        path: vec![node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID)],
        nodes: Vec::new(),
    };
    let _ = state.update(Message::Loaded(Some(id(FOLDER_ID)), Ok(listing)));

    let _ = state.update(Message::Shortcut(Shortcut::Back));

    assert_eq!(state.folder.id, None);
}

#[test]
fn back_after_a_breadcrumb_jump_returns_to_the_folder_it_left() {
    let mut state = state_with_listing();
    open_deep_path(&mut state, 3);
    let deep = state.folder.id;

    let _ = state.update(Message::Jump(None));
    assert_eq!(state.folder.id, None);
    let _ = state.update(Message::Shortcut(Shortcut::Back));

    assert_eq!(state.folder.id, deep);
    // The jump is spent: the next back goes to the parent.
    let _ = state.update(Message::Loaded(
        deep,
        Ok(Listing {
            path: (1..=3)
                .map(|raw| node(&format!("level-{raw}"), NodeKind::Folder, 100 + raw))
                .collect(),
            nodes: Vec::new(),
        }),
    ));
    let _ = state.update(Message::Shortcut(Shortcut::Back));
    assert_eq!(state.folder.id, Some(id(102)));
}

fn open_deep_path(state: &mut State, depth: u128) {
    let path: Vec<Node> = (1..=depth)
        .map(|raw| node(&format!("level-{raw}"), NodeKind::Folder, 100 + raw))
        .collect();
    let current = path.last().map(|folder| folder.id);
    let _ = state.update(Message::Jump(current));
    let listing = Listing {
        path,
        nodes: Vec::new(),
    };
    let _ = state.update(Message::Loaded(current, Ok(listing)));
}

#[test]
fn deep_breadcrumbs_hide_the_middle_folders() {
    let mut state = state_with_listing();
    open_deep_path(&mut state, 5);
    let mut ui = simulator(state.view());

    assert!(ui.find("level-1").is_ok());
    assert!(ui.find("level-5").is_ok());
    assert!(ui.find("level-3").is_err());
    assert_eq!(state.folder.hidden_crumbs().len(), 3);
}

const MENU_AT: Vector = Vector::new(200.0, 150.0);

#[test]
fn an_entry_of_a_context_menu_moved_into_place_can_be_clicked() {
    let mut state = state_with_listing();
    let _ = state.update(Message::ContextMenu(
        id(FOLDER_ID),
        Placement::cursor(Point::ORIGIN + MENU_AT),
    ));
    let mut ui = simulator(state.view());
    // The simulator finds a floating widget where it was laid out, before it was moved.
    let entry = ui.find("Details").expect("the entry is shown");
    let laid_out = entry.visible_bounds().expect("the entry is visible");

    ui.point_at(laid_out.center() + MENU_AT);
    let _ = ui.simulate(simulator::click());

    assert!(
        ui.into_messages()
            .any(|message| matches!(message, Message::Act(Action::Details)))
    );
}

#[test]
fn the_overflow_crumb_opens_the_hidden_folders_menu() {
    let mut state = state_with_listing();
    open_deep_path(&mut state, 4);

    let _ = state.update(Message::CrumbsMenu(Placement::cursor(Point::ORIGIN)));
    let mut ui = simulator(state.view());

    assert!(matches!(state.menu, Some(Menu::Crumbs(_))));
    assert!(ui.find("level-2").is_ok());
}

#[test]
fn a_created_node_in_the_open_folder_appears() {
    let mut state = state_with_listing();
    let created = node("added.txt", NodeKind::File, 9);

    let update = state.update(Message::Server(protocol::Event::NodeCreated {
        node: created,
    }));

    assert!(matches!(update, Update::Task(task) if task.units() == 0));
    assert!(
        state
            .folder
            .listing
            .nodes
            .iter()
            .any(|node| node.id == id(9))
    );
}

#[test]
fn a_deleted_selected_node_leaves_the_selection() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Click(id(FILE_ID)));

    let _ = state.update(Message::Server(protocol::Event::NodeDeleted {
        id: id(FILE_ID),
        parent_id: None,
    }));

    assert!(state.folder.selected().is_empty());
}

fn hit_for(node: Node) -> SearchHit {
    SearchHit {
        node,
        location: Vec::new(),
    }
}

fn searching(state: &mut State, query: &str) -> u64 {
    let _ = state.update(Message::Search(search::Message::Changed(query.to_owned())));
    state.search.seq
}

#[test]
fn typing_a_query_schedules_a_debounced_search() {
    let mut state = state_with_listing();

    let update = state.update(Message::Search(search::Message::Changed("rep".to_owned())));

    assert!(matches!(update, Update::Task(task) if task.units() > 0));
    assert!(state.search.is_active());
}

#[test]
fn a_blank_query_schedules_nothing() {
    let mut state = state_with_listing();

    let update = state.update(Message::Search(search::Message::Changed("  ".to_owned())));

    assert!(matches!(update, Update::Task(task) if task.units() == 0));
}

#[test]
fn results_of_an_outdated_query_are_ignored() {
    let mut state = state_with_listing();
    let stale = searching(&mut state, "a");
    searching(&mut state, "ab");

    let _ = state.update(Message::Search(search::Message::Loaded(
        stale,
        Ok(vec![hit_for(node(FILE_NAME, NodeKind::File, FILE_ID))]),
    )));

    assert!(state.search.hit(0).is_none());
}

#[test]
fn results_of_the_current_query_are_shown() {
    let mut state = state_with_listing();
    let seq = searching(&mut state, "rep");

    let _ = state.update(Message::Search(search::Message::Loaded(
        seq,
        Ok(vec![hit_for(node(FILE_NAME, NodeKind::File, FILE_ID))]),
    )));
    let mut ui = simulator(state.view());

    assert!(ui.find("1 result").is_ok());
    assert!(ui.find("rep").is_ok());
    assert!(ui.find("ort.pdf").is_ok());
}

#[test]
fn opening_a_folder_hit_leaves_the_search() {
    let mut state = state_with_listing();
    let seq = searching(&mut state, "folder");
    let _ = state.update(Message::Search(search::Message::Loaded(
        seq,
        Ok(vec![hit_for(node(
            FOLDER_NAME,
            NodeKind::Folder,
            FOLDER_ID,
        ))]),
    )));

    let _ = state.update(Message::Search(search::Message::Open(0)));

    assert_eq!(state.folder.id, Some(id(FOLDER_ID)));
    assert!(!state.search.is_active());
}

#[test]
fn escape_clears_an_active_search() {
    let mut state = state_with_listing();
    searching(&mut state, "rep");

    let _ = state.update(Message::Shortcut(Shortcut::Escape));

    assert!(!state.search.is_active());
}

#[test]
fn folder_shortcuts_do_nothing_while_searching() {
    let mut state = state_with_listing();
    let _ = state.update(Message::Click(id(FILE_ID)));
    searching(&mut state, "rep");

    let _ = state.update(Message::Shortcut(Shortcut::Delete));

    assert!(state.dialog.is_none());
}

#[test]
fn a_resync_reloads_the_listing() {
    let mut state = state_with_listing();

    let update = state.update(Message::Server(protocol::Event::Resync));

    assert!(matches!(update, Update::Task(task) if task.units() > 0));
}

#[test]
fn the_sidebar_opens_the_recycle_bin() {
    let state = state_with_listing();
    let mut ui = simulator(state.view());

    ui.click("Recycle bin").expect("the entry is shown");

    let messages: Vec<_> = ui.into_messages().collect();
    assert!(matches!(messages.as_slice(), [Message::OpenTrash]));
}

#[test]
fn opening_the_recycle_bin_loads_it() {
    let mut state = state_with_listing();

    let update = state.update(Message::OpenTrash);

    assert!(matches!(update, Update::Task(task) if task.units() > 0));
    assert!(matches!(state.page, Page::Trash(_)));
}

#[test]
fn the_sidebar_opens_what_is_shared_with_the_user() {
    let state = state_with_listing();
    let mut ui = simulator(state.view());

    ui.click(client_core::received::SHARED_WITH_ME)
        .expect("the entry is shown");

    let messages: Vec<_> = ui.into_messages().collect();
    assert!(matches!(messages.as_slice(), [Message::OpenShared]));
}

#[test]
fn opening_shared_with_me_loads_it() {
    let mut state = state_with_listing();

    let update = state.update(Message::OpenShared);

    assert!(matches!(update, Update::Task(task) if task.units() > 0));
    assert!(matches!(state.page, Page::Shared(_)));
}

#[test]
fn a_trash_change_reloads_the_open_bin_only() {
    let mut state = state_with_listing();
    let update = state.update(Message::Server(protocol::Event::TrashChanged));
    assert!(matches!(update, Update::Task(task) if task.units() == 0));

    let _ = state.update(Message::OpenTrash);
    let update = state.update(Message::Server(protocol::Event::TrashChanged));
    assert!(matches!(update, Update::Task(task) if task.units() > 0));
}

#[test]
fn opening_a_folder_leaves_the_recycle_bin() {
    let mut state = state_with_listing();
    let _ = state.update(Message::OpenTrash);

    let _ = state.update(Message::Jump(None));

    assert!(matches!(state.page, Page::Folder));
}

#[test]
fn restore_clashes_open_the_conflicts_dialog() {
    let mut state = state_with_listing();
    let _ = state.update(Message::OpenTrash);
    let response = protocol::MoveResponse {
        conflicts: vec![protocol::Conflict {
            source: node(FILE_NAME, NodeKind::File, FILE_ID),
            path: format!("/{FILE_NAME}"),
            existing: None,
        }],
        ..protocol::MoveResponse::default()
    };

    let _ = state.update(Message::Trash(trash::Message::Restored(Ok(Box::new(
        response,
    )))));

    assert!(state.dialog.is_some());
}

fn with_queue(state: &mut State, queue: Vec<sync::Item>) {
    let status = sync::Status {
        queue,
        ..sync::Status::default()
    };
    let update = sync::Update::Status(std::sync::Arc::new(status));
    let _ = state.update(Message::Sync(syncing::Message::Engine(update)));
}

fn waiting_upload(name: &str) -> sync::Item {
    use crate::sync::{Item, ItemState, Operation};
    Item::test(
        PathBuf::from("/sync").join(name),
        Operation::Upload,
        10,
        0,
        ItemState::Queued,
    )
}

#[test]
fn the_sidebar_opens_the_devices() {
    let state = state_with_listing();
    let mut ui = simulator(state.view());

    ui.click("Devices").expect("the entry is shown");

    let messages: Vec<_> = ui.into_messages().collect();
    assert!(matches!(messages.as_slice(), [Message::OpenDevices]));
}

#[test]
fn opening_the_devices_loads_them_and_escape_leaves() {
    let mut state = state_with_listing();

    let update = state.update(Message::OpenDevices);
    assert!(matches!(update, Update::Task(task) if task.units() > 0));
    assert!(matches!(state.page, Page::Devices(_)));

    let _ = state.update(Message::Shortcut(Shortcut::Escape));
    assert!(matches!(state.page, Page::Folder));
}

#[test]
fn the_sidebar_opens_the_sync_queue() {
    let state = state_with_listing();
    let mut ui = simulator(state.view());

    ui.click("Sync queue").expect("the entry is shown");

    let messages: Vec<_> = ui.into_messages().collect();
    assert!(matches!(messages.as_slice(), [Message::OpenSyncQueue]));
}

#[test]
fn the_sync_queue_page_lists_the_work_ahead_and_back_leaves_it() {
    let mut state = state_with_listing();
    with_queue(&mut state, vec![waiting_upload("new.txt")]);

    let _ = state.update(Message::OpenSyncQueue);
    assert!(matches!(state.page, Page::SyncQueue));
    {
        let mut ui = simulator(state.view());
        assert!(ui.find("Waiting (1)").is_ok());
        assert!(ui.find("new.txt").is_ok());
    }

    let _ = state.update(Message::Shortcut(Shortcut::Back));
    assert!(matches!(state.page, Page::Folder));
}

#[test]
fn a_new_local_file_waiting_to_upload_is_listed_but_inert() {
    let mut state = state_with_listing();
    with_queue(&mut state, vec![waiting_upload("new.txt")]);

    let mut ui = simulator(state.view());
    assert!(ui.find("new.txt").is_ok());
    assert!(ui.find("pending sync").is_ok());
    ui.click("new.txt").expect("the row is shown");
    assert_eq!(ui.into_messages().count(), 0);
}

#[test]
fn a_waiting_file_is_not_listed_twice_once_the_server_lists_it() {
    let mut state = state_with_listing();
    with_queue(&mut state, vec![waiting_upload(FILE_NAME)]);

    let mut ui = simulator(state.view());
    assert!(ui.find("pending sync").is_err());
}

#[test]
fn an_edited_file_stays_a_normal_row() {
    let mut state = state_with_listing();
    let edited = sync::Item {
        node: Some(id(FILE_ID)),
        ..waiting_upload(FILE_NAME)
    };
    with_queue(&mut state, vec![edited]);

    let mut ui = simulator(state.view());
    assert!(ui.find("pending sync").is_err());
    ui.click(FILE_NAME).expect("the row is shown");
    let messages: Vec<_> = ui.into_messages().collect();
    assert!(matches!(messages.as_slice(), [Message::Click(_)]));
}

#[test]
fn a_waiting_file_in_another_folder_is_not_listed_here() {
    let mut state = state_with_listing();
    let elsewhere = sync::Item {
        parent: Some(Some(id(FOLDER_ID))),
        ..waiting_upload("new.txt")
    };
    with_queue(&mut state, vec![elsewhere]);

    let mut ui = simulator(state.view());
    assert!(ui.find("new.txt").is_err());
}

fn with_synced_file(state: &mut State, raw: u128, path: PathBuf) {
    let status = sync::Status {
        files: std::collections::HashMap::from([(id(raw), path)]),
        ..sync::Status::default()
    };
    let update = sync::Update::Status(std::sync::Arc::new(status));
    let _ = state.update(Message::Sync(syncing::Message::Engine(update)));
}

#[test]
fn a_synced_file_on_disk_has_a_local_copy() {
    let mut state = state_with_listing();
    let dir = std::env::temp_dir().join(format!("home-space-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir is created");
    let path = dir.join(FILE_NAME);
    std::fs::write(&path, b"pdf").expect("temp file is written");
    with_synced_file(&mut state, FILE_ID, path.clone());

    let file = state
        .folder
        .node(id(FILE_ID))
        .cloned()
        .expect("file is listed");
    let local = state.local_copy(&file);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(local, Some(path));
}

#[test]
fn a_synced_file_not_yet_on_disk_has_no_local_copy() {
    let mut state = state_with_listing();
    let path = std::env::temp_dir().join(format!("home-space-missing-{}", Uuid::new_v4()));
    with_synced_file(&mut state, FILE_ID, path);

    let file = state
        .folder
        .node(id(FILE_ID))
        .cloned()
        .expect("file is listed");

    assert_eq!(state.local_copy(&file), None);
}

#[test]
fn an_unsynced_file_has_no_local_copy() {
    let state = state_with_listing();
    let file = state
        .folder
        .node(id(FILE_ID))
        .cloned()
        .expect("file is listed");

    assert_eq!(state.local_copy(&file), None);
}

#[test]
fn double_clicking_an_unsynced_file_stays_in_the_folder() {
    let mut state = state_with_listing();

    let _ = state.update(Message::Click(id(FILE_ID)));
    let _ = state.update(Message::Click(id(FILE_ID)));

    assert_eq!(state.folder.id, None);
    assert!(state.folder.error.is_none());
}

#[test]
fn the_context_menu_of_a_file_offers_open() {
    let mut state = state_with_listing();
    let _ = state.update(Message::ContextMenu(
        id(FILE_ID),
        Placement::cursor(Point::ORIGIN),
    ));
    let mut ui = simulator(state.view());

    ui.click("Open").expect("open is shown");

    let messages: Vec<_> = ui.into_messages().collect();
    assert!(matches!(
        messages.as_slice(),
        [Message::Act(Action::OpenFile)]
    ));
}

#[tokio::test]
async fn a_session_the_server_ended_returns_to_the_login() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    let server = MockServer::start().await;
    let tokens = serde_json::json!({
        "access_token": "access", "refresh_token": "refresh", "expires_in": 900
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": Uuid::nil(), "username": "alice", "lost_and_found_id": Uuid::nil()
        })))
        .mount(&server)
        .await;
    // Both the access token and the refresh token have expired.
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let client = Client::new(server.uri());
    let me = client.login("alice", "pw").await.expect("login");
    let (mut state, _task) = State::open(client.clone(), me);

    let result = client.list(None).await.map(|nodes| Listing {
        path: Vec::new(),
        nodes,
    });
    assert_eq!(result.as_ref().err(), Some(&ApiError::Unauthorized));
    let update = state.update(Message::Loaded(None, result));

    assert!(matches!(update, Update::SignedOut { server: url } if url == server.uri()));
}

fn state_with_text_file() -> State {
    let mut state = state_with_listing();
    let text = Node {
        mime: "text/plain".to_owned(),
        ..node(OTHER_FILE_NAME, NodeKind::File, OTHER_FILE_ID)
    };
    let listing = Listing {
        path: Vec::new(),
        nodes: vec![node(FILE_NAME, NodeKind::File, FILE_ID), text],
    };
    let _ = state.update(Message::Loaded(None, Ok(listing)));
    state
}

#[test]
fn double_clicking_a_text_file_opens_the_viewer() {
    let mut state = state_with_text_file();

    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));

    assert!(matches!(&state.page, Page::Viewer(viewer) if viewer.node_id() == id(OTHER_FILE_ID)));
}

#[test]
fn escape_leaves_the_viewer() {
    let mut state = state_with_text_file();
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));

    let _ = state.update(Message::Shortcut(Shortcut::Escape));

    assert!(matches!(state.page, Page::Folder));
}

/// The browser with the text file open in the viewer, its content loaded and being edited.
fn state_editing_text(changed: bool) -> State {
    let mut state = state_with_text_file();
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));
    let text = Node {
        mime: "text/plain".to_owned(),
        ..node(OTHER_FILE_NAME, NodeKind::File, OTHER_FILE_ID)
    };
    let _ = state.update(Message::Viewer(viewer::Message::Loaded(Ok(
        viewer::Loaded::new(text, b"hello"),
    ))));
    let _ = state.update(Message::Viewer(viewer::Message::Edit));
    if changed {
        let _ = state.update(Message::Viewer(viewer::Message::typed('x')));
    }
    state
}

#[test]
fn leaving_unsaved_edits_asks_first() {
    let mut state = state_editing_text(true);

    let _ = state.update(Message::Shortcut(Shortcut::Escape));

    assert!(matches!(state.page, Page::Viewer(_)));
    assert!(state.dialog.is_some());
}

#[test]
fn the_sidebar_asks_before_leaving_unsaved_edits() {
    let mut state = state_editing_text(true);

    let _ = state.update(Message::OpenTrash);

    assert!(matches!(state.page, Page::Viewer(_)));
    assert!(state.dialog.is_some());
}

#[test]
fn discarding_unsaved_edits_goes_where_the_user_went() {
    let mut state = state_editing_text(true);
    let _ = state.update(Message::OpenTrash);

    let _ = state.update(Message::Dialog(dialogs::Message::Submit));

    assert!(matches!(state.page, Page::Trash(_)));
    assert!(state.dialog.is_none());
}

#[test]
fn keeping_unsaved_edits_stays_in_the_editor() {
    let mut state = state_editing_text(true);
    let _ = state.update(Message::Shortcut(Shortcut::Escape));

    let _ = state.update(Message::Dialog(dialogs::Message::Cancel));

    assert!(matches!(&state.page, Page::Viewer(viewer) if viewer.has_unsaved_edits()));
    assert!(state.dialog.is_none());
}

#[test]
fn leaving_the_editor_without_changes_does_not_ask() {
    let mut state = state_editing_text(false);

    let _ = state.update(Message::Shortcut(Shortcut::Escape));

    assert!(matches!(state.page, Page::Folder));
    assert!(state.dialog.is_none());
}

#[test]
fn the_viewer_closes_when_its_file_is_deleted() {
    let mut state = state_with_text_file();
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));
    let _ = state.update(Message::Click(id(OTHER_FILE_ID)));

    let _ = state.update(Message::Server(protocol::Event::NodeDeleted {
        id: id(OTHER_FILE_ID),
        parent_id: None,
    }));

    assert!(matches!(state.page, Page::Folder));
}

#[test]
fn the_context_menu_of_a_text_file_offers_the_system_app() {
    let mut state = state_with_text_file();
    let _ = state.update(Message::ContextMenu(
        id(OTHER_FILE_ID),
        Placement::cursor(Point::ORIGIN),
    ));
    let mut ui = simulator(state.view());

    ui.click("Open with system app")
        .expect("the entry is shown");

    let messages: Vec<_> = ui.into_messages().collect();
    assert!(matches!(
        messages.as_slice(),
        [Message::Act(Action::OpenWith)]
    ));
}

#[test]
fn the_context_menu_of_a_file_without_a_viewer_leaves_out_the_system_app() {
    let mut state = state_with_listing();
    let _ = state.update(Message::ContextMenu(
        id(FILE_ID),
        Placement::cursor(Point::ORIGIN),
    ));

    assert!(
        simulator(state.view())
            .find("Open with system app")
            .is_err()
    );
}

const IMAGE_IDS: [u128; 3] = [41, 42, 43];

fn image(name: &str, raw: u128) -> Node {
    Node {
        mime: "image/png".to_owned(),
        ..node(name, NodeKind::File, raw)
    }
}

/// A folder of three images and a text file, with the image `raw` open in the viewer.
fn viewing_image(raw: u128) -> State {
    let mut state = state_with_listing();
    let listing = Listing {
        path: Vec::new(),
        nodes: vec![
            image("a.png", IMAGE_IDS[0]),
            Node {
                mime: "text/plain".to_owned(),
                ..node("b.txt", NodeKind::File, 50)
            },
            image("c.png", IMAGE_IDS[1]),
            image("d.png", IMAGE_IDS[2]),
        ],
    };
    let _ = state.update(Message::Loaded(None, Ok(listing)));
    let _ = state.update(Message::Click(id(raw)));
    let _ = state.update(Message::Shortcut(Shortcut::Activate));
    state
}

fn viewed(state: &State) -> Option<NodeId> {
    match &state.page {
        Page::Viewer(viewer) => Some(viewer.node_id()),
        _ => None,
    }
}

#[test]
fn an_image_opens_in_the_viewer() {
    let state = viewing_image(IMAGE_IDS[0]);
    assert_eq!(viewed(&state), Some(id(IMAGE_IDS[0])));
}

#[test]
fn the_right_arrow_shows_the_next_image_skipping_other_files() {
    let mut state = viewing_image(IMAGE_IDS[0]);
    let _ = state.update(Message::Shortcut(Shortcut::Next));
    assert_eq!(viewed(&state), Some(id(IMAGE_IDS[1])));
}

#[test]
fn space_shows_the_next_image() {
    let mut state = viewing_image(IMAGE_IDS[1]);
    let _ = state.update(Message::Shortcut(Shortcut::Open));
    assert_eq!(viewed(&state), Some(id(IMAGE_IDS[2])));
}

#[test]
fn the_left_arrow_shows_the_previous_image() {
    let mut state = viewing_image(IMAGE_IDS[2]);
    let _ = state.update(Message::Shortcut(Shortcut::Previous));
    assert_eq!(viewed(&state), Some(id(IMAGE_IDS[1])));
}

#[test]
fn stepping_twice_keeps_the_gallery() {
    let mut state = viewing_image(IMAGE_IDS[0]);
    let _ = state.update(Message::Shortcut(Shortcut::Next));
    let _ = state.update(Message::Shortcut(Shortcut::Next));
    assert_eq!(viewed(&state), Some(id(IMAGE_IDS[2])));
}

#[test]
fn the_last_image_has_no_next() {
    let mut state = viewing_image(IMAGE_IDS[2]);
    let _ = state.update(Message::Shortcut(Shortcut::Next));
    assert_eq!(viewed(&state), Some(id(IMAGE_IDS[2])));
}

#[test]
fn the_first_image_has_no_previous() {
    let mut state = viewing_image(IMAGE_IDS[0]);
    let _ = state.update(Message::Shortcut(Shortcut::Previous));
    assert_eq!(viewed(&state), Some(id(IMAGE_IDS[0])));
}

/// Sends `messages` to `state`, as the runtime does.
fn deliver(state: &mut State, messages: Vec<Message>) {
    for message in messages {
        let _ = state.update(message);
    }
}

#[test]
fn the_share_picker_follows_its_field_when_the_dialog_grows() {
    let mut state = state_with_listing();
    let file = node(FILE_NAME, NodeKind::File, FILE_ID);
    let (dialog, _) = share::ShareDialog::open(&state.client, vec![file], "alice".to_owned());
    state.share = Some(dialog);
    let dana = protocol::UserRef {
        id: protocol::UserId(Uuid::from_u128(11)),
        username: "dana".to_owned(),
    };
    let _ = state.update(Message::Share(share::Message::Loaded(Ok(Vec::new()))));
    let _ = state.update(Message::Share(share::Message::UsersLoaded(Ok(vec![
        dana.clone(),
    ]))));
    let messages: Vec<_> = {
        let mut ui = simulator(state.view());
        ui.click(client_core::share::ADD_PEOPLE_PLACEHOLDER)
            .expect("the field is shown");
        ui.into_messages().collect()
    };
    deliver(&mut state, messages);
    let Some(Menu::SharePeople(opened)) = state.menu else {
        panic!("the picker is open");
    };

    let _ = state.update(Message::Share(share::Message::Toggle(dana)));
    let messages: Vec<_> = {
        let mut ui = simulator(state.view());
        ui.simulate([iced::Event::Window(window::Event::RedrawRequested(
            std::time::Instant::now(),
        ))]);
        ui.into_messages().collect()
    };
    deliver(&mut state, messages);

    let Some(Menu::SharePeople(followed)) = state.menu else {
        panic!("the picker stays open");
    };
    assert_ne!(followed, opened);
}

mod protected_folders {
    use std::{collections::HashMap, sync::Arc};

    use protocol::{FolderLock, ProtectedFolder, ProtectionState};
    use vault_core::{FolderKeys, FolderSecret};

    use super::*;
    use crate::protected::Listed;

    fn keys() -> FolderKeys {
        FolderSecret::generate().keys().expect("keys")
    }

    fn protection(keys: &FolderKeys) -> ProtectedFolder {
        ProtectedFolder {
            node_id: id(FOLDER_ID),
            lock: FolderLock::Password,
            wrapped_secret: Vec::new().into(),
            epoch: None,
            kdf: None,
            recovery_wrapped_secret: None,
            recipient: keys.recipient().to_string(),
            format: 1,
            state: ProtectionState::Protected,
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    /// The browser in the protected folder "Folder B", listing one file named `stored`.
    fn in_protected_folder(keys: Option<FolderKeys>, stored: &str) -> State {
        let mut state = state_with_listing();
        let folder = protection(keys.as_ref().unwrap_or(&self::keys()));
        let _ = state.update(Message::ProtectedLoaded(Ok(Listed {
            folders: vec![folder.clone()],
            ancestors: HashMap::new(),
        })));
        if let Some(keys) = keys {
            state
                .protected
                .unlock(&folder, Arc::new(keys))
                .expect("unlocked");
        }
        let _ = state.update(Message::Jump(Some(id(FOLDER_ID))));
        let mut file = node(stored, NodeKind::File, FILE_ID);
        file.parent_id = Some(id(FOLDER_ID));
        let listing = Listing {
            path: vec![node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID)],
            nodes: vec![file],
        };
        let _ = state.update(Message::Loaded(Some(id(FOLDER_ID)), Ok(listing)));
        state
    }

    fn sealed(keys: &FolderKeys, name: &str) -> String {
        client_core::protected::seal_name(keys, id(FOLDER_ID), NodeKind::File, name)
            .expect("sealed")
    }

    #[test]
    fn a_locked_folder_lists_nothing_and_offers_unlock() {
        let state = in_protected_folder(None, "c2VhbGVk");

        assert!(state.folder.listing.nodes.is_empty());
        let mut ui = simulator(state.view());
        assert!(ui.find("“Folder B” is locked").is_ok());
        ui.click("Unlock…").expect("the button is shown");
        assert!(ui.into_messages().any(
            |message| matches!(message, Message::UnlockFolder(folder) if folder == id(FOLDER_ID))
        ));
    }

    #[test]
    fn nothing_is_added_to_a_locked_folder() {
        let mut state = in_protected_folder(None, "c2VhbGVk");
        assert_eq!(state.adding_inside().err(), Some(LOCKED));
    }

    #[test]
    fn an_unlocked_folder_lists_its_names_opened() {
        let keys = keys();
        let stored = sealed(&keys, FILE_NAME);
        let state = in_protected_folder(Some(keys), &stored);

        let names: Vec<&str> = state
            .folder
            .listing
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(names, [FILE_NAME]);
        let mut ui = simulator(state.view());
        assert!(ui.find(FILE_NAME).is_ok());
        assert!(ui.find("Lock").is_ok());
    }

    #[test]
    fn locking_wipes_the_keys_and_lists_nothing_again() {
        let keys = keys();
        let stored = sealed(&keys, FILE_NAME);
        let mut state = in_protected_folder(Some(keys), &stored);

        let _ = state.update(Message::LockFolder(id(FOLDER_ID)));
        assert!(!state.protected.is_unlocked(id(FOLDER_ID)));

        let mut file = node(&stored, NodeKind::File, FILE_ID);
        file.parent_id = Some(id(FOLDER_ID));
        let listing = Listing {
            path: vec![node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID)],
            nodes: vec![file],
        };
        let _ = state.update(Message::Loaded(Some(id(FOLDER_ID)), Ok(listing)));
        assert!(state.folder.listing.nodes.is_empty());
        assert!(state.folder.protection.is_locked());
    }

    #[test]
    fn deleting_inside_says_it_is_for_good() {
        let keys = keys();
        let stored = sealed(&keys, FILE_NAME);
        let mut state = in_protected_folder(Some(keys), &stored);
        let _ = state.update(Message::Toggle(id(FILE_ID)));

        let _ = state.update(Message::Act(Action::Delete));

        assert!(simulator(state.view()).find("Delete for good").is_ok());
    }

    #[test]
    fn a_plain_copy_out_of_a_protected_folder_is_asked_once_until_it_locks() {
        let keys = keys();
        let stored = sealed(&keys, FILE_NAME);
        let mut state = in_protected_folder(Some(keys), &stored);
        let _ = state.update(Message::Toggle(id(FILE_ID)));

        let _ = state.update(Message::Act(Action::OpenWith));
        let mut ui = simulator(state.view());
        assert!(
            ui.find(format!("Open “{FILE_NAME}” with another app?"))
                .is_ok()
        );
        drop(ui);
        let _ = state.update(Message::Dialog(dialogs::Message::Submit));
        assert!(state.dialog.is_none());
        assert!(state.protected.plain_copies_allowed(id(FOLDER_ID)));

        let _ = state.update(Message::Act(Action::Download));
        assert!(state.dialog.is_none(), "agreed already");

        // Locking ends the agreement; so does unlocking anew.
        let _ = state.update(Message::LockFolder(id(FOLDER_ID)));
        assert!(!state.protected.plain_copies_allowed(id(FOLDER_ID)));
        let keys = self::keys();
        let stored = sealed(&keys, FILE_NAME);
        let mut state = in_protected_folder(Some(keys), &stored);
        let _ = state.update(Message::Toggle(id(FILE_ID)));
        let _ = state.update(Message::Act(Action::Download));
        let mut ui = simulator(state.view());
        assert!(
            ui.find(format!("Save “{FILE_NAME}” without protection?"))
                .is_ok()
        );
        ui.click("Cancel").expect("the button is shown");
        for message in ui.into_messages() {
            let _ = state.update(message);
        }
        assert!(state.dialog.is_none());
        assert!(!state.protected.plain_copies_allowed(id(FOLDER_ID)));
    }

    #[test]
    fn what_is_inside_is_never_shared_nor_sent() {
        let keys = keys();
        let stored = sealed(&keys, FILE_NAME);
        let mut state = in_protected_folder(Some(keys), &stored);
        let _ = state.update(Message::Toggle(id(FILE_ID)));
        let selected = state.folder.selected();
        let selected: Vec<&Node> = selected.iter().collect();

        let enabled: Vec<Action> =
            toolbar::actions(&selected, id(FOUND_ID), state.limits(&selected))
                .into_iter()
                .filter(|(_, enabled)| *enabled)
                .map(|(action, _)| action)
                .collect();
        assert!(!enabled.contains(&Action::Share));
        assert!(!enabled.contains(&Action::Send));
        assert!(enabled.contains(&Action::Download));
    }

    #[test]
    fn the_menu_of_a_protected_folder_unlocks_it_or_locks_it() {
        let keys = keys();
        let mut state = state_with_listing();
        let folder = protection(&keys);
        let _ = state.update(Message::ProtectedLoaded(Ok(Listed {
            folders: vec![folder.clone()],
            ancestors: HashMap::new(),
        })));
        let listed = node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID);
        let labels = |state: &State| -> Vec<&'static str> {
            state
                .protect_entries(&[&listed])
                .into_iter()
                .map(|entry| entry.label)
                .collect()
        };
        assert_eq!(labels(&state), ["Unlock…"]);

        state
            .protected
            .unlock(&folder, Arc::new(keys))
            .expect("unlocked");
        assert_eq!(labels(&state), ["Lock", "Change password…", "Unprotect…"]);
    }

    #[test]
    fn a_link_to_a_locked_folder_opens_it_and_asks_for_its_password() {
        let mut state = state_with_listing();
        let folder = protection(&keys());
        let _ = state.update(Message::ProtectedLoaded(Ok(Listed {
            folders: vec![folder],
            ancestors: HashMap::new(),
        })));
        let link = AppLink::folder("http://localhost", id(FOLDER_ID)).expect("link");

        let _ = state.update(Message::OpenLink(link));
        assert_eq!(state.folder.id, Some(id(FOLDER_ID)));
        let listing = Listing {
            path: vec![node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID)],
            nodes: Vec::new(),
        };
        let _ = state.update(Message::Loaded(Some(id(FOLDER_ID)), Ok(listing)));

        assert!(state.protect.is_some());
        assert!(simulator(state.view()).find("Unlock “Folder B”").is_ok());
    }

    #[test]
    fn a_link_to_another_server_opens_nothing_and_says_so() {
        let mut state = state_with_listing();
        let link = AppLink::folder("https://elsewhere.example.com", id(FOLDER_ID)).expect("link");

        let _ = state.update(Message::OpenLink(link));

        assert_eq!(state.folder.id, None);
        assert!(
            state
                .folder
                .error
                .as_deref()
                .is_some_and(|error| error.contains("another server"))
        );
    }

    fn vault(keys: &[&str], recovery: bool) -> protocol::Vault {
        let unlocker = |kind, name: Option<&str>, n: u8| protocol::VaultUnlocker {
            id: Uuid::from_bytes([n; 16]),
            kind,
            name: name.map(str::to_owned),
            credential_id: name.map(|_| vec![n; 16].into()),
            rp_id: name.map(|_| "localhost".to_owned()),
            prf_salt: name.map(|_| vec![n; 32].into()),
            wrapped_vault_key: vec![0; protocol::WRAPPED_LEN].into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
        let mut unlockers: Vec<protocol::VaultUnlocker> = keys
            .iter()
            .zip(1u8..)
            .map(|(name, n)| unlocker(protocol::UnlockerKind::SecurityKey, Some(name), n))
            .collect();
        if recovery {
            unlockers.push(unlocker(protocol::UnlockerKind::Recovery, None, 99));
        }
        protocol::Vault {
            epoch: 1,
            unlockers,
        }
    }

    fn vault_loaded(state: &mut State, vault: Option<protocol::Vault>) {
        let _ = state.update(Message::Protect(protect::Message::VaultLoaded(Ok(vault))));
    }

    #[test]
    fn a_security_key_folder_asks_for_the_pin_or_the_recovery_key() {
        let mut state = state_with_listing();
        let mut folder = protection(&keys());
        folder.lock = FolderLock::SecurityKey;
        folder.epoch = Some(1);
        let _ = state.update(Message::ProtectedLoaded(Ok(Listed {
            folders: vec![folder],
            ancestors: HashMap::new(),
        })));
        let _ = state.update(Message::UnlockFolder(id(FOLDER_ID)));
        vault_loaded(&mut state, Some(vault(&["YubiKey"], true)));

        let mut ui = simulator(state.view());
        assert!(ui.find("Unlock “Folder B”").is_ok());
        assert!(ui.find("Uses your security key “YubiKey”.").is_ok());
        assert!(ui.find("PIN of “YubiKey”").is_ok());
        ui.click("Lost your security key?")
            .expect("offered with a recovery key");
        for message in ui.into_messages() {
            let _ = state.update(message);
        }

        let mut ui = simulator(state.view());
        assert!(ui.find("Recovery key").is_ok());
        assert!(ui.find("PIN of “YubiKey”").is_err());
    }

    #[test]
    fn protecting_with_a_security_key_needs_one_names_it_and_warns_about_a_single_one() {
        let mut state = state_with_listing();
        let folder = node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID);
        let _ = state.update(Message::ProtectWithKey(folder.clone()));
        vault_loaded(&mut state, None);

        let mut ui = simulator(state.view());
        assert!(ui.find("Protect “Folder B” with a security key").is_ok());
        assert!(ui.find(protect::NO_KEY).is_ok());
        assert!(ui.find("PIN of your security key").is_err());
        ui.click("Protect").expect("the button is shown");
        assert!(
            !ui.into_messages()
                .any(|message| matches!(message, Message::Protect(protect::Message::Submit))),
            "nothing to protect with"
        );

        let _ = state.update(Message::ProtectWithKey(folder));
        vault_loaded(&mut state, Some(vault(&["YubiKey"], false)));
        let mut ui = simulator(state.view());
        assert!(ui.find(protect::ONE_KEY).is_ok());
        assert!(ui.find("Uses your security key “YubiKey”.").is_ok());
        assert!(ui.find("PIN of “YubiKey”").is_ok());
        assert!(ui.find("Add a key…").is_ok());
        assert!(ui.find("Create a recovery key…").is_ok());
        drop(ui);

        let _ = state.update(Message::ProtectWithKey(node(
            FOLDER_NAME,
            NodeKind::Folder,
            FOLDER_ID,
        )));
        vault_loaded(&mut state, Some(vault(&["YubiKey", "Spare"], false)));
        let mut ui = simulator(state.view());
        assert!(ui.find(protect::ONE_KEY).is_err());
        assert!(
            ui.find("Uses one of your security keys: “YubiKey” or “Spare”. Only one is needed.")
                .is_ok()
        );
        assert!(ui.find("PIN of the key you use").is_ok());
    }

    fn sync_status(state: &mut State, status: sync::Status) {
        let update = sync::Update::Status(Arc::new(status));
        let _ = state.update(Message::Sync(syncing::Message::Engine(update)));
    }

    /// Home synced to /sync, which takes "Folder B" along unless it was left out.
    fn home_synced(left_out: bool) -> sync::Status {
        let folder = Some(id(FOLDER_ID));
        let mut status = sync::Status {
            mappings: vec![sync::Mapping {
                remote: None,
                local: Some(PathBuf::from("/sync")),
            }],
            locations: HashMap::from([
                (None, PathBuf::from("/sync")),
                (folder, PathBuf::from("/sync/Folder B")),
            ]),
            ..sync::Status::default()
        };
        if left_out {
            status.badges.insert(folder, sync::Badge::Disabled);
        }
        status
    }

    #[test]
    fn a_folder_synced_through_home_can_be_protected_but_not_one_synced_on_its_own() {
        let mut state = state_with_listing();
        sync_status(&mut state, home_synced(false));
        assert!(!state.syncs_on_its_own(id(FOLDER_ID)));

        let mut own = home_synced(false);
        own.mappings.push(sync::Mapping {
            remote: Some(id(FOLDER_ID)),
            local: Some(PathBuf::from("/elsewhere")),
        });
        sync_status(&mut state, own);
        assert!(state.syncs_on_its_own(id(FOLDER_ID)));
    }

    /// The browser at Home with "Folder B" unlocked and still being protected.
    fn protecting_folder_b() -> State {
        let keys = keys();
        let mut state = state_with_listing();
        let mut folder = protection(&keys);
        folder.state = ProtectionState::Protecting;
        let _ = state.update(Message::ProtectedLoaded(Ok(Listed {
            folders: vec![folder.clone()],
            ancestors: HashMap::new(),
        })));
        state
            .protected
            .unlock(&folder, Arc::new(keys))
            .expect("unlocked");
        state
    }

    #[test]
    fn a_folder_does_not_open_while_it_is_encrypted() {
        let mut state = protecting_folder_b();
        let _ = state.start_conversion(id(FOLDER_ID), convert::Direction::Protect);
        assert!(
            simulator(state.view())
                .find("“Folder B” opens when this is done.")
                .is_ok()
        );

        let _ = state.activate(id(FOLDER_ID));
        assert_eq!(state.folder.id, None, "still at Home");

        let finished = convert::Finished {
            folder: None,
            binned: Vec::new(),
        };
        let _ = state.update(Message::Converting(
            id(FOLDER_ID),
            convert::Progress::Done(Ok(finished)),
        ));
        let _ = state.activate(id(FOLDER_ID));
        assert_eq!(state.folder.id, Some(id(FOLDER_ID)));
    }

    #[test]
    fn a_folder_is_locked_once_it_is_protected() {
        let mut state = protecting_folder_b();
        let _ = state.start_conversion(id(FOLDER_ID), convert::Direction::Protect);
        assert!(
            state.protected.is_unlocked(id(FOLDER_ID)),
            "open to encrypt"
        );

        let mut folder = state.protected.get(id(FOLDER_ID)).expect("known").clone();
        folder.state = ProtectionState::Protected;
        let finished = convert::Finished {
            folder: Some(folder),
            binned: Vec::new(),
        };
        let _ = state.update(Message::Converting(
            id(FOLDER_ID),
            convert::Progress::Done(Ok(finished)),
        ));

        assert!(!state.protected.is_unlocked(id(FOLDER_ID)));
        let _ = state.activate(id(FOLDER_ID));
        let listing = Listing {
            path: vec![node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID)],
            nodes: Vec::new(),
        };
        let _ = state.update(Message::Loaded(Some(id(FOLDER_ID)), Ok(listing)));
        assert!(simulator(state.view()).find("Unlock…").is_ok());
    }

    #[test]
    fn a_conversion_started_inside_the_folder_leaves_it_for_its_parent() {
        let mut state = protecting_folder_b();
        let _ = state.open_folder(Some(id(FOLDER_ID)));
        let listing = Listing {
            path: vec![node(FOLDER_NAME, NodeKind::Folder, FOLDER_ID)],
            nodes: Vec::new(),
        };
        let _ = state.update(Message::Loaded(Some(id(FOLDER_ID)), Ok(listing.clone())));
        assert_eq!(state.folder.id, Some(id(FOLDER_ID)));

        let _ = state.start_conversion(id(FOLDER_ID), convert::Direction::Protect);
        assert_eq!(state.folder.id, None, "back at Home, which holds it");

        // A link straight inside lands on its parent too.
        state.folder.open(Some(id(FOLDER_ID)));
        let _ = state.update(Message::Loaded(Some(id(FOLDER_ID)), Ok(listing)));
        assert_eq!(state.folder.id, None);
    }

    #[test]
    fn protecting_a_synced_folder_encrypts_once_the_sync_has_left_it() {
        let keys = keys();
        let mut state = state_with_listing();
        let mut folder = protection(&keys);
        folder.state = ProtectionState::Protecting;
        let _ = state.update(Message::ProtectedLoaded(Ok(Listed {
            folders: vec![folder.clone()],
            ancestors: HashMap::new(),
        })));
        state
            .protected
            .unlock(&folder, Arc::new(keys))
            .expect("unlocked");
        sync_status(&mut state, home_synced(false));

        let _ = state.start_conversion(id(FOLDER_ID), convert::Direction::Protect);
        let waiting = |state: &State| {
            state
                .converting
                .as_ref()
                .is_some_and(|conversion| conversion.leaving_sync)
        };
        assert!(waiting(&state), "the sync still follows the folder");
        assert!(
            simulator(state.view())
                .find(conversion::LEAVING_SYNC)
                .is_ok()
        );

        sync_status(&mut state, home_synced(false));
        assert!(waiting(&state), "a pass from before the change");

        sync_status(&mut state, home_synced(true));
        let conversion = state.converting.as_ref().expect("converting");
        assert!(!conversion.leaving_sync);
        assert!(
            simulator(state.view())
                .find("Counting the files to encrypt…")
                .is_ok()
        );
    }

    #[test]
    fn the_security_keys_list_names_each_key_and_the_recovery_key() {
        let mut state = state_with_listing();
        let _ = state.update(Message::FolderKeys);
        vault_loaded(&mut state, Some(vault(&["YubiKey"], true)));

        let mut ui = simulator(state.view());
        assert!(ui.find("Security keys for protected folders").is_ok());
        assert!(ui.find("YubiKey").is_ok());
        assert!(ui.find("Replace the recovery key…").is_ok());
        ui.click("Remove").expect("each key can be removed");
        for message in ui.into_messages() {
            let _ = state.update(message);
        }
        let mut ui = simulator(state.view());
        assert!(ui.find("Remove “YubiKey”?").is_ok());

        ui.click("Cancel").expect("the button is shown");
        for message in ui.into_messages() {
            let _ = state.update(message);
        }
        assert!(state.protect.is_some(), "back to the list, not closed");

        let _ = state.update(Message::FolderKeys);
        vault_loaded(&mut state, None);
        let mut ui = simulator(state.view());
        assert!(ui.find("No security keys yet.").is_ok());
        ui.click("Add a key…").expect("the button is shown");
        for message in ui.into_messages() {
            let _ = state.update(message);
        }
        let mut ui = simulator(state.view());
        assert!(ui.find("PIN of the new key").is_ok());
        assert!(
            ui.find("PIN of a key you already have").is_err(),
            "the first key starts the vault"
        );
    }

    #[test]
    fn adding_another_key_opens_the_vault_with_one_already_there() {
        let mut state = state_with_listing();
        let _ = state.update(Message::FolderKeys);
        vault_loaded(&mut state, Some(vault(&["YubiKey"], true)));
        let _ = state.update(Message::Protect(protect::Message::AddKey));

        let mut ui = simulator(state.view());
        assert!(ui.find("PIN of the new key").is_ok());
        assert!(ui.find("PIN of a key you already have").is_ok());
        ui.click("Use the recovery key").expect("there is one");
        for message in ui.into_messages() {
            let _ = state.update(message);
        }
        assert!(simulator(state.view()).find("Recovery key").is_ok());
    }

    fn remove_first_key(state: &mut State, vault: protocol::Vault) {
        let _ = state.update(Message::FolderKeys);
        vault_loaded(state, Some(vault));
        let mut ui = simulator(state.view());
        ui.click("Remove").expect("each key can be removed");
        for message in ui.into_messages() {
            let _ = state.update(message);
        }
        let _ = state.update(Message::Protect(protect::Message::Lost(true)));
    }

    #[test]
    fn removing_a_lost_key_replaces_the_vault_key_with_every_key_that_stays() {
        let mut state = state_with_listing();
        remove_first_key(&mut state, vault(&["YubiKey", "Spare", "Desk"], true));
        let mut ui = simulator(state.view());
        assert!(ui.find("Remove “YubiKey”?").is_ok());
        assert!(ui.find("PIN of “Spare”").is_ok());
        assert!(ui.find("PIN of “Desk”").is_ok());
        assert!(ui.find("PIN of “YubiKey”").is_err());
        assert!(ui.find("Remove and replace the vault key").is_ok());
        assert!(
            ui.find("Your recovery key is replaced as well; the new one is shown once.")
                .is_ok()
        );
        drop(ui);
        // Not before every PIN is typed.
        let _ = state.update(Message::Protect(protect::Message::Input(
            protect::Field::KeyPin(0),
            "1234".to_owned(),
        )));
        let _ = state.update(Message::Protect(protect::Message::Submit));
        assert!(
            simulator(state.view())
                .find("Remove and replace the vault key")
                .is_ok(),
            "still waiting for the PIN of Desk"
        );

        // The last key, with a recovery key: the recovery key opens the vault.
        remove_first_key(&mut state, vault(&["YubiKey"], true));
        let mut ui = simulator(state.view());
        assert!(ui.find("Recovery key").is_ok());
        drop(ui);

        // The last key and nothing else: nothing to replace the vault key with.
        remove_first_key(&mut state, vault(&["YubiKey"], false));
        let mut ui = simulator(state.view());
        assert!(
            ui.find("Nothing else opens your vault, so there is nothing to replace its key with.")
                .is_ok()
        );
    }

    #[test]
    fn unplugging_the_security_key_locks_the_folder_it_unlocked() {
        let keys = keys();
        let stored = sealed(&keys, FILE_NAME);
        let mut state = in_protected_folder(Some(keys), &stored);
        state
            .protected
            .unlocked_with_key(id(FOLDER_ID), "/dev/hidraw2".to_owned());

        let _ = state.update(Message::KeysPlugged(vec!["/dev/hidraw2".to_owned()]));
        assert!(state.protected.is_unlocked(id(FOLDER_ID)));

        let _ = state.update(Message::KeysPlugged(Vec::new()));
        assert!(!state.protected.is_unlocked(id(FOLDER_ID)));
    }

    #[test]
    fn a_link_names_the_same_server_whatever_its_path() {
        assert!(same_server(
            "https://files.example.com/",
            "https://files.example.com"
        ));
        assert!(same_server(
            "http://localhost:8080/x",
            "http://localhost:8080"
        ));
        assert!(!same_server(
            "https://files.example.com",
            "http://files.example.com"
        ));
        assert!(!same_server(
            "https://a.example.com",
            "https://b.example.com"
        ));
    }
}
