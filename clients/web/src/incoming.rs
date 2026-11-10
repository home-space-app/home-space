//! Transfers waiting for the user: how many there are, the toast for one that just arrived
//! for this browser, and downloading them.

use std::collections::{HashMap, HashSet};

use leptos::{prelude::*, task::spawn_local};
use leptos_router::hooks::use_location;
use uuid::Uuid;

use api_client::ApiError;
use client_core::transfers::{download_name, is_single_file, received_detail, received_title};
use protocol::{Event, NodeKind, TicketRequest, TransferState, TransferSummary};

use crate::{
    events,
    icon::Icon,
    load, locale,
    pages::SaveTransferDialog,
    route,
    save_file::{self, Pick},
    session::{self, Auth},
};

/// What every page knows about the transfers sent to the user.
#[derive(Clone, Copy)]
pub struct Incoming {
    /// The transfers still waiting for the user, on any of their devices.
    pub waiting: RwSignal<Vec<TransferSummary>>,
    /// How far each download this tab is writing has got, between 0 and 1, by transfer id.
    pub downloading: RwSignal<HashMap<Uuid, f64>>,
    /// Downloads handed to the browser, which does not say whether they finished.
    pub handed_over: RwSignal<HashSet<Uuid>>,
    /// The transfer that just arrived for this browser, until the user answers.
    toast: RwSignal<Option<TransferSummary>>,
    /// Why the last download failed.
    error: RwSignal<Option<String>>,
}

/// Shares one [`Incoming`] with every component below the caller.
pub fn provide_incoming() -> Incoming {
    let incoming = Incoming {
        waiting: RwSignal::new(Vec::new()),
        downloading: RwSignal::new(HashMap::new()),
        handed_over: RwSignal::new(HashSet::new()),
        toast: RwSignal::new(None),
        error: RwSignal::new(None),
    };
    provide_context(incoming);
    incoming
}

impl Incoming {
    /// Downloads a transfer to this computer. Where the browser lets the page write the
    /// file, the download is confirmed once it is complete and the server deletes its copy;
    /// elsewhere the browser downloads it and the transfer stays until the user deletes it.
    pub fn download(self, auth: Auth, transfer: TransferSummary) {
        if self.downloading.read_untracked().contains_key(&transfer.id) {
            return;
        }
        self.error.set(None);
        spawn_local(async move {
            if let Err(message) = self.run(auth, &transfer).await {
                self.error.set(Some(message));
            }
        });
    }

    async fn run(self, auth: Auth, transfer: &TransferSummary) -> Result<(), String> {
        // The picker comes first: it only opens right after the click.
        let target = match save_file::pick(&download_name(transfer)).await {
            Pick::Picked(target) => Some(target),
            Pick::Cancelled => return Ok(()),
            Pick::Unavailable => None,
        };
        let id = transfer.id;
        let Some(url) = download_url(transfer)
            .await
            .map_err(|err| auth.fail(&err))?
        else {
            return Ok(());
        };
        let Some(target) = target else {
            if let Some(window) = web_sys::window() {
                let _ = window.location().set_href(&url);
            }
            self.handed_over.update(|ids| {
                ids.insert(id);
            });
            return Ok(());
        };
        // A zip is a little larger than its files, so the fraction is capped.
        let total = (transfer.total_bytes as f64).max(1.0);
        let report = move |done: f64| {
            let fraction = (done / total).min(1.0);
            let shown = self.downloading.read_untracked().get(&id).copied();
            if shown.is_none_or(|shown| percent(shown) != percent(fraction)) {
                self.downloading.update(|downloads| {
                    downloads.insert(id, fraction);
                });
            }
        };
        report(0.0);
        let written = target.write_from(&url, report).await;
        self.downloading.update(|downloads| {
            downloads.remove(&id);
        });
        written?;
        session::client()
            .confirm_transfer(id)
            .await
            .map_err(|err| auth.fail(&err))?;
        self.waiting
            .update(|waiting| waiting.retain(|transfer| transfer.id != id));
        Ok(())
    }
}

/// A fraction as whole percent, rounded down.
pub fn percent(fraction: f64) -> u32 {
    (fraction.clamp(0.0, 1.0) * 100.0).floor() as u32
}

/// Where a transfer downloads from: its one file, or a zip of everything. `None` when it
/// holds no file.
async fn download_url(transfer: &TransferSummary) -> Result<Option<String>, ApiError> {
    let client = session::client();
    if is_single_file(transfer) {
        let full = client.transfer(transfer.id).await?;
        let Some(item) = full.items.iter().find(|item| item.kind == NodeKind::File) else {
            return Ok(None);
        };
        let request = TicketRequest::TransferItem {
            transfer_id: transfer.id,
            item_id: item.id,
        };
        let ticket = client.ticket(&request).await?;
        Ok(Some(client.transfer_item_url(
            transfer.id,
            item.id,
            &ticket.ticket,
        )))
    } else {
        let request = TicketRequest::TransferArchive {
            transfer_id: transfer.id,
        };
        let ticket = client.ticket(&request).await?;
        Ok(Some(
            client.transfer_archive_url(transfer.id, &ticket.ticket),
        ))
    }
}

/// Keeps [`Incoming`] current while the user is signed in, and shows a transfer that
/// arrives for this browser in the corner of the window.
#[component]
pub fn IncomingToast(auth: Auth) -> impl IntoView {
    let incoming = expect_context::<Incoming>();
    let location = use_location();
    let saving = RwSignal::new(None::<TransferSummary>);
    let this_device = session::device_id();
    // The list of incoming transfers already shows what arrives.
    let on_list = move || location.pathname.get() == route::TRANSFERS_PATH;

    let listed = load::load(
        || async { session::client().incoming_transfers(false).await },
        move |result| {
            let Ok(mut transfers) = result else {
                return;
            };
            transfers.retain(|transfer| transfer.state == TransferState::Ready);
            let gone = incoming.toast.with_untracked(|toast| {
                toast
                    .as_ref()
                    .is_some_and(|toast| !transfers.iter().any(|transfer| transfer.id == toast.id))
            });
            if gone {
                incoming.toast.set(None);
            }
            incoming.waiting.set(transfers);
        },
    );
    events::use_events(move |event| match event {
        Event::TransferReceived { transfer } => {
            let on_list = location.pathname.get_untracked() == route::TRANSFERS_PATH;
            if Some(transfer.recipient_device) == this_device && !on_list {
                incoming.toast.set(Some(transfer));
            }
            listed.refetch();
        }
        Event::TransferChanged { .. } | Event::Resync => listed.refetch(),
        _ => {}
    });
    Effect::new(move |_| {
        if on_list() {
            incoming.toast.set(None);
        }
    });
    on_cleanup(move || {
        incoming.waiting.set(Vec::new());
        incoming.toast.set(None);
        incoming.error.set(None);
    });

    let me = auth.user.get_untracked().map(|me| me.id);
    let close_save = Callback::new(move |_saved: bool| {
        saving.set(None);
        listed.refetch();
    });

    view! {
        {move || {
            let transfer = incoming.toast.get()?;
            let title = received_title(&transfer, me?);
            let detail = received_detail(&transfer, locale::now());
            let (save, download) = (transfer.clone(), transfer);
            Some(
                view! {
                    <section class="toast" role="region" aria-label="Incoming transfer" aria-live="polite">
                        <p class="toast__title">
                            <Icon name="download" />
                            <span>{title}</span>
                        </p>
                        <p class="toast__detail">{detail}</p>
                        <div class="toast__actions">
                            <button class="button" on:click=move |_| incoming.toast.set(None)>
                                "Later"
                            </button>
                            <button
                                class="button"
                                on:click=move |_| {
                                    incoming.toast.set(None);
                                    saving.set(Some(save.clone()));
                                }
                            >
                                <Icon name="drive_folder_upload" />
                                "Save"
                            </button>
                            <button
                                class="button button--primary"
                                on:click=move |_| {
                                    incoming.toast.set(None);
                                    incoming.download(auth, download.clone());
                                }
                            >
                                <Icon name="download" />
                                "Download"
                            </button>
                        </div>
                    </section>
                },
            )
        }}
        {move || {
            incoming
                .error
                .get()
                .map(|message| {
                    view! {
                        <section class="toast toast--error" role="alert">
                            <p class="toast__title">{message}</p>
                            <div class="toast__actions">
                                <button class="button" on:click=move |_| incoming.error.set(None)>
                                    "Close"
                                </button>
                            </div>
                        </section>
                    }
                })
        }}
        {move || {
            saving
                .get()
                .map(|transfer| view! { <SaveTransferDialog transfer auth on_close=close_save /> })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_rounds_down_and_stays_in_range() {
        assert_eq!(percent(0.645), 64);
        assert_eq!(percent(1.2), 100);
        assert_eq!(percent(-1.0), 0);
    }
}
