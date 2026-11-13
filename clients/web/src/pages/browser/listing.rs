use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_params_map};

use api_client::ApiError;
use client_core::listing::{Sort, affects_path, apply_event, reveals_lost_and_found, sort_nodes};
use protocol::{Event, Node, NodeId};

use crate::{
    events, load, route,
    session::{self, Auth},
};

type Loaded = Result<(Vec<Node>, Vec<Node>), ApiError>;

async fn fetch(folder: Option<NodeId>, lost_and_found: Option<NodeId>) -> Loaded {
    let client = session::client();
    let listing = match (folder, lost_and_found) {
        (None, Some(found)) => client.list_home(found).await?,
        _ => client.list(folder).await?,
    };
    let path = match folder {
        Some(id) => {
            let mut path = client.ancestors(id).await?;
            path.push(client.get(id).await?);
            path
        }
        None => Vec::new(),
    };
    Ok((listing, path))
}

/// The open folder: its contents and path, loaded from the route and kept current by
/// live events.
#[derive(Clone, Copy)]
pub(super) struct Listing {
    pub folder: Memo<Option<NodeId>>,
    pub nodes: RwSignal<Vec<Node>>,
    pub sorted: Memo<Vec<Node>>,
    pub crumbs: RwSignal<Vec<Node>>,
    pub sort: RwSignal<Sort>,
    pub error: RwSignal<Option<String>>,
    auth: Auth,
    resource: LocalResource<Loaded>,
}

impl Listing {
    pub fn new(auth: Auth) -> Self {
        let params = use_params_map();
        let location = use_location();
        let folder = Memo::new(move |previous: Option<&Option<NodeId>>| {
            if location.pathname.with(|path| route::is_folder_path(path)) {
                params.read().get("id").and_then(|id| route::parse_id(&id))
            } else {
                previous.copied().flatten()
            }
        });
        let nodes = RwSignal::new(Vec::<Node>::new());
        let crumbs = RwSignal::new(Vec::<Node>::new());
        let sort = RwSignal::new(Sort::default());
        let error = RwSignal::new(None::<String>);

        let lost_and_found = move || auth.user.get_untracked().map(|user| user.lost_and_found_id);

        let resource = load::load(
            move || fetch(folder.get(), lost_and_found()),
            move |loaded: Loaded| match loaded {
                Ok((listing, path)) => {
                    nodes.set(listing);
                    crumbs.set(path);
                    error.set(None);
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            },
        );

        let sorted = Memo::new(move |_| {
            let mut listing = nodes.get();
            sort_nodes(&mut listing, sort.get());
            listing
        });

        events::use_events(move |event| {
            let reveals = lost_and_found()
                .is_some_and(|found| reveals_lost_and_found(folder.get_untracked(), found, &event));
            if matches!(event, Event::Resync)
                || reveals
                || affects_path(&crumbs.get_untracked(), &event)
            {
                resource.refetch();
            } else {
                nodes.update(|listing| {
                    apply_event(
                        listing,
                        folder.get_untracked(),
                        &event,
                        sort.get_untracked(),
                    )
                });
            }
        });

        Self {
            folder,
            nodes,
            sorted,
            crumbs,
            sort,
            error,
            auth,
            resource,
        }
    }

    /// Loads the folder again.
    pub fn refetch(self) {
        self.resource.refetch();
    }

    /// Shows the error of a finished change, if any, and loads the folder again.
    pub fn settle(self, result: Result<(), ApiError>) {
        if let Err(err) = result {
            self.error.set(Some(self.auth.fail(&err)));
        }
        self.refetch();
    }
}
