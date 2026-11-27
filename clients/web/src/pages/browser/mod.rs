mod breadcrumbs;
mod details;
mod file_drop;
mod listing;
mod node_table;
mod toolbar;

use std::collections::HashSet;

use leptos::{html::Input, prelude::*, task::spawn_local};
use web_sys::File;

use client_core::{labels::HOME, protected::seal_name};
use protocol::{
    Node, NodeId, NodeKind, ProtectedFolder, ProtectionState, TicketRequest, TrashItem,
};

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
    protect_dialog::{FolderPasswordDialog, PasswordAction, ProtectDialog},
    send_dialog::{SendDialog, SendSource},
    share_dialog::ShareDialog,
    unprotect_dialog::{BinnedDialog, UnprotectDialog},
};
use crate::{
    components::{ErrorAlert, reconfirm::Reconfirm},
    icon::Icon,
    protected::{self as vault_files, Direction, use_vault},
    session::{self, Auth},
    uploads::{Encryption, UploadItem, start_upload},
};

const LOCKED_MEANWHILE: &str = "The folder locked meanwhile; unlock it again.";
/// How long a decrypted download's `blob:` URL lives once the browser has it.
const DOWNLOAD_URL_MS: u32 = 60_000;

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
        protection,
        locked,
        ..
    } = listing;
    let vault = use_vault();
    let reconfirm = Reconfirm::get();
    let inside_protected = Signal::derive(move || protection.read().is_some());
    let lockable = Signal::derive(move || inside_protected.get() && !locked.get());
    // A folder being unprotected takes nothing new: what it would seal could be left sealed.
    let frozen = Signal::derive(move || {
        protection
            .read()
            .as_ref()
            .is_some_and(|folder| folder.state == ProtectionState::Unprotecting)
    });
    // Lock all, when a folder other than the open one is unlocked.
    let others_unlocked = Signal::derive(move || {
        vault.unlocked_count() > usize::from(lockable.get())
    });
    let protecting = RwSignal::new(None::<Node>);
    // Where the folder being protected is, as the recycle bin writes it: `/Photos/2024`.
    let protecting_path = StoredValue::new(String::new());
    let binned = RwSignal::new(None::<(String, Vec<TrashItem>)>);
    let unprotecting = RwSignal::new(None::<Node>);
    let password_dialog = RwSignal::new(None::<(ProtectedFolder, String, PasswordAction)>);
    // Inside a protected folder a name is sealed for its parent before it is sent.
    let seal = move |parent: NodeId, kind: NodeKind, name: &str| -> Result<String, String> {
        match protection.get_untracked() {
            None => Ok(name.to_owned()),
            Some(inside) => vault
                .with_keys(inside.node_id, |keys| {
                    seal_name(keys, parent, kind, name).map_err(|err| err.to_string())
                })
                .unwrap_or_else(|| Err(LOCKED_MEANWHILE.to_owned())),
        }
    };
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
    let sharing = RwSignal::new(None::<Vec<Node>>);

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
            let parent = folder.get_untracked();
            let sealed = parent.map_or_else(
                || Ok(name.clone()),
                |parent| seal(parent, NodeKind::Folder, &name),
            );
            let name = match sealed {
                Ok(name) => name,
                Err(message) => return listing.fail_with(message),
            };
            saving.set(true);
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
        let Some(node) = nodes
            .read_untracked()
            .iter()
            .find(|node| node.id == id)
            .cloned()
        else {
            return cancel_rename.run(());
        };
        if name.is_empty() || node.name == name {
            cancel_rename.run(());
        } else if !saving.get_untracked() {
            let sealed = node
                .parent_id
                .map_or_else(|| Ok(name.clone()), |parent| seal(parent, node.kind, &name));
            let name = match sealed {
                Ok(name) => name,
                Err(message) => return listing.fail_with(message),
            };
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
    let share = Callback::new(move |nodes: Vec<Node>| sharing.set(Some(nodes)));
    let share_selected = Callback::new(move |()| share.run(selected_nodes.get_untracked()));
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
        (current.is_none() || current != lost_and_found.get()) && !locked.get() && !frozen.get()
    });
    let download = Callback::new(move |node: Node| {
        if let Some(inside) = protection.get_untracked() {
            spawn_local(async move {
                let plain =
                    vault_files::download_plain(vault, inside.node_id, node.id, node.size).await;
                match plain.and_then(|plain| save_plain(&plain, &node.name)) {
                    Ok(()) => {}
                    Err(message) => listing.fail_with(message),
                }
            });
            return;
        }
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
                Err(err) => listing.fail(&err),
            }
        });
    });
    let protect = Callback::new(move |node: Node| {
        let path: String = crumbs
            .read_untracked()
            .iter()
            .chain(std::iter::once(&node))
            .map(|folder| format!("/{}", folder.name))
            .collect();
        protecting_path.set_value(path);
        protecting.set(Some(node));
    });
    let on_protected = Callback::new(move |protected: ProtectedFolder| {
        if protected.state == ProtectionState::Protecting {
            vault.convert(protected.node_id, Direction::Protect, move || {
                listing.refetch()
            });
        }
        listing.refetch();
        // Its files binned before are plain and outside it now: offer to delete them for good.
        let path = protecting_path.get_value();
        spawn_local(async move {
            if let Ok(items) = vault_files::binned_from(&path).await
                && !items.is_empty()
            {
                let name = path.rsplit('/').next().unwrap_or_default().to_owned();
                binned.set(Some((name, items)));
            }
        });
    });
    let ask_password = move |node: Node, action: PasswordAction| {
        if let Some(folder) = vault.protection_now(node.id) {
            password_dialog.set(Some((folder, node.name, action)));
        }
    };
    let unlock = Callback::new(move |node: Node| ask_password(node, PasswordAction::Unlock));
    let change_password =
        Callback::new(move |node: Node| ask_password(node, PasswordAction::Change));
    let lock = Callback::new(move |id: NodeId| vault.lock(id));
    let lock_all = Callback::new(move |()| vault.lock_all());
    let unprotect = Callback::new(move |node: Node| unprotecting.set(Some(node)));
    let confirm_unprotect = Callback::new(move |node: Node| {
        spawn_local(async move {
            let started = vault
                .unprotect(reconfirm, node.id, move || listing.refetch())
                .await;
            if let Err(message) = started {
                listing.fail_with(message);
            }
            listing.refetch();
        });
    });
    let lock_open = Callback::new(move |()| {
        if let Some(inside) = protection.get_untracked() {
            vault.lock(inside.node_id);
        }
    });
    // Goes on with protecting or unprotecting `id`, whichever it stopped in.
    let finish = Callback::new(move |id: NodeId| {
        let direction = match vault.protection_now(id).map(|folder| folder.state) {
            Some(ProtectionState::Protecting) => Direction::Protect,
            Some(ProtectionState::Unprotecting) => Direction::Unprotect,
            _ => return,
        };
        vault.convert(id, direction, move || listing.refetch());
    });
    // The protected folder the open folder is in, by its node in the path.
    let protected_crumb = move || {
        let inside = protection.get_untracked()?;
        crumbs
            .read_untracked()
            .iter()
            .find(|node| node.id == inside.node_id)
            .cloned()
    };
    let unlock_open = move |_| {
        if let Some(node) = protected_crumb() {
            unlock.run(node);
        }
    };
    let conversion = move || {
        let inside = protection
            .read()
            .as_ref()
            .map(|folder| (folder.node_id, folder.state))?;
        match vault.converting.get() {
            Some(conversion) if conversion.folder == inside.0 => {
                Some((conversion.message, conversion.failed))
            }
            _ if locked.get() => None,
            _ if inside.1 == ProtectionState::Protecting => Some((
                "This folder is still being protected: some of its files may not be encrypted yet."
                    .to_owned(),
                true,
            )),
            _ if inside.1 == ProtectionState::Unprotecting => Some((
                "This folder is still being unprotected: some of its files may still be encrypted."
                    .to_owned(),
                true,
            )),
            _ => None,
        }
    };
    let show_details = Callback::new(move |node: Node| {
        selected.set(HashSet::from([node.id]));
        details.set(Some(node.id));
    });
    // The send dialog takes files dropped on it; the page takes them otherwise.
    let can_drop = Signal::derive(move || can_add.get() && send_from.read().is_none());
    let upload = Callback::new(move |files: Vec<File>| {
        if !can_drop.get_untracked() {
            return;
        }
        let parent = folder.get_untracked();
        let encryption = match (protection.get_untracked(), parent) {
            (Some(inside), Some(parent)) => {
                let names: Result<Vec<String>, String> = files
                    .iter()
                    .map(|file| seal(parent, NodeKind::File, &file.name()))
                    .collect();
                match (names, vault.recipient(inside.node_id)) {
                    (Ok(names), Some(recipient)) => Some(Encryption { recipient, names }),
                    (Err(message), _) => return listing.fail_with(message),
                    (Ok(_), None) => return listing.fail_with(LOCKED_MEANWHILE.to_owned()),
                }
            }
            _ => None,
        };
        start_upload(
            uploads,
            move || listing.refetch(),
            auth,
            parent,
            files,
            encryption,
        );
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
        share,
        details: show_details,
        delete,
        lost_and_found,
        inside_protected,
        frozen,
        protect,
        unprotect,
        unlock,
        lock,
        change_password,
        finish,
    };

    view! {
        <main class="explorer">
                <div class="explorer__header">
                    <Breadcrumbs crumbs />
                    <Toolbar
                        selected_count
                        on_move=move_selected
                        on_send=send_selected
                        on_share=share_selected
                        on_send_local=send_local
                        on_delete=delete_selected
                        can_delete
                        can_add
                        on_new_folder=new_folder
                        on_upload=upload
                        inside_protected
                        frozen
                        lockable
                        on_lock=lock_open
                        others_unlocked
                        on_lock_all=lock_all
                    />
                </div>
                <ErrorAlert message=error />
                <div class="explorer__body" class:explorer__body--with-details=move || details_target.get().is_some()>
                    <div class="explorer__nodes">
                        {move || {
                            conversion()
                                .map(|(message, failed)| {
                                    view! {
                                        <p
                                            class="alert explorer__banner"
                                            class:alert--warning=!failed
                                            class:alert--error=failed
                                            role="status"
                                        >
                                            {message}
                                        </p>
                                    }
                                })
                        }}
                        <Show
                            when=move || !locked.get()
                            fallback=move || {
                                view! {
                                    <div class="explorer__locked">
                                        <Icon name="lock" />
                                        <p>"This folder is protected. Unlock it to see what it holds."</p>
                                        <button class="button button--primary" on:click=unlock_open>
                                            "Unlock…"
                                        </button>
                                    </div>
                                }
                            }
                        >
                        <NodeTable nodes=sorted sort actions draft_input />
                        </Show>
                        <Show when=move || !locked.get() && nodes.read().is_empty() && error.read().is_none() && draft.read().is_none()>
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
                .map(|nodes| {
                    let within = protection
                        .get_untracked()
                        .map(|folder| (folder, crumbs.get_untracked()));
                    view! {
                        <MoveDialog nodes moving selected refresh=move || listing.refetch() auth within />
                    }
                })
        }}
        {move || {
            send_from
                .get()
                .map(|source| {
                    view! { <SendDialog source auth on_close=Callback::new(move |()| send_from.set(None)) /> }
                })
        }}
        {move || {
            sharing
                .get()
                .map(|nodes| {
                    view! { <ShareDialog nodes auth on_close=Callback::new(move |()| sharing.set(None)) /> }
                })
        }}
        {move || {
            deleting
                .get()
                .map(|nodes| {
                    let for_good = inside_protected.get_untracked()
                        || nodes.iter().any(|node| vault.protection_now(node.id).is_some());
                    view! { <DeleteDialog nodes deleting on_confirm=confirm_delete for_good /> }
                })
        }}
        {move || {
            protecting
                .get()
                .map(|node| {
                    view! {
                        <ProtectDialog
                            node
                            on_close=Callback::new(move |()| protecting.set(None))
                            on_protected
                        />
                    }
                })
        }}
        {move || {
            unprotecting
                .get()
                .map(|node| {
                    view! {
                        <UnprotectDialog
                            node
                            on_close=Callback::new(move |()| unprotecting.set(None))
                            on_confirm=confirm_unprotect
                        />
                    }
                })
        }}
        {move || {
            // After the Protect dialog, which shows the recovery key first.
            binned
                .get()
                .filter(|_| protecting.read().is_none())
                .map(|(name, items)| {
                    view! {
                        <BinnedDialog name items on_close=Callback::new(move |()| binned.set(None)) />
                    }
                })
        }}
        {move || {
            password_dialog
                .get()
                .map(|(folder, name, action)| {
                    view! {
                        <FolderPasswordDialog
                            folder
                            name
                            action
                            on_close=Callback::new(move |()| password_dialog.set(None))
                        />
                    }
                })
        }}
    }
}

/// Hands `plain`, a decrypted file, to the browser as a download named `name`.
fn save_plain(plain: &[u8], name: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let failed = || "The file could not be saved.".to_owned();
    let url = vault_files::blob_url(plain, name).ok_or_else(failed)?;
    let anchor: web_sys::HtmlAnchorElement = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.create_element("a").ok())
        .and_then(|element| element.dyn_into().ok())
        .ok_or_else(failed)?;
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();
    gloo_timers::callback::Timeout::new(DOWNLOAD_URL_MS, move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    })
    .forget();
    Ok(())
}
