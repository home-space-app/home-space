use std::{collections::HashMap, rc::Rc};

use leptos::{prelude::*, task::spawn_local};
use uuid::Uuid;
use web_sys::File;

use protocol::{CreatedTransfer, NodeId};

use crate::{
    session::{self, Auth},
    upload,
};

#[derive(Clone, PartialEq)]
enum UploadState {
    Queued,
    Active,
    Done,
    Failed(String),
}

#[derive(Clone)]
pub struct UploadItem {
    id: u64,
    name: String,
    progress: f64,
    state: UploadState,
}

fn update_upload(
    uploads: RwSignal<Vec<UploadItem>>,
    id: u64,
    change: impl FnOnce(&mut UploadItem),
) {
    uploads.update(|items| {
        if let Some(item) = items.iter_mut().find(|item| item.id == id) {
            change(item);
        }
    });
}

pub fn start_upload(
    uploads: RwSignal<Vec<UploadItem>>,
    on_done: impl Fn() + 'static,
    auth: Auth,
    parent: Option<NodeId>,
    files: Vec<File>,
) {
    let ids: Vec<u64> = files.iter().map(|_| upload::next_id()).collect();
    uploads.update(|items| {
        items.extend(ids.iter().zip(&files).map(|(id, file)| UploadItem {
            id: *id,
            name: file.name(),
            progress: 0.0,
            state: UploadState::Queued,
        }));
    });
    spawn_local(async move {
        for (id, file) in ids.into_iter().zip(files) {
            update_upload(uploads, id, |item| item.state = UploadState::Active);
            let on_progress: upload::OnProgress = Rc::new(move |fraction| {
                update_upload(uploads, id, |item| item.progress = fraction)
            });
            match upload::upload(&session::client(), &file, parent, &on_progress).await {
                Ok(_) => update_upload(uploads, id, |item| {
                    item.progress = 1.0;
                    item.state = UploadState::Done;
                }),
                Err(err) => {
                    let message = auth.fail(&err);
                    update_upload(uploads, id, |item| {
                        item.state = UploadState::Failed(message)
                    });
                }
            }
        }
        on_done();
    });
}

/// How far each transfer this tab is uploading has got, between 0 and 1, by transfer id.
#[derive(Clone, Copy)]
pub struct Sending(pub RwSignal<HashMap<Uuid, f64>>);

/// Adds a finished row for something sent without uploading, such as files already on the
/// server.
pub fn note_sent(uploads: RwSignal<Vec<UploadItem>>, name: String) {
    uploads.update(|items| {
        items.push(UploadItem {
            id: upload::next_id(),
            name,
            progress: 1.0,
            state: UploadState::Done,
        });
    });
}

/// Uploads the files of a created transfer, `files` by their path in it, as one row of the
/// uploads panel named `name`; `sending` follows its progress until it ends.
pub fn start_send(
    uploads: RwSignal<Vec<UploadItem>>,
    sending: Sending,
    auth: Auth,
    name: String,
    created: CreatedTransfer,
    files: Vec<(String, File)>,
) {
    let id = upload::next_id();
    let transfer = created.id;
    let mut by_path: HashMap<String, File> = files.into_iter().collect();
    let work: Vec<(Uuid, File)> = created
        .items
        .iter()
        .filter(|item| item.upload_needed)
        .filter_map(|item| Some((item.id, by_path.remove(&item.path)?)))
        .collect();
    let total = work
        .iter()
        .map(|(_, file)| file.size())
        .sum::<f64>()
        .max(1.0);
    uploads.update(|items| {
        items.push(UploadItem {
            id,
            name,
            progress: 0.0,
            state: UploadState::Active,
        });
    });
    let report = move |fraction: f64| {
        update_upload(uploads, id, |item| item.progress = fraction);
        sending.0.update(|sends| {
            sends.insert(transfer, fraction);
        });
    };
    report(0.0);
    spawn_local(async move {
        let client = session::client();
        let mut done = 0.0;
        let mut outcome = Ok(());
        for (item, file) in work {
            let size = file.size();
            let start = done;
            let on_progress: upload::OnProgress =
                Rc::new(move |fraction| report((start + fraction * size) / total));
            outcome = upload::upload_transfer_item(&client, &file, item, &on_progress).await;
            if outcome.is_err() {
                break;
            }
            done += size;
        }
        sending.0.update(|sends| {
            sends.remove(&transfer);
        });
        match outcome {
            Ok(()) => update_upload(uploads, id, |item| {
                item.progress = 1.0;
                item.state = UploadState::Done;
            }),
            Err(err) => {
                let message = auth.fail(&err);
                update_upload(uploads, id, |item| {
                    item.state = UploadState::Failed(message)
                });
            }
        }
    });
}

fn is_pending(item: &UploadItem) -> bool {
    matches!(item.state, UploadState::Queued | UploadState::Active)
}

fn summarize(items: &[UploadItem]) -> String {
    let total = items.len();
    let pending = items.iter().filter(|item| is_pending(item)).count();
    let failed = items
        .iter()
        .filter(|item| matches!(item.state, UploadState::Failed(_)))
        .count();
    if pending > 0 {
        format!("Uploading {} of {total}", total - pending + 1)
    } else if failed > 0 {
        format!("{failed} failed, {} uploaded", total - failed)
    } else {
        format!("{total} uploaded")
    }
}

fn overall_progress(items: &[UploadItem]) -> f64 {
    if items.is_empty() {
        0.0
    } else {
        items.iter().map(|item| item.progress).sum::<f64>() / items.len() as f64
    }
}

#[component]
pub fn UploadPanel(uploads: RwSignal<Vec<UploadItem>>) -> impl IntoView {
    let minimized_at = RwSignal::new(None::<u64>);
    let idle = move || !uploads.read().iter().any(is_pending);
    let has_done = move || {
        uploads
            .read()
            .iter()
            .any(|item| item.state == UploadState::Done)
    };
    let newest = Memo::new(move |_| {
        uploads
            .read()
            .iter()
            .map(|item| item.id)
            .max()
            .unwrap_or_default()
    });
    let minimized = Memo::new(move |_| minimized_at.get().is_some_and(|at| newest.get() <= at));

    view! {
        <Show when=move || !uploads.read().is_empty()>
            <section class="upload-panel" role="region" aria-label="Uploads">
                <header class="upload-panel__header">
                    <span class="upload-panel__summary" aria-live="polite">
                        {move || summarize(&uploads.read())}
                    </span>
                    <button
                        class="button button--ghost button--small"
                        disabled=move || !has_done()
                        on:click=move |_| {
                            uploads.update(|items| items.retain(|item| item.state != UploadState::Done))
                        }
                    >
                        "Clear completed"
                    </button>
                    <button
                        class="button button--ghost button--small upload-panel__icon"
                        aria-label=move || if minimized.get() { "Expand uploads" } else { "Minimize uploads" }
                        aria-expanded=move || (!minimized.get()).to_string()
                        on:click=move |_| {
                            minimized_at.set((!minimized.get_untracked()).then(|| newest.get_untracked()))
                        }
                    >
                        {move || if minimized.get() { "▴" } else { "▾" }}
                    </button>
                    <Show when=idle>
                        <button
                            class="button button--ghost button--small upload-panel__icon"
                            aria-label="Close uploads"
                            on:click=move |_| uploads.set(Vec::new())
                        >
                            "✕"
                        </button>
                    </Show>
                </header>
                <progress
                    class="upload-panel__overall"
                    max="1"
                    value=move || overall_progress(&uploads.read())
                ></progress>
                <ul class="upload-panel__list" hidden=move || minimized.get()>
                    {move || {
                        uploads
                            .get()
                            .into_iter()
                            .map(|item| {
                                let (status, modifier) = match &item.state {
                                    UploadState::Queued => ("Waiting".to_owned(), "queued"),
                                    UploadState::Active => ("Uploading".to_owned(), "active"),
                                    UploadState::Done => ("Done".to_owned(), "done"),
                                    UploadState::Failed(message) => {
                                        (format!("Failed: {message}"), "failed")
                                    }
                                };
                                view! {
                                    <li class=format!(
                                        "upload-panel__item upload-panel__item--{modifier}",
                                    )>
                                        <span class="upload-panel__name">{item.name}</span>
                                        <progress
                                            class="upload-panel__progress"
                                            max="1"
                                            value=item.progress
                                        ></progress>
                                        <span class="upload-panel__status">{status}</span>
                                    </li>
                                }
                            })
                            .collect_view()
                    }}
                </ul>
            </section>
        </Show>
    }
}
