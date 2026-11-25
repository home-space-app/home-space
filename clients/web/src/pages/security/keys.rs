//! The user's passkeys and security keys in the settings: the list, renaming and removing one,
//! and adding one through the browser.

use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use uuid::Uuid;

use protocol::{WebAuthnAdded, WebAuthnKey, WebAuthnKind};

use crate::{
    components::reconfirm::Reconfirm,
    locale,
    session::{self, Auth},
    webauthn,
};

#[component]
pub fn KeysSection(
    auth: Auth,
    error: RwSignal<Option<String>>,
    /// Gets the recovery codes when the new key was the account's first second factor.
    on_codes: Callback<Vec<String>>,
) -> impl IntoView {
    let keys = RwSignal::new(Vec::<WebAuthnKey>::new());
    let busy = RwSignal::new(false);
    let reconfirm = Reconfirm::get();
    let load = move || {
        spawn_local(async move {
            match session::client().webauthn_keys().await {
                Ok(found) => keys.set(found),
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    };
    load();
    let add = move |kind: WebAuthnKind| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let result = add_key(auth, reconfirm, kind).await;
            busy.set(false);
            match result {
                Ok(Some(added)) => {
                    let _ = auth.reload().await;
                    load();
                    if !added.recovery_codes.is_empty() {
                        on_codes.run(added.recovery_codes);
                    }
                }
                Ok(None) => {}
                Err(message) => error.set(Some(message)),
            }
        });
    };
    let changed = move || {
        load();
        spawn_local(async move {
            let _ = auth.reload().await;
        });
    };

    view! {
        <h3 class="card__heading">"Passkeys and security keys"</h3>
        <p class="card__text">
            "A passkey signs you in on its own, with your fingerprint, face or PIN. A security key, such as a YubiKey, takes the place of the app's code after your password."
        </p>
        <ul class="key-list">
            <For each=move || keys.get() key=|key| (key.id, key.name.clone()) let(key)>
                <KeyRow key auth error busy changed />
            </For>
        </ul>
        {move || {
            if webauthn::available() {
                view! {
                    <div class="card__actions">
                        <button
                            class="button"
                            disabled=move || busy.get()
                            on:click=move |_| add(WebAuthnKind::Passkey)
                        >
                            "Add a passkey"
                        </button>
                        <button
                            class="button"
                            disabled=move || busy.get()
                            on:click=move |_| add(WebAuthnKind::SecurityKey)
                        >
                            "Add a security key"
                        </button>
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <p class="card__text">
                        "This browser can't add passkeys here: they need the server's https address."
                    </p>
                }
                    .into_any()
            }
        }}
    }
}

/// Asks the server for a challenge, the browser for a new credential, and keeps it. `None`
/// when the user cancelled; the error is fit for display.
async fn add_key(
    auth: Auth,
    reconfirm: Reconfirm,
    kind: WebAuthnKind,
) -> Result<Option<WebAuthnAdded>, String> {
    let client = session::client();
    let started = reconfirm
        .confirmed(|| client.start_webauthn(kind))
        .await
        .map_err(|err| auth.fail(&err))?;
    let Some(challenge) = started else {
        return Ok(None);
    };
    let credential = match webauthn::create(challenge.options).await {
        Ok(credential) => credential,
        Err(failure) => return failure.message().map_or(Ok(None), Err),
    };
    client
        .finish_webauthn(challenge.challenge_id, &credential, None)
        .await
        .map(Some)
        .map_err(|err| auth.fail(&err))
}

/// One passkey or security key, renamed in place and removed after a confirmation.
#[component]
fn KeyRow(
    key: WebAuthnKey,
    auth: Auth,
    error: RwSignal<Option<String>>,
    busy: RwSignal<bool>,
    changed: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let id = key.id;
    let editing = RwSignal::new(false);
    let confirm_removal = RwSignal::new(false);
    let name = RwSignal::new(key.name.clone());
    let reconfirm = Reconfirm::get();
    let rename = move |event: SubmitEvent| {
        event.prevent_default();
        editing.set(false);
        let new_name = name.get_untracked();
        spawn_local(async move {
            match session::client().rename_webauthn(id, &new_name).await {
                Ok(_) => changed(),
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    };
    let remove = move |_| {
        confirm_removal.set(false);
        busy.set(true);
        spawn_local(async move {
            let result = remove_key(reconfirm, id).await;
            busy.set(false);
            match result {
                Ok(_) => changed(),
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    };
    let elsewhere = other_site(&key.rp_id);
    view! {
        <li class="key-list__item">
            <Show
                when=move || editing.get()
                fallback=move || {
                    view! {
                        <span class="key-list__name">{move || name.get()}</span>
                    }
                }
            >
                <form class="key-list__rename" on:submit=rename>
                    <input
                        class="field__input"
                        type="text"
                        maxlength="64"
                        required
                        aria-label="Name"
                        prop:value=move || name.get()
                        on:input:target=move |event| name.set(event.target().value())
                    />
                    <button type="submit" class="button button--small">"Save"</button>
                </form>
            </Show>
            <span class="key-list__details">{details(&key)}</span>
            {elsewhere.map(|site| view! { <span class="card__text--warning">{site}</span> })}
            <span class="card__actions">
                <button class="button button--ghost button--small" on:click=move |_| editing.update(|on| *on = !*on)>
                    "Rename"
                </button>
                <Show
                    when=move || confirm_removal.get()
                    fallback=move || {
                        view! {
                            <button
                                class="button button--ghost button--small"
                                on:click=move |_| confirm_removal.set(true)
                            >
                                "Remove"
                            </button>
                        }
                    }
                >
                    <span class="card__confirm">
                        "Remove it?"
                        <button class="button button--small" on:click=move |_| confirm_removal.set(false)>
                            "Keep it"
                        </button>
                        <button
                            class="button button--danger button--small"
                            disabled=move || busy.get()
                            on:click=remove
                        >
                            "Remove"
                        </button>
                    </span>
                </Show>
            </span>
        </li>
    }
}

async fn remove_key(reconfirm: Reconfirm, id: Uuid) -> Result<Option<()>, api_client::ApiError> {
    let client = session::client();
    reconfirm.confirmed(|| client.remove_webauthn(id)).await
}

/// What kind of key it is, and when it was added and last used.
fn details(key: &WebAuthnKey) -> String {
    let kind = match key.kind {
        WebAuthnKind::Passkey => "Passkey",
        WebAuthnKind::SecurityKey => "Security key",
    };
    let used = key.last_used_at.map_or_else(
        || "never used".to_owned(),
        |at| format!("last used {}", locale::short_date(at)),
    );
    format!(
        "{kind} · added {} · {used}",
        locale::short_date(key.created_at)
    )
}

/// A note when the key was made for another address of the server, where alone it works.
fn other_site(rp_id: &str) -> Option<String> {
    let here = web_sys::window()?.location().hostname().ok()?;
    (!here.eq_ignore_ascii_case(rp_id)).then(|| format!("Works only on {rp_id}"))
}
