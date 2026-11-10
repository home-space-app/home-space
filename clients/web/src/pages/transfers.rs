//! Files sent to the user's devices, and the ones the user sent.

use leptos::{prelude::*, task::spawn_local};
use time::OffsetDateTime;

use client_core::transfers::{
    is_single_file, time_left, transfer_from, transfer_name, transfer_size, transfer_state,
    transfer_to,
};
use protocol::{Event, TransferState, TransferSummary, UserId};

use super::{
    shell::{Tab, Tabs},
    transfer_save::SaveTransferDialog,
};
use crate::{
    components::ErrorAlert,
    events,
    icon::Icon,
    incoming::{Incoming, percent},
    load,
    locale::{self, short_date},
    route, save_file,
    session::{self, Auth},
    uploads::Sending,
};

/// How often the time left in each row is brought up to date.
const CLOCK_TICK_MS: u32 = 60_000;

/// Which list the page shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TransferList {
    Incoming,
    Sent,
}

/// The icon of a transfer: a file, or a folder for several items.
fn transfer_icon(transfer: &TransferSummary) -> &'static str {
    if is_single_file(transfer) {
        "description"
    } else {
        "folder"
    }
}

fn me(auth: Auth) -> UserId {
    auth.user
        .get_untracked()
        .map(|me| me.id)
        .unwrap_or(UserId(uuid::Uuid::nil()))
}

#[component]
fn TransfersHeader(refresh: Callback<()>) -> impl IntoView {
    view! {
        <header class="page-header">
            <h1 class="page-title">"Transfers"</h1>
            <span class="page-header__spacer"></span>
            <button class="button" on:click=move |_| refresh.run(())>
                <Icon name="refresh" />
                "Refresh"
            </button>
        </header>
        <Tabs>
            <Tab href=route::TRANSFERS_PATH icon="download" label="Incoming" />
            <Tab href=route::TRANSFERS_SENT_PATH icon="upload" label="Sent" />
        </Tabs>
    }
}

#[component]
pub fn TransfersPage(auth: Auth, list: TransferList) -> impl IntoView {
    let transfers = RwSignal::new(Vec::<TransferSummary>::new());
    let error = RwSignal::new(None::<String>);
    let loaded = RwSignal::new(false);
    let saving = RwSignal::new(None::<TransferSummary>);
    let incoming = expect_context::<Incoming>();
    // Ticks so the time left in each row stays current.
    let now = RwSignal::new(locale::now());
    let ticker = StoredValue::new_local(Some(gloo_timers::callback::Interval::new(
        CLOCK_TICK_MS,
        move || now.set(locale::now()),
    )));
    on_cleanup(move || ticker.update_value(|ticker| drop(ticker.take())));

    let resource = load::load(
        move || async move {
            match list {
                TransferList::Incoming => session::client().incoming_transfers(false).await,
                TransferList::Sent => session::client().outgoing_transfers().await,
            }
        },
        move |result| {
            loaded.set(true);
            now.set(locale::now());
            match result {
                Ok(listed) => {
                    transfers.set(listed);
                    error.set(None);
                }
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        },
    );
    let refresh = Callback::new(move |()| resource.refetch());
    // A send this tab finished uploading is now waiting for its device.
    let sending = expect_context::<Sending>();
    let uploading = Memo::new(move |_| sending.0.read().len());
    Effect::new(move |before: Option<usize>| {
        let now = uploading.get();
        if before.is_some_and(|before| now < before) {
            resource.refetch();
        }
        now
    });
    events::use_events(move |event| {
        if matches!(
            event,
            Event::TransferReceived { .. } | Event::TransferChanged { .. } | Event::Resync
        ) {
            resource.refetch();
        }
    });

    let act = move |action: Action, transfer: TransferSummary| {
        let id = transfer.id;
        if action == Action::Download {
            incoming.download(auth, transfer);
            return;
        }
        spawn_local(async move {
            let client = session::client();
            let result = match action {
                Action::Decline => client.decline_transfer(id).await,
                Action::Cancel => client.cancel_transfer(id).await,
                // The user says the download is on this computer, so the server's copy can go.
                Action::Delete => client.confirm_transfer(id).await,
                Action::Download => Ok(()),
            };
            if let Err(err) = result {
                error.set(Some(auth.fail(&err)));
            }
            resource.refetch();
        });
    };
    let act = Callback::new(move |(action, transfer)| act(action, transfer));
    let me = me(auth);
    let empty = match list {
        TransferList::Incoming => "Nothing has been sent to your devices.",
        TransferList::Sent => "You have not sent anything yet.",
    };
    let note = match list {
        TransferList::Incoming if save_file::supported() => {
            "Several files or a folder download as one zip. The server deletes its copy once the download is complete."
        }
        TransferList::Incoming => {
            "Several files or a folder download as one zip. This browser does not say when a download is complete: delete the transfer afterwards, or leave it until it expires."
        }
        TransferList::Sent => {
            "Close (×) cancels a transfer that has not been downloaded yet and deletes the copy on the server."
        }
    };
    let (first, last) = match list {
        TransferList::Incoming => ("From", "Expires"),
        TransferList::Sent => ("Items", "Sent"),
    };
    let close_save = Callback::new(move |saved: bool| {
        saving.set(None);
        if saved {
            resource.refetch();
        }
    });

    view! {
        <main class="page">
            <TransfersHeader refresh />
            <ErrorAlert message=error />
            <section class="card card--table">
                <div class="table-scroll">
                    <table class="data-table transfer-table">
                        <thead>
                            <tr>
                                <th>{first}</th>
                                <th>"State"</th>
                                <th class="transfer-table__time">{last}</th>
                                {(list == TransferList::Sent).then(|| view! { <th class="transfer-table__cancel"></th> })}
                            </tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || transfers.get()
                                key=|transfer| (transfer.id, transfer.state)
                                children=move |transfer| {
                                    view! { <TransferRow transfer list me now act saving sending incoming /> }
                                }
                            />
                        </tbody>
                    </table>
                </div>
                <Show when=move || loaded.get() && transfers.read().is_empty() && error.read().is_none()>
                    <p class="card__empty">{empty}</p>
                </Show>
            </section>
            <p class="page__note">{note}</p>
        </main>
        {move || {
            saving
                .get()
                .map(|transfer| view! { <SaveTransferDialog transfer auth on_close=close_save /> })
        }}
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Download,
    Delete,
    Decline,
    Cancel,
}

/// The state of a transfer this tab is uploading: `Uploading 64%`.
fn uploading_label(fraction: f64) -> String {
    format!("Uploading {}%", percent(fraction))
}

/// The state of a transfer this tab is downloading: `Downloading 64%`.
fn downloading_label(fraction: f64) -> String {
    format!("Downloading {}%", percent(fraction))
}

/// The pill of a state, and its look.
fn state_pill(state: TransferState) -> &'static str {
    match state {
        TransferState::Uploading => "pill pill--accent",
        TransferState::Downloaded | TransferState::Saved => "pill pill--success",
        _ => "pill",
    }
}

#[component]
fn TransferRow(
    transfer: TransferSummary,
    list: TransferList,
    me: UserId,
    now: RwSignal<OffsetDateTime>,
    act: Callback<(Action, TransferSummary)>,
    saving: RwSignal<Option<TransferSummary>>,
    sending: Sending,
    incoming: Incoming,
) -> impl IntoView {
    let title = match list {
        TransferList::Incoming => {
            format!(
                "{} · {}",
                transfer_name(&transfer),
                transfer_size(&transfer)
            )
        }
        TransferList::Sent => transfer_name(&transfer),
    };
    let detail = match list {
        TransferList::Incoming => transfer_from(&transfer, me),
        TransferList::Sent => format!(
            "{} · {}",
            transfer_to(&transfer, me),
            transfer_size(&transfer)
        ),
    };
    let detail = match &transfer.message {
        Some(message) => format!("{detail} · “{message}”"),
        None => detail,
    };
    let icon = transfer_icon(&transfer);
    let (state, expires_at) = (transfer.state, transfer.expires_at);
    let actionable = list == TransferList::Incoming && state == TransferState::Ready;
    let cancellable = list == TransferList::Sent && !state.is_finished();
    let created = short_date(transfer.created_at);
    let id = transfer.id;

    let state_cell = if actionable {
        let progress = Memo::new(move |_| incoming.downloading.read().get(&id).copied());
        // Once the browser has the download, the transfer is only in the way: it can go.
        let handed_over = Memo::new(move |_| incoming.handed_over.read().contains(&id));
        let waiting = transfer.clone();
        (move || {
            if let Some(fraction) = progress.get() {
                return view! { <span class="pill pill--accent">{downloading_label(fraction)}</span> }
                    .into_any();
            }
            let (save, download, last) = (waiting.clone(), waiting.clone(), waiting.clone());
            let (last_action, last_label) = if handed_over.get() {
                (Action::Delete, "Delete")
            } else {
                (Action::Decline, "Decline")
            };
            view! {
                <span class="transfer-actions">
                    <button class="button" on:click=move |_| saving.set(Some(save.clone()))>
                        <Icon name="drive_folder_upload" />
                        "Save"
                    </button>
                    <button
                        class="button button--primary"
                        on:click=move |_| act.run((Action::Download, download.clone()))
                    >
                        <Icon name="download" />
                        "Download"
                    </button>
                    <button
                        class="button"
                        on:click=move |_| act.run((last_action, last.clone()))
                    >
                        {last_label}
                    </button>
                </span>
            }
            .into_any()
        })
        .into_any()
    } else {
        view! {
            <span class=state_pill(state)>
                {move || match sending.0.read().get(&id) {
                    Some(fraction) if state == TransferState::Uploading => {
                        uploading_label(*fraction)
                    }
                    _ => transfer_state(state, expires_at, now.get()),
                }}
            </span>
        }
        .into_any()
    };
    let time = move || match list {
        TransferList::Incoming => time_left(expires_at, now.get()),
        TransferList::Sent => created.clone(),
    };
    let cancel = transfer.clone();

    view! {
        <tr>
            <td>
                <span class="user-cell transfer-cell">
                    <Icon name=icon />
                    <span class="user-cell__text">
                        <span class="user-cell__name">{title}</span>
                        <span class="user-cell__detail">{detail}</span>
                    </span>
                </span>
            </td>
            <td>{state_cell}</td>
            <td class="transfer-table__time">{time}</td>
            {(list == TransferList::Sent)
                .then(|| {
                    view! {
                        <td class="transfer-table__cancel">
                            {cancellable
                                .then(|| {
                                    view! {
                                        <button
                                            class="button button--ghost button--small"
                                            aria-label="Cancel transfer"
                                            title="Cancel transfer"
                                            on:click=move |_| act.run((Action::Cancel, cancel.clone()))
                                        >
                                            <Icon name="close" />
                                        </button>
                                    }
                                })}
                        </td>
                    }
                })}
        </tr>
    }
}

#[cfg(test)]
mod tests {
    use protocol::UserRef;
    use uuid::Uuid;

    use super::*;

    fn summary(items: u32, files: u32) -> TransferSummary {
        let user = UserRef {
            id: UserId(Uuid::nil()),
            username: "sam".to_owned(),
        };
        TransferSummary {
            id: Uuid::nil(),
            sender: user.clone(),
            sender_device: None,
            sender_device_name: None,
            recipient: user,
            recipient_device: Uuid::nil(),
            recipient_device_name: "Phone".to_owned(),
            state: TransferState::Ready,
            total_bytes: 1,
            item_count: items,
            file_count: files,
            name: None,
            message: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn only_a_lone_file_downloads_without_a_zip() {
        assert_eq!(transfer_icon(&summary(1, 1)), "description");
        assert_eq!(transfer_icon(&summary(3, 3)), "folder");
        assert_eq!(uploading_label(0.645), "Uploading 64%");
        assert_eq!(uploading_label(1.2), "Uploading 100%");
        assert_eq!(downloading_label(0.5), "Downloading 50%");
    }
}
