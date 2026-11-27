//! Unprotecting a folder, and deleting for good the files a folder had in the recycle bin from
//! before it was protected.

use leptos::{prelude::*, task::spawn_local};

use protocol::{Node, TrashItem};

use crate::{components::ErrorAlert, protected::MAX_IN_MEMORY_LABEL, session};

/// Asks before unprotecting `node`; `on_confirm` starts it.
#[component]
pub fn UnprotectDialog(
    node: Node,
    on_close: Callback<()>,
    on_confirm: Callback<Node>,
) -> impl IntoView {
    let target = StoredValue::new(node.clone());
    view! {
        <div class="modal" on:click=move |_| on_close.run(())>
            <div
                class="modal__panel"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="unprotect-title"
                on:click=|event| event.stop_propagation()
            >
                <header class="modal__header">
                    <h2 class="modal__title" id="unprotect-title">
                        {format!("Unprotect “{}”?", node.name)}
                    </h2>
                    <p class="modal__subtitle">
                        "Its files and names are decrypted in this browser and stored on the server in plain again, where the server can read them."
                    </p>
                </header>
                <p class="alert alert--warning">
                    "The previous versions of the files in this folder will be deleted."
                </p>
                <p class="modal__note">
                    {format!("Keep this page open until it is done. Files larger than {MAX_IN_MEMORY_LABEL} cannot be decrypted in the browser; use the desktop app for those.")}
                </p>
                <div class="modal__actions">
                    <button class="button" on:click=move |_| on_close.run(())>
                        "Cancel"
                    </button>
                    <button
                        class="button button--danger"
                        on:click=move |_| {
                            on_close.run(());
                            on_confirm.run(target.get_value());
                        }
                    >
                        "Unprotect"
                    </button>
                </div>
            </div>
        </div>
    }
}

/// Offers to delete for good `items`, the files the folder `name` had in the recycle bin from
/// before it was protected: they are still plain there.
#[component]
pub fn BinnedDialog(name: String, items: Vec<TrashItem>, on_close: Callback<()>) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let count = items.len();
    let ids: Vec<_> = items.iter().map(|item| item.node.id).collect();
    let purge = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        let ids = ids.clone();
        spawn_local(async move {
            let client = session::client();
            for id in ids {
                if let Err(err) = client.purge_trashed(id).await {
                    busy.set(false);
                    error.set(Some(err.to_string()));
                    return;
                }
            }
            busy.set(false);
            on_close.run(());
        });
    };
    let files = if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    };
    view! {
        <div class="modal">
            <div
                class="modal__panel"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="binned-title"
            >
                <header class="modal__header">
                    <h2 class="modal__title" id="binned-title">
                        {format!("Delete {files} of “{name}” from the recycle bin?")}
                    </h2>
                    <p class="modal__subtitle">
                        {format!("{files} deleted from this folder before it was protected are still in the recycle bin, unencrypted. Deleting them for good removes them and their previous versions.")}
                    </p>
                </header>
                <ul class="modal__list">
                    {items
                        .into_iter()
                        .take(10)
                        .map(|item| view! { <li>{item.original_path}</li> })
                        .collect_view()}
                    {(count > 10).then(|| view! { <li>{format!("and {} more", count - 10)}</li> })}
                </ul>
                <ErrorAlert message=error />
                <div class="modal__actions">
                    <button class="button" disabled=move || busy.get() on:click=move |_| on_close.run(())>
                        "Keep them"
                    </button>
                    <button class="button button--danger" disabled=move || busy.get() on:click=purge>
                        {move || if busy.get() { "Deleting…" } else { "Delete for good" }}
                    </button>
                </div>
            </div>
        </div>
    }
}
