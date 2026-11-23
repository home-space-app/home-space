//! The "New version available" toast for administrators: the web client is the server, so it
//! compares the server's version with the latest release and links to the update guide.

use leptos::prelude::*;

use client_core::update;

use crate::{icon::Icon, session::Auth, session::storage};

/// This server's version, which the web client is built with.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The release the administrator skipped, in this browser's storage.
const SKIPPED_KEY: &str = "update.skipped_version";
/// `"off"` when the administrator turned the check off in this browser.
const CHECK_KEY: &str = "update.check";
const CHECK_OFF: &str = "off";

/// Whether this browser checks for new versions; on unless turned off.
pub fn checks() -> bool {
    storage()
        .and_then(|storage| storage.get_item(CHECK_KEY).ok().flatten())
        .is_none_or(|value| value != CHECK_OFF)
}

/// Turns the check on or off for this browser.
pub fn set_checks(on: bool) {
    if let Some(storage) = storage() {
        let _ = if on {
            storage.remove_item(CHECK_KEY)
        } else {
            storage.set_item(CHECK_KEY, CHECK_OFF)
        };
    }
}

fn skipped() -> Option<String> {
    storage()?.get_item(SKIPPED_KEY).ok().flatten()
}

fn skip(version: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(SKIPPED_KEY, version);
    }
}

/// The toast, for administrators only, once the latest release is known to be newer.
#[component]
pub fn UpdateToast(auth: Auth) -> impl IntoView {
    let offered = RwSignal::new(None::<String>);
    let checked = StoredValue::new(false);
    Effect::new(move |_| {
        let is_admin = auth.user.read().as_ref().is_some_and(|me| me.is_admin);
        if !is_admin {
            offered.set(None);
            return;
        }
        if checked.get_value() || !checks() {
            return;
        }
        checked.set_value(true);
        leptos::task::spawn_local(async move {
            let Some(release) = api_client::latest_release(update::RELEASES_API_URL).await else {
                return;
            };
            if update::should_offer(VERSION, &release.version, skipped().as_deref()) {
                offered.set(Some(release.version));
            }
        });
    });

    move || {
        let latest = offered.get()?;
        let skipped = latest.clone();
        Some(view! {
            <section class="toast" role="region" aria-label=update::TITLE aria-live="polite">
                <p class="toast__title">
                    <Icon name="info" />
                    <span class="toast__grow">{update::TITLE}</span>
                    <button
                        class="button button--ghost button--small toast__close"
                        aria-label="Close"
                        on:click=move |_| offered.set(None)
                    >
                        <Icon name="close" />
                    </button>
                </p>
                <p class="toast__detail">{update::server_message(&latest, VERSION)}</p>
                <div class="toast__actions">
                    <button
                        class="button"
                        on:click=move |_| {
                            skip(&skipped);
                            offered.set(None);
                        }
                    >
                        {update::SKIP}
                    </button>
                    <a
                        class="button button--primary"
                        href=update::UPDATE_GUIDE_URL
                        target="_blank"
                        rel="noopener"
                    >
                        <Icon name="language" />
                        {update::HOW_TO_UPDATE}
                    </a>
                </div>
            </section>
        })
    }
}
