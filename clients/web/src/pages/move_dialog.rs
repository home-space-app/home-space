use std::collections::HashSet;

use leptos::{prelude::*, task::spawn_local};

use client_core::{
    format::conflicts_title,
    labels::HOME,
    listing::{Sort, already_in, sort_nodes},
};
use client_core::protected::seal_name;
use protocol::{Conflict, Node, NodeId, NodeKind, OnConflict, ProtectedFolder};

use super::conflict_dialog::ConflictDialog;
use crate::{
    components::ErrorAlert,
    icon::Icon,
    load,
    protected::{Vault, use_vault},
    session,
    session::Auth,
};

const LOCKED_MEANWHILE: &str = "The folder locked meanwhile; unlock it again.";

/// Picks a destination folder and moves `nodes` there; `refresh` reloads the listing behind
/// the dialog once the move is done.
///
/// Inside a protected folder (`within`: the folder and the path to the open folder, names
/// opened) the destination is one of its folders, starting at the open one, and each name is
/// sealed again for its new folder, as a sealed name is bound to it.
#[component]
pub fn MoveDialog(
    nodes: Vec<Node>,
    moving: RwSignal<Option<Vec<Node>>>,
    selected: RwSignal<HashSet<NodeId>>,
    refresh: impl Fn() + Copy + Send + Sync + 'static,
    auth: Auth,
    #[prop(optional_no_strip)] within: Option<(ProtectedFolder, Vec<Node>)>,
) -> impl IntoView {
    let vault = use_vault();
    let root = within.as_ref().map(|(folder, _)| folder.node_id);
    // The trail never goes above the protected folder.
    let (floor, start) = within.map_or((0, Vec::new()), |(folder, path)| {
        let floor = path
            .iter()
            .position(|node| node.id == folder.node_id)
            .map_or(path.len(), |at| at + 1);
        (floor, path)
    });
    let ids: Vec<NodeId> = nodes.iter().map(|node| node.id).collect();
    let title = match nodes.as_slice() {
        [node] => format!("Move “{}”", node.name),
        _ => format!("Move {} items", nodes.len()),
    };
    let moving_ids = ids.clone();
    let origin = nodes.clone();
    let trail = RwSignal::new(start);
    let folders = RwSignal::new(Vec::<Node>::new());
    let error = RwSignal::new(None::<String>);
    let target = Memo::new(move |_| trail.read().last().map(|folder| folder.id));

    load::load(
        move || {
            let wanted = target.get();
            let moving_ids = moving_ids.clone();
            async move {
                let listing = session::client().list(wanted).await?;
                let mut listing: Vec<Node> = listing
                    .into_iter()
                    .filter(|entry| {
                        entry.kind == NodeKind::Folder && !moving_ids.contains(&entry.id)
                    })
                    .collect();
                if let Some(root) = root {
                    listing = vault.opened(root, listing).unwrap_or_default();
                }
                sort_nodes(&mut listing, Sort::default());
                Ok(listing)
            }
        },
        move |result| match result {
            Ok(listing) => folders.set(listing),
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );

    // The title and clashes of a move that was held back, until the user chooses.
    let conflicts = RwSignal::new(None::<(String, Vec<Conflict>)>);
    let sealed_moves = nodes.clone();
    let send = Callback::new(move |mode: OnConflict| {
        let destination: Option<NodeId> = target.get_untracked();
        if let (Some(root), Some(destination)) = (root, destination) {
            let nodes = sealed_moves.clone();
            spawn_local(async move {
                match move_sealed(vault, root, &nodes, destination).await {
                    Ok(()) => {
                        moving.set(None);
                        selected.update(HashSet::clear);
                    }
                    Err(message) => error.set(Some(message)),
                }
                refresh();
            });
            return;
        }
        let ids = ids.clone();
        spawn_local(async move {
            match session::client().move_nodes(&ids, destination, mode).await {
                Ok(response) if !response.conflicts.is_empty() => {
                    let name = response
                        .target
                        .map_or_else(|| HOME.to_owned(), |folder| folder.name);
                    let title = conflicts_title(response.conflicts.len(), &format!("“{name}”"));
                    conflicts.set(Some((title, response.conflicts)));
                }
                Ok(_) => {
                    moving.set(None);
                    selected.update(HashSet::clear);
                    refresh();
                }
                Err(err) => {
                    error.set(Some(auth.fail(&err)));
                    refresh();
                }
            }
        });
    });
    let choose = Callback::new(move |mode: Option<OnConflict>| {
        conflicts.set(None);
        if let Some(mode) = mode {
            send.run(mode);
        }
    });
    // Moving into the folder the nodes are in changes nothing.
    let in_place = Memo::new(move |_| already_in(&origin, target.get()));
    let confirm = move |_| {
        if !in_place.get_untracked() {
            send.run(OnConflict::Abort);
        }
    };

    view! {
        <div class="modal" on:click=move |_| moving.set(None)>
            <div
                class="modal__panel"
                role="dialog"
                aria-modal="true"
                on:click=|event| event.stop_propagation()
            >
                <header class="modal__header">
                    <h2 class="modal__title">{title}</h2>
                    <p class="modal__subtitle">
                        {move || {
                            let names: Vec<String> = trail
                                .read()
                                .iter()
                                .map(|folder| folder.name.clone())
                                .collect();
                            if names.is_empty() {
                                "To Home".to_string()
                            } else {
                                format!("To /{}", names.join("/"))
                            }
                        }}
                    </p>
                </header>
                <ul class="folder-picker">
                    <Show when=move || { floor < trail.read().len() }>
                        <li>
                            <button
                                class="folder-picker__button folder-picker__button--up"
                                on:click=move |_| trail.update(|path| { path.pop(); })
                            >
                                <Icon name="arrow_upward" />
                                "Up"
                            </button>
                        </li>
                    </Show>
                    <Show when=move || folders.read().is_empty()>
                        <li class="folder-picker__empty">"No subfolders"</li>
                    </Show>
                    <For
                        each=move || folders.get()
                        key=|folder| folder.id
                        children=move |folder| {
                            let name = folder.name.clone();
                            view! {
                                <li>
                                    <button
                                        class="folder-picker__button"
                                        on:click=move |_| {
                                            trail.update(|path| path.push(folder.clone()))
                                        }
                                    >
                                        <Icon name="folder" />
                                        {name}
                                    </button>
                                </li>
                            }
                        }
                    />
                </ul>
                <ErrorAlert message=error />
                <div class="modal__actions">
                    <button class="button" on:click=move |_| moving.set(None)>
                        "Cancel"
                    </button>
                    <button
                        class="button button--primary"
                        disabled=move || in_place.get()
                        on:click=confirm
                    >
                        "Move here"
                    </button>
                </div>
            </div>
        </div>
        {move || {
            conflicts
                .get()
                .map(|(title, conflicts)| {
                    view! { <ConflictDialog title conflicts on_choose=choose /> }
                })
        }}
    }
}

/// Moves `nodes`, of the protected folder `root` with their names opened, into its folder
/// `destination`, each under its name sealed for that folder, one by one.
async fn move_sealed(
    vault: Vault,
    root: NodeId,
    nodes: &[Node],
    destination: NodeId,
) -> Result<(), String> {
    let client = session::client();
    for node in nodes {
        let sealed = vault
            .with_keys(root, |keys| seal_name(keys, destination, node.kind, &node.name))
            .ok_or(LOCKED_MEANWHILE)?
            .map_err(|err| format!("“{}”: {err}", node.name))?;
        client
            .relocate(node.id, Some(destination), &sealed)
            .await
            .map_err(|err| format!("“{}” was not moved: {err}", node.name))?;
    }
    Ok(())
}
