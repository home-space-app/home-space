//! Sending files to one of the user's devices, or to another user's device.

use std::collections::BTreeSet;

use leptos::{prelude::*, task::spawn_local};
use uuid::Uuid;
use web_sys::{DragEvent, File, HtmlInputElement};

use client_core::{
    format::human_size,
    labels::files,
    send::{
        LocalEntry, device_status, first_choice, free_name, items_summary, manifest, names_line,
        pickable, recipient_name, send_note, send_title, upload_note,
    },
};
use protocol::{
    CreateTransfer, NewTransferItem, Node, NodeKind, TargetDevice, TransferTarget, UserId,
};

use crate::{
    components::ErrorAlert,
    icon::{Icon, platform_icon},
    load,
    local_files::{self, Picked},
    session::{self, Auth},
    uploads::{Sending, UploadItem, note_sent, start_send},
};

const TARGET_GROUP: &str = "send-target";

/// What the dialog sends.
#[derive(Clone)]
pub enum SendSource {
    /// Files and folders already on the server.
    Nodes(Vec<Node>),
    /// Files and folders picked on this computer.
    Local,
}

/// A device that can be picked, by its owner and id.
type Choice = (UserId, Uuid);

fn find(targets: &[TransferTarget], choice: Choice) -> Option<(TransferTarget, TargetDevice)> {
    let target = targets.iter().find(|target| target.user.id == choice.0)?;
    let device = target.devices.iter().find(|device| device.id == choice.1)?;
    Some((target.clone(), device.clone()))
}

/// The subtitle of a send of server items: their size, and their names when there are several.
fn nodes_subtitle(nodes: &[Node]) -> String {
    let bytes: i64 = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::File)
        .map(|node| node.size)
        .sum();
    let size = human_size(bytes);
    match nodes {
        [only] if only.kind == NodeKind::Folder => "Folder · everything in it".to_owned(),
        [_] => size,
        _ => format!("{} · {size}", names_line(&names(nodes))),
    }
}

fn names(nodes: &[Node]) -> Vec<String> {
    nodes.iter().map(|node| node.name.clone()).collect()
}

fn node_items(nodes: &[Node]) -> Vec<NewTransferItem> {
    nodes
        .iter()
        .map(|node| NewTransferItem {
            path: node.name.clone(),
            kind: node.kind,
            size: u64::try_from(node.size).unwrap_or_default(),
            node_id: Some(node.id),
            mime: None,
            modified_at: None,
        })
        .collect()
}

/// The entries of everything picked, for the transfer's manifest.
fn local_entries(picked: &[Picked]) -> Vec<LocalEntry> {
    picked
        .iter()
        .flat_map(|item| {
            let files = item.files.iter().map(|(path, file)| LocalEntry {
                path: path.clone(),
                kind: NodeKind::File,
                size: file.size() as u64,
            });
            let folders = item.empty_folders.iter().map(|path| LocalEntry {
                path: path.clone(),
                kind: NodeKind::Folder,
                size: 0,
            });
            files.chain(folders)
        })
        .collect()
}

/// The size line of a picked item: `220 KB`, or `124 files · 118 MB` for a folder.
fn picked_detail(item: &Picked) -> String {
    let size = human_size(i64::try_from(item.bytes()).unwrap_or(i64::MAX));
    if item.folder {
        format!("{} · {size}", files(item.files.len()))
    } else {
        size
    }
}

/// Asks where to send `source` and sends it; `on_close` runs when the dialog closes.
#[component]
pub fn SendDialog(source: SendSource, auth: Auth, on_close: Callback<()>) -> impl IntoView {
    let uploads = expect_context::<RwSignal<Vec<UploadItem>>>();
    let progress = expect_context::<Sending>();
    let targets = RwSignal::new(Vec::<TransferTarget>::new());
    let loaded = RwSignal::new(false);
    let chosen = RwSignal::new(None::<Choice>);
    let others = RwSignal::new(false);
    let picking = RwSignal::new(false);
    let picked = RwSignal::new_local(Vec::<Picked>::new());
    let error = RwSignal::new(None::<String>);
    let sending = RwSignal::new(false);
    let local = matches!(source, SendSource::Local);

    load::load(
        || async { session::client().transfer_targets().await },
        move |result| {
            loaded.set(true);
            match result {
                Ok(listed) => {
                    if chosen.get_untracked().is_none() {
                        let first = first_choice(&listed)
                            .map(|(target, device)| (target.user.id, device.id));
                        chosen.set(first);
                        picking.set(first.is_none());
                    }
                    targets.set(listed);
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        },
    );

    let current = Memo::new(move |_| {
        let choice = chosen.get()?;
        find(&targets.read(), choice)
    });
    let add = move |new: Vec<Picked>| {
        picked.update(|picked| {
            let mut taken: BTreeSet<String> = picked.iter().map(|item| item.name.clone()).collect();
            for mut item in new {
                let name = free_name(&item.name, &taken);
                if name != item.name {
                    item.rename(name.clone());
                }
                taken.insert(name);
                picked.push(item);
            }
        });
    };
    let on_drop = move |event: DragEvent| {
        event.prevent_default();
        event.stop_propagation();
        if !local {
            return;
        }
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        let entries = local_files::dropped_entries(&transfer);
        spawn_local(async move {
            match local_files::read_dropped(entries).await {
                Ok(new) => add(new),
                Err(_) => error.set(Some("Could not read what was dropped.".to_owned())),
            }
        });
    };
    let on_pick = move |input: HtmlInputElement| {
        if let Some(list) = input.files() {
            add(local_files::from_list(&list));
        }
        input.set_value("");
    };

    let (title, subtitle, what) = match &source {
        SendSource::Nodes(nodes) => (
            Signal::stored(send_title(&names(nodes))),
            Signal::stored(nodes_subtitle(nodes)),
            names_line(&names(nodes)),
        ),
        SendSource::Local => (
            Signal::stored("Send from this computer".to_owned()),
            Signal::derive(move || {
                let picked = picked.read();
                let folders = picked.iter().filter(|item| item.folder).count();
                let file_count = picked.iter().map(|item| item.files.len()).sum();
                let bytes = picked.iter().map(Picked::bytes).sum();
                if picked.is_empty() {
                    "Pick files or folders to send.".to_owned()
                } else {
                    items_summary(folders, file_count, bytes)
                }
            }),
            String::new(),
        ),
    };
    let nodes = match &source {
        SendSource::Nodes(nodes) => nodes.clone(),
        SendSource::Local => Vec::new(),
    };
    let can_send =
        move || !sending.get() && current.read().is_some() && (!local || !picked.read().is_empty());
    let send = move |_| {
        let Some((target, device)) = current.get_untracked() else {
            return;
        };
        let to = recipient_name(&target, &device);
        let (items, files, label) = if local {
            let picked = picked.get_untracked();
            let files: Vec<(String, File)> = picked
                .iter()
                .flat_map(|item| item.files.iter().cloned())
                .collect();
            let names: Vec<String> = picked.iter().map(|item| item.name.clone()).collect();
            (manifest(&local_entries(&picked)), files, names_line(&names))
        } else {
            (node_items(&nodes), Vec::new(), what.clone())
        };
        let request = CreateTransfer {
            recipient_id: target.user.id,
            recipient_device: device.id,
            items,
            message: None,
        };
        let label = format!("{label} → {to}");
        sending.set(true);
        spawn_local(async move {
            match session::client().create_transfer(&request).await {
                Ok(created) => {
                    if local {
                        start_send(uploads, progress, auth, label, created, files);
                    } else {
                        note_sent(uploads, label);
                    }
                    on_close.run(());
                }
                Err(err) => {
                    sending.set(false);
                    error.set(Some(auth.fail(&err)));
                }
            }
        });
    };
    let note = move || {
        current.get().map(|(target, device)| {
            if local {
                upload_note(&target, &device)
            } else {
                send_note(&target, &device)
            }
        })
    };
    let button = move || match current.get() {
        Some((target, device)) => format!("Send to {}", recipient_name(&target, &device)),
        None => "Send".to_owned(),
    };

    view! {
        <div class="modal" on:click=move |_| on_close.run(())>
            <div
                class="modal__panel modal__panel--wide send-dialog"
                role="dialog"
                aria-modal="true"
                aria-labelledby="send-title"
                on:click=|event| event.stop_propagation()
                on:dragover=move |event: DragEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                }
                on:drop=on_drop
            >
                <header class="modal__header">
                    <h2 id="send-title" class="modal__title">{title}</h2>
                    <p class="modal__subtitle">{subtitle}</p>
                </header>
                {local
                    .then(|| {
                        view! {
                            <section class="send-box">
                                <h3 class="send-box__title">"Items"</h3>
                                <ul class="send-items">
                                    <For
                                        each=move || picked.get()
                                        key=|item| item.name.clone()
                                        children=move |item| {
                                            let name = item.name.clone();
                                            let remove = name.clone();
                                            let label = format!("Remove {name}");
                                            view! {
                                                <li class="send-row">
                                                    <Icon name=if item.folder { "folder" } else { "description" } />
                                                    <span class="send-row__name">{name}</span>
                                                    <span class="send-row__detail">{picked_detail(&item)}</span>
                                                    <button
                                                        class="button button--ghost button--small"
                                                        aria-label=label
                                                        on:click=move |_| {
                                                            picked.update(|picked| picked.retain(|item| item.name != remove))
                                                        }
                                                    >
                                                        <Icon name="close" />
                                                    </button>
                                                </li>
                                            }
                                        }
                                    />
                                    <li class="send-row send-row--add">
                                        <Icon name="add" />
                                        <label class="send-link">
                                            "Add files…"
                                            <input
                                                type="file"
                                                multiple
                                                hidden
                                                on:change:target=move |event| on_pick(event.target())
                                            />
                                        </label>
                                        <label class="send-link">
                                            "Add a folder…"
                                            <input
                                                type="file"
                                                hidden
                                                prop:webkitdirectory=true
                                                on:change:target=move |event| on_pick(event.target())
                                            />
                                        </label>
                                        <span class="send-row__detail send-row__hint hint-drop">"or drop them here"</span>
                                    </li>
                                </ul>
                            </section>
                            <section class="send-box">
                                <Show
                                    when=move || picking.get()
                                    fallback=move || {
                                        view! {
                                            <h3 class="send-box__title">"To"</h3>
                                            <button
                                                class="send-row send-row--button"
                                                aria-expanded="false"
                                                on:click=move |_| picking.set(true)
                                            >
                                                {move || {
                                                    current
                                                        .get()
                                                        .map(|(target, device)| {
                                                            view! {
                                                                <Icon name=platform_icon(&device.platform) />
                                                                <span class="send-row__name">
                                                                    {recipient_name(&target, &device)}
                                                                </span>
                                                                <span class="send-row__detail">
                                                                    {device_status(&device)}
                                                                </span>
                                                            }
                                                        })
                                                }}
                                                <Icon name="expand_more" />
                                            </button>
                                        }
                                    }
                                >
                                    <DevicePicker targets loaded chosen others on_pick=Callback::new(move |()| picking.set(false)) />
                                </Show>
                            </section>
                        }
                    })}
                {(!local)
                    .then(|| {
                        view! {
                            <section class="send-box">
                                <DevicePicker targets loaded chosen others on_pick=Callback::new(|()| {}) />
                            </section>
                        }
                    })}
                {move || note().map(|note| view! { <p class="modal__note">{note}</p> })}
                <ErrorAlert message=error />
                <div class="modal__actions">
                    <button class="button" on:click=move |_| on_close.run(())>
                        "Cancel"
                    </button>
                    <button class="button button--primary" disabled=move || !can_send() on:click=send>
                        <Icon name="send" />
                        {button}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// The devices to send to: the user's own, or, a step further, other users' devices.
#[component]
fn DevicePicker(
    targets: RwSignal<Vec<TransferTarget>>,
    loaded: RwSignal<bool>,
    chosen: RwSignal<Option<Choice>>,
    others: RwSignal<bool>,
    on_pick: Callback<()>,
) -> impl IntoView {
    let has_others = move || !pickable(&targets.read(), false).is_empty();
    let device_row = move |target: &TransferTarget, device: &TargetDevice, nested: bool| {
        let choice = (target.user.id, device.id);
        let selected = move || chosen.get() == Some(choice);
        view! {
            <label
                class="send-row send-row--choice"
                class:send-row--nested=nested
                class:send-row--selected=selected
            >
                <Icon name=platform_icon(&device.platform) />
                <span class="send-row__name">{device.name.clone()}</span>
                <span class="send-row__detail">{device_status(device)}</span>
                <input
                    type="radio"
                    class="choice__radio"
                    name=TARGET_GROUP
                    prop:checked=selected
                    on:change=move |_| {
                        chosen.set(Some(choice));
                        on_pick.run(());
                    }
                />
            </label>
        }
    };

    view! {
        <h3 class="send-box__title">
            {move || if others.get() { "Another user’s device" } else { "Your devices" }}
        </h3>
        <div class="send-items" role="radiogroup">
            {move || {
                let targets = targets.read();
                if others.get() {
                    let groups = pickable(&targets, false)
                        .into_iter()
                        .map(|(target, devices)| {
                            view! {
                                <p class="send-row send-row--user">
                                    <Icon name="person" />
                                    {target.user.username.clone()}
                                </p>
                                {devices
                                    .into_iter()
                                    .map(|device| device_row(target, device, true))
                                    .collect_view()}
                            }
                        })
                        .collect_view();
                    view! {
                        <button class="send-row send-row--button" on:click=move |_| others.set(false)>
                            <Icon name="arrow_back" />
                            <span class="send-row__name">"Back to your devices"</span>
                        </button>
                        {groups}
                    }
                        .into_any()
                } else {
                    let own = pickable(&targets, true);
                    let rows = own
                        .into_iter()
                        .flat_map(|(target, devices)| {
                            devices.into_iter().map(move |device| device_row(target, device, false))
                        })
                        .collect_view();
                    let empty = loaded.get() && pickable(&targets, true).is_empty();
                    view! {
                        {rows}
                        {empty
                            .then(|| {
                                view! {
                                    <p class="send-row send-row--empty">
                                        "None of your other devices can receive files. Sign in on one, or check its visibility in Settings › Devices."
                                    </p>
                                }
                            })}
                        <Show when=has_others>
                            <button class="send-row send-row--button" on:click=move |_| others.set(true)>
                                <Icon name="group" />
                                <span class="send-row__name">"Another user’s device…"</span>
                                <Icon name="chevron_right" />
                            </button>
                        </Show>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use protocol::NodeId;
    use time::OffsetDateTime;

    use super::*;

    fn node(name: &str, kind: NodeKind, size: i64) -> Node {
        Node {
            id: NodeId(Uuid::nil()),
            parent_id: None,
            kind,
            name: name.to_owned(),
            size,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            edited_on: None,
        }
    }

    #[test]
    fn server_items_are_sent_by_id_under_their_own_name() {
        let nodes = [
            node("report.pdf", NodeKind::File, 1536),
            node("Photos", NodeKind::Folder, 0),
        ];
        let items = node_items(&nodes);
        assert_eq!(items[0].path, "report.pdf");
        assert_eq!(items[0].node_id, Some(nodes[0].id));
        assert_eq!(items[1].kind, NodeKind::Folder);
        assert_eq!(nodes_subtitle(&nodes[..1]), "1.5 KB");
        assert_eq!(nodes_subtitle(&nodes[1..]), "Folder · everything in it");
        assert_eq!(nodes_subtitle(&nodes), "report.pdf, Photos · 1.5 KB");
    }
}
