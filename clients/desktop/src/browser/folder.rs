use std::time::{Duration, Instant};

use iced::{Task, keyboard::Modifiers, widget::scrollable::Viewport};

use api_client::{ApiError, Client};
use client_core::listing::{
    Sort, SortKey, affects_path, apply_event, collapsed_crumbs, sort_nodes,
};
use protocol::{Node, NodeId};

use super::{Message, grid::reveal_offset, selection::Selection};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug, Clone)]
pub struct Listing {
    pub(super) path: Vec<Node>,
    pub(super) nodes: Vec<Node>,
}

impl Listing {
    /// The name of folder `id` if the listing or its path holds it.
    pub(super) fn name_of(&self, id: NodeId) -> Option<String> {
        self.nodes
            .iter()
            .chain(&self.path)
            .find(|node| node.id == id)
            .map(|node| node.name.clone())
    }
}

/// The folder being browsed: its listing, order, selection and load state.
pub struct Folder {
    /// The folder shown (`None` for Home).
    pub(super) id: Option<NodeId>,
    /// Lost & Found, which the sidebar offers instead of the listing.
    lost_and_found: Option<NodeId>,
    pub(super) listing: Listing,
    pub(super) sort: Sort,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) selection: Selection,
    last_click: Option<(NodeId, Instant)>,
    /// When a folder link last navigated, to tell the second click of a double click.
    last_link: Option<Instant>,
    /// Where the rows are scrolled; `None` while they all fit.
    pub(super) viewport: Option<Viewport>,
}

impl Default for Folder {
    fn default() -> Self {
        Self {
            id: None,
            lost_and_found: None,
            listing: Listing {
                path: Vec::new(),
                nodes: Vec::new(),
            },
            sort: Sort::default(),
            loading: true,
            error: None,
            selection: Selection::default(),
            last_click: None,
            last_link: None,
            viewport: None,
        }
    }
}

impl Folder {
    pub fn new(lost_and_found: NodeId) -> Self {
        Self {
            lost_and_found: Some(lost_and_found),
            ..Self::default()
        }
    }

    /// Switches to folder `id`; the listing is fetched with [`Folder::load`].
    pub fn open(&mut self, id: Option<NodeId>) {
        self.id = id;
        self.loading = true;
        self.error = None;
        self.selection.clear();
        self.last_click = None;
        self.viewport = None;
    }

    /// Fetches the listing of the folder shown.
    pub fn load(&self, client: &Client) -> Task<Message> {
        let id = self.id;
        Task::perform(fetch(client.clone(), id), move |result| {
            Message::Loaded(id, result)
        })
    }

    /// Like [`Folder::load`], marking the listing as loading.
    pub fn reload(&mut self, client: &Client) -> Task<Message> {
        self.loading = true;
        self.load(client)
    }

    /// Takes in a fetched listing; one of a folder that is no longer shown is dropped.
    pub fn loaded(&mut self, id: Option<NodeId>, result: Result<Listing, ApiError>) {
        if id != self.id {
            return;
        }
        self.loading = false;
        match result {
            Ok(mut listing) => {
                listing
                    .nodes
                    .retain(|node| Some(node.id) != self.lost_and_found);
                sort_nodes(&mut listing.nodes, self.sort);
                self.selection.retain(&listing.nodes);
                self.listing = listing;
                self.error = None;
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    pub fn sort_by(&mut self, key: SortKey) {
        self.sort = self.sort.toggled(key);
        sort_nodes(&mut self.listing.nodes, self.sort);
    }

    /// Takes in a click on a row; returns the row when it completes a double click.
    pub fn click(&mut self, id: NodeId, modifiers: Modifiers) -> Option<NodeId> {
        let now = Instant::now();
        let double = self
            .last_click
            .is_some_and(|(last, at)| last == id && now.duration_since(at) <= DOUBLE_CLICK);
        if double {
            self.last_click = None;
            Some(id)
        } else {
            self.last_click = Some((id, now));
            self.selection.click(id, modifiers, &self.listing.nodes);
            None
        }
    }

    /// Takes in a click on a folder link; returns whether to follow it. The second click of a
    /// double click, a habit from the rows, must not go one level deeper.
    pub fn follow_link(&mut self) -> bool {
        let now = Instant::now();
        let follow = self
            .last_link
            .is_none_or(|at| now.duration_since(at) > DOUBLE_CLICK);
        if follow {
            self.last_link = Some(now);
        }
        follow
    }

    /// Moves a single selection one row; returns the scroll offset that brings the row into view
    /// when it is out of view.
    pub fn step(&mut self, forward: bool) -> Option<f32> {
        let index = self.selection.step(forward, &self.listing.nodes)?;
        let viewport = self.viewport?;
        reveal_offset(
            index,
            viewport.absolute_offset().y,
            viewport.bounds().height,
        )
    }

    /// Applies a server event to the listing; returns whether it has to be fetched again.
    pub fn apply(&mut self, event: &protocol::Event) -> bool {
        if matches!(event, protocol::Event::Resync) || affects_path(&self.listing.path, event) {
            return true;
        }
        apply_event(&mut self.listing.nodes, self.id, event, self.sort);
        self.listing
            .nodes
            .retain(|node| Some(node.id) != self.lost_and_found);
        self.selection.retain(&self.listing.nodes);
        false
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.listing.nodes.iter().find(|node| node.id == id)
    }

    pub fn selected(&self) -> Vec<Node> {
        self.selection
            .selected(&self.listing.nodes)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The folder holding the one shown: `None` at Home, `Some(None)` for Home itself.
    pub fn parent(&self) -> Option<Option<NodeId>> {
        match self.listing.path.as_slice() {
            [] => None,
            [.., parent, _] => Some(Some(parent.id)),
            [_] => Some(None),
        }
    }

    /// The breadcrumbs left out of the trail.
    pub fn hidden_crumbs(&self) -> &[Node] {
        collapsed_crumbs(self.listing.path.len())
            .and_then(|hidden| self.listing.path.get(hidden))
            .unwrap_or_default()
    }
}

async fn fetch(client: Client, folder: Option<NodeId>) -> Result<Listing, ApiError> {
    let (nodes, path) = match folder {
        None => (client.list(None).await?, Vec::new()),
        Some(id) => {
            let (nodes, current, mut path) =
                tokio::try_join!(client.list(Some(id)), client.get(id), client.ancestors(id))?;
            path.push(current);
            (nodes, path)
        }
    };
    Ok(Listing { path, nodes })
}
