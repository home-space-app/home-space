mod breadcrumbs;
mod details;
mod file_drop;
mod listing;
mod node_table;
mod toolbar;

use std::collections::HashSet;

use leptos::{html::Input, prelude::*, task::spawn_local};
use web_sys::File;

use client_core::labels::HOME;
use protocol::{Node, NodeId, TicketRequest};

use self::{
    breadcrumbs::Breadcrumbs,
    details::DetailsPanel,
    file_drop::use_file_drop,
    listing::Listing,
    node_table::{NodeTable, TableActions},
    toolbar::Toolbar,
};
use super::{
    delete_dialog::DeleteDialog,
    move_dialog::MoveDialog,
    send_dialog::{SendDialog, SendSource},
};
use crate::{
    components::ErrorAlert,
    session::{self, Auth},
    uploads::{UploadItem, start_upload},
};

#[component]
pub fn Browser(auth: Auth) -> impl IntoView {
    view! { <Explorer auth /> }
}

#[component]
fn Explorer(auth: Auth) -> impl IntoView {
    let listing = Listing::new(auth);
    let Listing {
        folder,
        nodes,
        sorted,
        crumbs,
        sort,
        error,
        ..
    } = listing;
    let moving = RwSignal::new(None::<Vec<Node>>);
    let deleting = RwSignal::new(None::<Vec<Node>>);
    let selected = RwSignal::new(HashSet::<NodeId>::new());
    let uploads = expect_context::<RwSignal<Vec<UploadItem>>>();
    let draft = RwSignal::new(None::<String>);
    let renaming = RwSignal::new(None::<(NodeId, String)>);
    let saving = RwSignal::new(false);
    let draft_input = NodeRef::<Input>::new();
    let details = RwSignal::new(None::<NodeId>);
    let send_from = RwSignal::new(None::<SendSource>);

    Effect::new(move |_| {
        folder.track();
        details.set(None);
        draft.set(None);
        renaming.set(None);
        selected.update(HashSet::clear);
    });

    let selected_nodes = Memo::new(move |_| {
        let ids = selected.read();
        sorted
            .read()
            .iter()
            .filter(|node| ids.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>()
    });

    let details_target = Memo::new(move |_| {
        let open = details.get()?;
        match selected_nodes.read().as_slice() {
            [only] => Some(only.id),
            _ => Some(open),
        }
    });
    Effect::new(move |_| {
        if let Some(input) = draft_input.get() {
            let _ = input.focus();
        }
    });

    let new_folder = Callback::new(move |()| {
        if draft.get_untracked().is_none() {
            draft.set(Some(String::new()));
        } else if let Some(input) = draft_input.get_untracked() {
            let _ = input.focus();
        }
    });
    let cancel_folder = Callback::new(move |()| draft.set(None));
    let create_folder = Callback::new(move |()| {
        let name = draft.get_untracked().unwrap_or_default().trim().to_owned();
        if name.is_empty() {
            cancel_folder.run(());
        } else if !saving.get_untracked() {
            saving.set(true);
            let parent = folder.get_untracked();
            spawn_local(async move {
                let result = session::client()
                    .create_folder(parent, &name)
                    .await
                    .map(drop);
                if result.is_ok() {
                    draft.set(None);
                }
                saving.set(false);
                listing.settle(result);
            });
        }
    });
    let cancel_rename = Callback::new(move |()| renaming.set(None));
    let save_rename = Callback::new(move |()| {
        let Some((id, name)) = renaming.get_untracked() else {
            return;
        };
        let name = name.trim().to_owned();
        let unchanged = nodes
            .read_untracked()
            .iter()
            .any(|node| node.id == id && node.name == name);
        if name.is_empty() || unchanged {
            cancel_rename.run(());
        } else if !saving.get_untracked() {
            saving.set(true);
            spawn_local(async move {
                let result = session::client().rename(id, &name).await.map(drop);
                if result.is_ok() {
                    renaming.set(None);
                }
                saving.set(false);
                listing.settle(result);
            });
        }
    });
    let delete = Callback::new(move |targets: Vec<Node>| deleting.set(Some(targets)));
    let confirm_delete = Callback::new(move |targets: Vec<Node>| {
        spawn_local(async move {
            let client = session::client();
            let mut outcome = Ok(());
            for node in targets {
                outcome = client.delete(node.id).await;
                if outcome.is_err() {
                    break;
                }
            }
            selected.update(HashSet::clear);
            listing.settle(outcome);
        });
    });
    let move_selected = Callback::new(move |()| moving.set(Some(selected_nodes.get_untracked())));
    let send = Callback::new(move |nodes: Vec<Node>| send_from.set(Some(SendSource::Nodes(nodes))));
    let send_selected = Callback::new(move |()| send.run(selected_nodes.get_untracked()));
    let send_local = Callback::new(move |()| send_from.set(Some(SendSource::Local)));
    let delete_selected = Callback::new(move |()| delete.run(selected_nodes.get_untracked()));
    let selected_count = Signal::derive(move || selected_nodes.read().len());
    let lost_and_found = Signal::derive(move || auth.user.get().map(|user| user.lost_and_found_id));
    let can_delete = Signal::derive(move || {
        let protected = lost_and_found.get();
        protected.is_none_or(|id| !selected_nodes.read().iter().any(|node| node.id == id))
    });
    // Lost & Found only receives restored files, so nothing is created or uploaded into it.
    let can_add = Signal::derive(move || {
        let current = folder.get();
        current.is_none() || current != lost_and_found.get()
    });
    let download = Callback::new(move |node: Node| {
        spawn_local(async move {
            let client = session::client();
            let request = TicketRequest::Download { node_id: node.id };
            match client.ticket(&request).await {
                Ok(ticket) => {
                    if let Some(window) = web_sys::window() {
                        let _ = window
                            .location()
                            .set_href(&client.download_url(node.id, &ticket.ticket));
                    }
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    });
    let show_details = Callback::new(move |node: Node| {
        selected.set(HashSet::from([node.id]));
        details.set(Some(node.id));
    });
    // The send dialog takes files dropped on it; the page takes them otherwise.
    let can_drop = Signal::derive(move || can_add.get() && send_from.read().is_none());
    let upload = Callback::new(move |files: Vec<File>| {
        if can_drop.get_untracked() {
            start_upload(
                uploads,
                move || listing.refetch(),
                auth,
                folder.get_untracked(),
                files,
            );
        }
    });

    let dragging = use_file_drop(can_drop, upload);

    let actions = TableActions {
        draft,
        renaming,
        saving,
        moving,
        selected,
        create_folder,
        cancel_folder,
        save_rename,
        cancel_rename,
        download,
        send,
        details: show_details,
        delete,
        lost_and_found,
    };

    view! {
        <main class="explorer">
                <div class="explorer__header">
                    <Breadcrumbs crumbs />
                    <Toolbar
                        selected_count
                        on_move=move_selected
                        on_send=send_selected
                        on_send_local=send_local
                        on_delete=delete_selected
                        can_delete
                        can_add
                        on_new_folder=new_folder
                        on_upload=upload
                    />
                </div>
                <ErrorAlert message=error />
                <div class="explorer__body" class:explorer__body--with-details=move || details_target.get().is_some()>
                    <div class="explorer__nodes">
                        <NodeTable nodes=sorted sort actions draft_input />
                        <Show when=move || nodes.read().is_empty() && error.read().is_none() && draft.read().is_none()>
                            <p class="explorer__empty">
                                {move || {
                                    if can_add.get() {
                                        view! {
                                            "Nothing here yet. "
                                            <span class="hint-drop">"Drop files anywhere to upload."</span>
                                            <span class="hint-touch">"Tap Upload to add files."</span>
                                        }
                                            .into_any()
                                    } else {
                                        "Nothing here. Restored files from the recycle bin land in this folder."
                                            .into_any()
                                    }
                                }}
                            </p>
                        </Show>
                    </div>
                    {move || {
                        details_target
                            .get()
                            .map(|id| view! { <DetailsPanel id details auth /> })
                    }}
                </div>
        </main>
        <Show when=move || dragging.get()>
            <div class="drop-overlay" aria-hidden="true">
                <p class="drop-overlay__label">
                    {move || {
                        let target = crumbs
                            .read()
                            .last()
                            .map_or_else(|| HOME.to_owned(), |node| node.name.clone());
                        format!("Drop files to upload to {target}")
                    }}
                </p>
            </div>
        </Show>
        {move || {
            moving
                .get()
                .map(|nodes| view! { <MoveDialog nodes moving selected refresh=move || listing.refetch() auth /> })
        }}
        {move || {
            send_from
                .get()
                .map(|source| {
                    view! { <SendDialog source auth on_close=Callback::new(move |()| send_from.set(None)) /> }
                })
        }}
        {move || {
            deleting
                .get()
                .map(|nodes| view! { <DeleteDialog nodes deleting on_confirm=confirm_delete /> })
        }}
    }
}
