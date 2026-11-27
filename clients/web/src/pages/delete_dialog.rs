use leptos::prelude::*;

use protocol::Node;

#[component]
pub fn DeleteDialog(
    nodes: Vec<Node>,
    deleting: RwSignal<Option<Vec<Node>>>,
    on_confirm: Callback<Vec<Node>>,
    /// The items are in a protected folder, or are one: they skip the recycle bin.
    #[prop(optional)]
    for_good: bool,
) -> impl IntoView {
    let title = match (nodes.as_slice(), for_good) {
        ([node], false) => format!("Move “{}” to recycle bin?", node.name),
        (_, false) => format!("Move {} items to recycle bin?", nodes.len()),
        ([node], true) => format!("Delete “{}” for good?", node.name),
        (_, true) => format!("Delete {} items for good?", nodes.len()),
    };
    let confirm = move |_| {
        if let Some(targets) = deleting.get_untracked() {
            on_confirm.run(targets);
        }
        deleting.set(None);
    };

    view! {
        <div class="modal" on:click=move |_| deleting.set(None)>
            <div
                class="modal__panel"
                role="alertdialog"
                aria-modal="true"
                on:click=|event| event.stop_propagation()
            >
                <header class="modal__header">
                    <h2 class="modal__title">{title}</h2>
                    {for_good
                        .then(|| {
                            view! {
                                <p class="modal__subtitle">
                                    "What is in a protected folder skips the recycle bin, with its old versions. It cannot be restored."
                                </p>
                            }
                        })}
                </header>
                <div class="modal__actions">
                    <button class="button" on:click=move |_| deleting.set(None)>
                        "Cancel"
                    </button>
                    <button class="button button--primary" on:click=confirm>
                        {if for_good { "Delete for good" } else { "Move to bin" }}
                    </button>
                </div>
            </div>
        </div>
    }
}
