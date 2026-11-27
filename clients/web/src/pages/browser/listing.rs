use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_params_map};

use api_client::ApiError;
use client_core::{
    listing::{Sort, affects_path, apply_event, reveals_lost_and_found, sort_nodes},
    protected::{open_listing, open_path},
};
use protocol::{Event, Node, NodeId, ProtectedFolder};

use crate::{
    events, load,
    protected::{Vault, use_vault},
    route,
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
    /// What is shown as the problem: why a change failed, else why loading failed.
    pub error: RwSignal<Option<String>>,
    /// The protected folder the open folder is in, or is.
    pub protection: RwSignal<Option<ProtectedFolder>>,
    /// Whether that folder is locked, so its content is not shown.
    pub locked: Memo<bool>,
    /// Why the last change failed. Kept across reloads, which only clear their own errors,
    /// until a change succeeds or another folder opens.
    change_error: RwSignal<Option<String>>,
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
        let change_error = RwSignal::new(None::<String>);
        let protection = RwSignal::new(None::<ProtectedFolder>);
        let vault = use_vault();
        let locked = Memo::new(move |_| {
            protection
                .read()
                .as_ref()
                .is_some_and(|folder| !vault.is_unlocked(folder.node_id))
        });
        Effect::new(move |_| {
            folder.track();
            change_error.set(None);
        });

        let lost_and_found = move || auth.user.get_untracked().map(|user| user.lost_and_found_id);

        let resource = load::load(
            move || {
                vault.track();
                fetch(folder.get(), lost_and_found())
            },
            move |loaded: Loaded| match loaded {
                Ok((listing, path)) => {
                    let (listing, path, inside) = shown(vault, listing, path);
                    nodes.set(listing);
                    crumbs.set(path);
                    protection.set(inside);
                    error.set(change_error.get_untracked());
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
            // Names inside a protected folder arrive sealed; the listing opens them.
            if matches!(event, Event::Resync)
                || protection.read_untracked().is_some()
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
            protection,
            locked,
            change_error,
            auth,
            resource,
        }
    }

    /// Shows `message` as why a change failed.
    pub fn fail_with(self, message: String) {
        self.change_error.set(Some(message.clone()));
        self.error.set(Some(message));
    }

    /// Loads the folder again.
    pub fn refetch(self) {
        self.resource.refetch();
    }

    /// Shows why a change failed, until a change succeeds or another folder opens.
    pub fn fail(self, err: &ApiError) {
        self.fail_with(self.auth.fail(err));
    }

    /// Shows the error of a finished change, if any, and loads the folder again.
    pub fn settle(self, result: Result<(), ApiError>) {
        match result {
            Ok(()) => self.change_error.set(None),
            Err(err) => self.fail(&err),
        }
        self.refetch();
    }
}

/// The listing and path as shown: inside an unlocked protected folder with the names opened,
/// inside a locked one with nothing below the folder; and the protected folder they are in.
fn shown(
    vault: Vault,
    listing: Vec<Node>,
    path: Vec<Node>,
) -> (Vec<Node>, Vec<Node>, Option<ProtectedFolder>) {
    let Some(inside) = vault.enclosing(path.iter().map(|node| node.id)) else {
        return (listing, path, None);
    };
    let (root, state) = (inside.node_id, inside.state);
    match vault.with_keys(root, |keys| {
        (
            open_listing(keys, listing, state),
            open_path(keys, path.clone(), root, state),
        )
    }) {
        Some((listing, path)) => (listing, path, Some(inside)),
        None => {
            let above = path
                .into_iter()
                .scan(false, |past, node| {
                    let keep = !*past;
                    *past |= node.id == root;
                    keep.then_some(node)
                })
                .collect();
            (Vec::new(), above, Some(inside))
        }
    }
}
