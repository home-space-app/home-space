//! Files sent to the user's devices, and the ones the user sent.

use leptos::{prelude::*, task::spawn_local};
use time::OffsetDateTime;

use client_core::transfers::{
    time_left, transfer_from, transfer_name, transfer_size, transfer_state, transfer_to,
};
use protocol::{Event, NodeKind, TicketRequest, TransferState, TransferSummary, UserId};

use super::{
    shell::{Tab, Tabs},
    transfer_save::SaveTransferDialog,
};
use crate::{
    components::ErrorAlert,
    events,
    icon::Icon,
    load,
    locale::{self, short_date},
    route,
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

/// Whether a transfer is one file, downloaded as is; anything else comes as a zip.
fn single_file(transfer: &TransferSummary) -> bool {
    transfer.item_count == 1 && transfer.file_count == 1
}

/// The icon of a transfer: a file, or a folder for several items.
fn transfer_icon(transfer: &TransferSummary) -> &'static str {
    if single_file(transfer) {
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
    let confirming = RwSignal::new(None::<TransferSummary>);
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
        spawn_local(async move {
            let client = session::client();
            let result = match action {
                Action::Decline => client.decline_transfer(id).await,
                Action::Cancel => client.cancel_transfer(id).await,
                Action::Confirm => client.confirm_transfer(id).await,
                Action::Download => download(&transfer).await.map(|()| {
                    confirming.set(Some(transfer.clone()));
                }),
            };
            if let Err(err) = result {
                error.set(Some(auth.fail(&err)));
            }
            if action != Action::Download {
                resource.refetch();
            }
        });
    };
    let act = Callback::new(move |(action, transfer)| act(action, transfer));
    let me = me(auth);
    let empty = match list {
        TransferList::Incoming => "Nothing has been sent to your devices.",
        TransferList::Sent => "You have not sent anything yet.",
    };
    let note = match list {
        TransferList::Incoming => {
            "Several files or a folder download as one zip. The server deletes its copy once you confirm the download finished."
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
                                    view! { <TransferRow transfer list me now act saving sending /> }
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
        {move || {
            confirming
                .get()
                .map(|transfer| {
                    let title = format!(
                        "{} · {} · {}",
                        transfer_name(&transfer),
                        transfer_size(&transfer),
                        transfer_from(&transfer, me),
                    );
                    let kind = if single_file(&transfer) { "file" } else { "zip" };
                    let left = time_left(transfer.expires_at, locale::now());
                    let body = format!(
                        "The browser does not say whether the {kind} was saved. If it was, the server deletes its copy. If not, you can download it again until it expires {left}.",
                    );
                    let yes = transfer.clone();
                    view! {
                        <div class="modal" on:click=move |_| confirming.set(None)>
                            <div
                                class="modal__panel modal__panel--wide"
                                role="alertdialog"
                                aria-modal="true"
                                on:click=|event| event.stop_propagation()
                            >
                                <header class="modal__header">
                                    <h2 class="modal__title">"Did the download finish?"</h2>
                                    <p class="modal__subtitle">{title}</p>
                                </header>
                                <p>{body}</p>
                                <div class="modal__actions">
                                    <button class="button" on:click=move |_| confirming.set(None)>
                                        "Not yet"
                                    </button>
                                    <button
                                        class="button button--primary"
                                        on:click=move |_| {
                                            confirming.set(None);
                                            act.run((Action::Confirm, yes.clone()));
                                        }
                                    >
                                        <Icon name="check" />
                                        "Yes, it finished"
                                    </button>
                                </div>
                            </div>
                        </div>
                    }
                })
        }}
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Download,
    Confirm,
    Decline,
    Cancel,
}

/// Starts the browser's download of a transfer: its one file, or a zip of everything.
async fn download(transfer: &TransferSummary) -> Result<(), api_client::ApiError> {
    let client = session::client();
    let url = if single_file(transfer) {
        let full = client.transfer(transfer.id).await?;
        let Some(item) = full.items.iter().find(|item| item.kind == NodeKind::File) else {
            return Ok(());
        };
        let request = TicketRequest::TransferItem {
            transfer_id: transfer.id,
            item_id: item.id,
        };
        let ticket = client.ticket(&request).await?;
        client.transfer_item_url(transfer.id, item.id, &ticket.ticket)
    } else {
        let request = TicketRequest::TransferArchive {
            transfer_id: transfer.id,
        };
        let ticket = client.ticket(&request).await?;
        client.transfer_archive_url(transfer.id, &ticket.ticket)
    };
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(&url);
    }
    Ok(())
}

/// The state of a transfer this tab is uploading: `Uploading 64%`.
fn uploading_label(fraction: f64) -> String {
    format!("Uploading {}%", (fraction.clamp(0.0, 1.0) * 100.0).floor())
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
    let single = single_file(&transfer);
    let id = transfer.id;

    let state_cell = if actionable {
        let (save, download, decline) = (transfer.clone(), transfer.clone(), transfer.clone());
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
                    {if single { "Download" } else { "Download zip" }}
                </button>
                <button
                    class="button"
                    on:click=move |_| act.run((Action::Decline, decline.clone()))
                >
                    "Decline"
                </button>
            </span>
        }
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
        assert!(single_file(&summary(1, 1)));
        assert!(!single_file(&summary(2, 1)));
        assert!(!single_file(&summary(3, 3)));
        assert_eq!(transfer_icon(&summary(3, 3)), "folder");
        assert_eq!(uploading_label(0.645), "Uploading 64%");
        assert_eq!(uploading_label(1.2), "Uploading 100%");
    }
}
