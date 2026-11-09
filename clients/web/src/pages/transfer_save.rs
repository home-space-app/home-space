//! Saving a received transfer into a folder of the user's tree.

use leptos::{html::Input, prelude::*, task::spawn_local};
use web_sys::KeyboardEvent;

use client_core::{
    labels::{HOME, files},
    listing::{Sort, sort_nodes},
    transfers::transfer_size,
};
use protocol::{Node, NodeId, NodeKind, TransferSummary};

use crate::{
    components::ErrorAlert,
    icon::Icon,
    load,
    session::{self, Auth},
};

const KEY_ENTER: &str = "Enter";
const KEY_ESCAPE: &str = "Escape";

/// `Documents / Receipts`, or `Home` at the top.
fn location(path: &[Node]) -> String {
    if path.is_empty() {
        HOME.to_owned()
    } else {
        path.iter()
            .map(|folder| folder.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// Picks a folder and saves `transfer` into it; `on_close` gets whether it was saved.
#[component]
pub fn SaveTransferDialog(
    transfer: TransferSummary,
    auth: Auth,
    on_close: Callback<bool>,
) -> impl IntoView {
    let id = transfer.id;
    let title = format!("Save {} on the server", files(transfer.file_count as usize));
    let mut subtitle = transfer_size(&transfer);
    if let Some(device) = &transfer.sender_device_name {
        subtitle = format!("From {device} · {subtitle}");
    }
    // The folders opened so far, the last one being where the list is.
    let trail = RwSignal::new(Vec::<Node>::new());
    // A folder of the list picked as the destination, instead of the open one.
    let picked = RwSignal::new(None::<Node>);
    let folders = RwSignal::new(Vec::<Node>::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let naming = RwSignal::new(None::<String>);
    let open = Memo::new(move |_| trail.read().last().map(|folder| folder.id));
    let lost_and_found = auth.user.get_untracked().map(|me| me.lost_and_found_id);

    let listing = load::load(
        move || {
            let parent = open.get();
            async move {
                let client = session::client();
                let nodes = match (parent, lost_and_found) {
                    (None, Some(hidden)) => client.list_home(hidden).await?,
                    _ => client.list(parent).await?,
                };
                let mut nodes: Vec<Node> = nodes
                    .into_iter()
                    .filter(|node| node.kind == NodeKind::Folder)
                    .collect();
                sort_nodes(&mut nodes, Sort::default());
                Ok(nodes)
            }
        },
        move |result| match result {
            Ok(listed) => folders.set(listed),
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );

    let destination = move || {
        let mut path = trail.get();
        if let Some(folder) = picked.get() {
            path.push(folder);
        }
        path
    };
    let enter = move |folder: Node| {
        picked.set(None);
        trail.update(|path| path.push(folder));
    };
    let back = move |_| {
        picked.set(None);
        trail.update(|path| {
            path.pop();
        });
    };
    let create = Callback::new(move |()| {
        let Some(name) = naming.get_untracked() else {
            return;
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            naming.set(None);
            return;
        }
        let parent: Option<NodeId> = open.get_untracked();
        spawn_local(async move {
            match session::client().create_folder(parent, &name).await {
                Ok(folder) => {
                    naming.set(None);
                    picked.set(Some(folder));
                    listing.refetch();
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    });
    let save = move |_| {
        let target = destination().last().map(|folder| folder.id);
        busy.set(true);
        spawn_local(async move {
            match session::client().save_transfer(id, target).await {
                Ok(_) => on_close.run(true),
                Err(err) => {
                    busy.set(false);
                    error.set(Some(auth.fail(&err)));
                }
            }
        });
    };
    let name_input = NodeRef::<Input>::new();
    Effect::new(move |_| {
        if naming.read().is_some()
            && let Some(input) = name_input.get()
        {
            let _ = input.focus();
        }
    });

    view! {
        <div class="modal" on:click=move |_| on_close.run(false)>
            <div
                class="modal__panel modal__panel--wide"
                role="dialog"
                aria-modal="true"
                on:click=|event| event.stop_propagation()
            >
                <header class="modal__header">
                    <h2 class="modal__title">{title}</h2>
                    <p class="modal__subtitle">{subtitle}</p>
                </header>
                <nav class="save-trail" aria-label="Folder">
                    <button
                        class="button button--ghost button--small"
                        aria-label="Up one folder"
                        disabled=move || trail.read().is_empty()
                        on:click=back
                    >
                        <Icon name="arrow_back" />
                    </button>
                    <button
                        class="save-trail__crumb"
                        class:save-trail__crumb--current=move || trail.read().is_empty()
                        on:click=move |_| {
                            picked.set(None);
                            trail.set(Vec::new());
                        }
                    >
                        {HOME}
                    </button>
                    <For
                        each=move || trail.get().into_iter().enumerate()
                        key=|(_, folder)| folder.id
                        children=move |(at, folder)| {
                            let current = move || trail.read().len() == at + 1;
                            view! {
                                <Icon name="chevron_right" />
                                <button
                                    class="save-trail__crumb"
                                    class:save-trail__crumb--current=current
                                    on:click=move |_| {
                                        picked.set(None);
                                        trail.update(|path| path.truncate(at + 1));
                                    }
                                >
                                    {folder.name.clone()}
                                </button>
                            }
                        }
                    />
                </nav>
                <ul class="save-folders">
                    {move || {
                        naming
                            .get()
                            .map(|_| {
                                view! {
                                    <li class="save-folders__row save-folders__row--new">
                                        <Icon name="create_new_folder" />
                                        <input
                                            node_ref=name_input
                                            class="field__input save-folders__input"
                                            aria-label="Folder name"
                                            placeholder="Folder name"
                                            on:input:target=move |event| {
                                                naming.set(Some(event.target().value()))
                                            }
                                            on:keydown=move |event: KeyboardEvent| {
                                                match event.key().as_str() {
                                                    KEY_ENTER => create.run(()),
                                                    KEY_ESCAPE => naming.set(None),
                                                    _ => {}
                                                }
                                            }
                                        />
                                        <button
                                            class="button button--small button--primary"
                                            on:click=move |_| create.run(())
                                        >
                                            "Create"
                                        </button>
                                    </li>
                                }
                            })
                    }}
                    <Show when=move || folders.read().is_empty() && naming.read().is_none()>
                        <li class="folder-picker__empty">"No folders here"</li>
                    </Show>
                    <For
                        each=move || folders.get()
                        key=|folder| folder.id
                        children=move |folder| {
                            let folder_id = folder.id;
                            let selected = move || {
                                picked.read().as_ref().is_some_and(|picked| picked.id == folder_id)
                            };
                            let (pick, open_it) = (folder.clone(), folder.clone());
                            let label = format!("Open {}", folder.name);
                            view! {
                                <li
                                    class="save-folders__row"
                                    class:save-folders__row--selected=selected
                                >
                                    <button
                                        class="save-folders__pick"
                                        aria-pressed=move || selected().to_string()
                                        on:click=move |_| {
                                            if selected() {
                                                picked.set(None);
                                            } else {
                                                picked.set(Some(pick.clone()));
                                            }
                                        }
                                        on:dblclick=move |_| enter(open_it.clone())
                                    >
                                        <Icon name="folder" />
                                        <span class="save-folders__name">{folder.name.clone()}</span>
                                    </button>
                                    {move || if selected() {
                                        view! {
                                            <span class="save-folders__check">
                                                <Icon name="check" />
                                            </span>
                                        }
                                            .into_any()
                                    } else {
                                        let folder = folder.clone();
                                        view! {
                                            <button
                                                class="button button--ghost button--small"
                                                aria-label=label.clone()
                                                on:click=move |_| enter(folder.clone())
                                            >
                                                <Icon name="chevron_right" />
                                            </button>
                                        }
                                            .into_any()
                                    }}
                                </li>
                            }
                        }
                    />
                </ul>
                <p class="modal__note">
                    {move || format!(
                        "Files go to {} and sync like any other files.",
                        location(&destination()),
                    )}
                </p>
                <ErrorAlert message=error />
                <div class="modal__actions modal__actions--split">
                    <button
                        class="button button--ghost"
                        disabled=move || naming.read().is_some()
                        on:click=move |_| naming.set(Some(String::new()))
                    >
                        <Icon name="create_new_folder" />
                        "New folder"
                    </button>
                    <span class="page-header__spacer"></span>
                    <button class="button" on:click=move |_| on_close.run(false)>
                        "Cancel"
                    </button>
                    <button
                        class="button button--primary"
                        disabled=move || busy.get()
                        on:click=save
                    >
                        <Icon name="check" />
                        "Save"
                    </button>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use protocol::NodeId;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn folder(name: &str) -> Node {
        Node {
            id: NodeId(Uuid::nil()),
            parent_id: None,
            kind: NodeKind::Folder,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    #[test]
    fn the_destination_reads_as_a_path() {
        assert_eq!(location(&[]), "Home");
        assert_eq!(
            location(&[folder("Documents"), folder("Receipts")]),
            "Documents / Receipts"
        );
    }
}
