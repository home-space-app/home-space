//! The video viewer: the browser's own player, streaming the file from the server.
//!
//! The player fetches the file in many range requests, so it gets a stream ticket, which
//! works until it expires (see `TicketRequest::Stream`). When the player fails, the viewer
//! gets a new ticket and picks up where it was; when that fails too, the browser cannot play
//! the file.

use leptos::{html, prelude::*, task::spawn_local};

use client_core::shared_link::DOWNLOAD;

use super::{ViewerHeader, source::Source};
use crate::{icon::Icon, session::Auth};

const LOADING: &str = "Loading…";
const UNSUPPORTED: &str = "This browser can't play this video. Download it to open it.";
const FAILED: &str = "The video could not be loaded.";
/// `MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED`.
const SRC_NOT_SUPPORTED: u16 = 4;
/// A failure this soon after the last retry is the file, not an expired ticket.
const RETRY_AFTER_MS: f64 = 60_000.0;

#[component]
pub fn VideoViewer(
    auth: Auth,
    source: Source,
    header: ViewerHeader,
    /// Offers Download in the header, for a file shared with the user.
    #[prop(optional)]
    on_download: Option<Callback<()>>,
) -> impl IntoView {
    let url = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let video = NodeRef::<html::Video>::new();
    // When the last retry was, and where to resume once the new URL plays.
    let last_retry = StoredValue::new(None::<f64>);
    let resume = StoredValue::new(None::<(f64, bool)>);

    let load = move || {
        spawn_local(async move {
            match source.stream_url(auth).await {
                Ok(new) => {
                    url.try_set(Some(new));
                }
                Err(err) => {
                    error.try_set(Some(err));
                }
            }
        });
    };
    load();

    let on_error = move |_| {
        let Some(player) = video.get_untracked() else {
            return;
        };
        let now = js_sys::Date::now();
        let retried_lately = last_retry
            .get_value()
            .is_some_and(|at| now - at < RETRY_AFTER_MS);
        if retried_lately {
            let unsupported = player
                .error()
                .is_some_and(|err| err.code() == SRC_NOT_SUPPORTED);
            error.set(Some(
                if unsupported { UNSUPPORTED } else { FAILED }.to_owned(),
            ));
            return;
        }
        last_retry.set_value(Some(now));
        resume.set_value(Some((player.current_time(), !player.paused())));
        load();
    };
    let on_loaded = move |_| {
        if let (Some(player), Some((at, playing))) = (video.get_untracked(), resume.get_value()) {
            resume.set_value(None);
            player.set_current_time(at);
            if playing {
                // A browser that refuses to play without a click leaves the player paused.
                let _ = player.play();
            }
        }
    };

    let actions = on_download.map(|download| {
        view! {
            <button class="button" on:click=move |_| download.run(())>
                <Icon name="download" />
                <span class="label">{DOWNLOAD}</span>
            </button>
        }
        .into_any()
    });
    let content = move || match (url.get(), error.read().is_some()) {
        (_, true) => ().into_any(),
        (Some(src), false) => view! {
            <div class="video-viewer">
                <video
                    class="video-viewer__video"
                    node_ref=video
                    src=src
                    controls
                    playsinline
                    preload="metadata"
                    on:error=on_error
                    on:loadedmetadata=on_loaded
                ></video>
            </div>
        }
        .into_any(),
        (None, false) => view! { <p class="explorer__empty">{LOADING}</p> }.into_any(),
    };

    view! {
        {header.view(actions)}
        {move || error.get().map(|message| view! { <p class="alert alert--error" role="alert">{message}</p> })}
        <div class="viewer__body">{content}</div>
    }
}
