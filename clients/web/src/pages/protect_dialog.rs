//! Protecting a folder with a password, changing or recovering its password, and the folder's
//! recovery key, shown once.

use gloo_timers::future::TimeoutFuture;
use leptos::{prelude::*, task::spawn_local};

use client_core::{
    accounts::password_problem,
    protected::{
        PasswordProof, ProtectedError, change_folder_password, protect_with_password,
        unlock_password_folder,
    },
};
use protocol::{FolderLock, Node, ProtectedFolder};

use crate::{
    components::{ErrorAlert, reconfirm::Reconfirm},
    protected::{Vault, use_vault},
    session,
};

/// How long the page waits before deriving a key, so it can show that it is working first:
/// the derivation holds the page for about a second.
const PAINT_MS: u32 = 30;
const WEB_CAVEAT: &str = "The browser runs code from your server. Someone who controls the server could change it to read the password; the desktop and mobile apps do not have this risk.";
const SECURITY_KEY_ON_WEB: &str =
    "This folder is protected with an encryption key. Open it in the desktop app.";

/// Asks for a new folder password, protects `node` with it and shows its recovery key.
/// `on_protected` gets the protection as soon as the server has it.
#[component]
pub fn ProtectDialog(
    node: Node,
    on_close: Callback<()>,
    on_protected: Callback<ProtectedFolder>,
) -> impl IntoView {
    let vault = use_vault();
    let reconfirm = Reconfirm::get();
    let password = RwSignal::new(String::new());
    let repeat = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let recovery_key = RwSignal::new(None::<String>);
    let folder = node.id;
    let name = node.name.clone();

    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        let (typed, again) = (password.get_untracked(), repeat.get_untracked());
        if let Some(problem) = password_problem(&typed, &again) {
            error.set(Some(problem.to_owned()));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            TimeoutFuture::new(PAINT_MS).await;
            let outcome = protect(vault, reconfirm, folder, &typed).await;
            busy.set(false);
            match outcome {
                Ok(Some((protected, key))) => {
                    password.set(String::new());
                    repeat.set(String::new());
                    recovery_key.set(Some(key));
                    on_protected.run(protected);
                }
                Ok(None) => {}
                Err(message) => error.set(Some(message)),
            }
        });
    };

    view! {
        <div class="modal">
            <div class="modal__panel" role="dialog" aria-modal="true" aria-labelledby="protect-title">
                {move || match recovery_key.get() {
                    Some(key) => {
                        view! { <RecoveryKey name=name.clone() key on_done=on_close /> }.into_any()
                    }
                    None => {
                        view! {
                            <header class="modal__header">
                                <h2 class="modal__title" id="protect-title">
                                    {format!("Protect “{}” with a password", name)}
                                </h2>
                                <p class="modal__subtitle">
                                    "Its files and names are encrypted on this device before they are uploaded. The server never sees the password."
                                </p>
                            </header>
                            <form
                                class="inline-form"
                                on:submit=move |event| {
                                    event.prevent_default();
                                    submit();
                                }
                            >
                                <PasswordField label="Password" value=password autocomplete="new-password" />
                                <PasswordField label="Repeat the password" value=repeat autocomplete="new-password" />
                                <p class="alert alert--warning">
                                    "The previous versions of the files in this folder will be deleted. Deleting something inside a protected folder deletes it for good."
                                </p>
                                <p class="modal__note">
                                    "Without the password and the recovery key you get next, nobody can open the files, not even the server's administrator."
                                </p>
                                <p class="modal__note">{WEB_CAVEAT}</p>
                                <ErrorAlert message=error />
                                <div class="modal__actions">
                                    <button type="button" class="button" on:click=move |_| on_close.run(())>
                                        "Cancel"
                                    </button>
                                    <button type="submit" class="button button--primary" disabled=move || busy.get()>
                                        {move || if busy.get() { "Protecting…" } else { "Protect" }}
                                    </button>
                                </div>
                            </form>
                        }
                            .into_any()
                    }
                }}
            </div>
        </div>
    }
}

async fn protect(
    vault: Vault,
    reconfirm: Reconfirm,
    folder: protocol::NodeId,
    password: &str,
) -> Result<Option<(ProtectedFolder, String)>, String> {
    let new = protect_with_password(folder, password).map_err(|err| err.to_string())?;
    let client = session::client();
    let Some(protected) = reconfirm
        .confirmed(|| client.protect_folder(&new.request))
        .await
        .map_err(|err| err.to_string())?
    else {
        return Ok(None);
    };
    let key = new.recovery_key.to_display().to_string();
    vault.remember(protected.clone());
    vault
        .unlock(&protected, new.keys)
        .map_err(|err| err.to_string())?;
    Ok(Some((protected, key)))
}

/// What the folder password dialog is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordAction {
    Unlock,
    Change,
}

/// Unlocks a password folder, or changes its password; either way the user can use the
/// recovery key instead, and then chooses a new password.
#[component]
pub fn FolderPasswordDialog(
    folder: ProtectedFolder,
    name: String,
    action: PasswordAction,
    on_close: Callback<()>,
) -> impl IntoView {
    let vault = use_vault();
    let reconfirm = Reconfirm::get();
    let current = RwSignal::new(String::new());
    let new_password = RwSignal::new(String::new());
    let repeat = RwSignal::new(String::new());
    let recovering = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let recovery_key = RwSignal::new(None::<String>);
    let folder = StoredValue::new(folder);
    let is_password = folder.with_value(|folder| folder.lock == FolderLock::Password);
    let sets_password = move || recovering.get() || action == PasswordAction::Change;

    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        let setting = recovering.get_untracked() || action == PasswordAction::Change;
        let (typed, fresh, again) = (
            current.get_untracked(),
            new_password.get_untracked(),
            repeat.get_untracked(),
        );
        if setting && let Some(problem) = password_problem(&fresh, &again) {
            error.set(Some(problem.to_owned()));
            return;
        }
        busy.set(true);
        error.set(None);
        let recovering = recovering.get_untracked();
        spawn_local(async move {
            TimeoutFuture::new(PAINT_MS).await;
            let listed = folder.get_value();
            let proof = if recovering {
                PasswordProof::RecoveryKey(&typed)
            } else {
                PasswordProof::Password(&typed)
            };
            let outcome = if setting {
                set_password(vault, reconfirm, &listed, proof, &fresh).await
            } else {
                unlock_password_folder(&listed, proof)
                    .and_then(|keys| vault.unlock(&listed, keys))
                    .map(|()| Some(None))
                    .map_err(|err| err.to_string())
            };
            busy.set(false);
            match outcome {
                Ok(Some(Some(key))) => recovery_key.set(Some(key)),
                Ok(Some(None)) => on_close.run(()),
                Ok(None) => {}
                Err(message) => error.set(Some(message)),
            }
        });
    };

    let title = match action {
        PasswordAction::Unlock => format!("Unlock “{name}”"),
        PasswordAction::Change => format!("Change the password of “{name}”"),
    };
    view! {
        <div class="modal">
            <div class="modal__panel" role="dialog" aria-modal="true" aria-labelledby="folder-password-title">
                {move || match recovery_key.get() {
                    Some(key) => {
                        view! { <RecoveryKey name=name.clone() key on_done=on_close /> }.into_any()
                    }
                    None if !is_password => {
                        view! {
                            <header class="modal__header">
                                <h2 class="modal__title" id="folder-password-title">{title.clone()}</h2>
                            </header>
                            <p>{SECURITY_KEY_ON_WEB}</p>
                            <div class="modal__actions">
                                <button class="button button--primary" on:click=move |_| on_close.run(())>
                                    "Close"
                                </button>
                            </div>
                        }
                            .into_any()
                    }
                    None => {
                        view! {
                            <header class="modal__header">
                                <h2 class="modal__title" id="folder-password-title">{title.clone()}</h2>
                                <Show when=move || recovering.get()>
                                    <p class="modal__subtitle">
                                        "Type the folder's recovery key, then choose a new password. You get a new recovery key, and the old one stops working."
                                    </p>
                                </Show>
                            </header>
                            <form
                                class="inline-form"
                                on:submit=move |event| {
                                    event.prevent_default();
                                    submit();
                                }
                            >
                                {move || {
                                    if recovering.get() {
                                        view! { <PasswordField label="Recovery key" value=current autocomplete="off" /> }
                                    } else if action == PasswordAction::Change {
                                        view! { <PasswordField label="Current password" value=current autocomplete="current-password" /> }
                                    } else {
                                        view! { <PasswordField label="Password" value=current autocomplete="current-password" /> }
                                    }
                                }}
                                <Show when=sets_password>
                                    <PasswordField label="New password" value=new_password autocomplete="new-password" />
                                    <PasswordField label="Repeat the new password" value=repeat autocomplete="new-password" />
                                </Show>
                                <ErrorAlert message=error />
                                <Show when=move || !recovering.get()>
                                    <button
                                        type="button"
                                        class="button button--ghost button--small"
                                        on:click=move |_| {
                                            current.set(String::new());
                                            error.set(None);
                                            recovering.set(true);
                                        }
                                    >
                                        "Forgot the password?"
                                    </button>
                                </Show>
                                <div class="modal__actions">
                                    <button type="button" class="button" on:click=move |_| on_close.run(())>
                                        "Cancel"
                                    </button>
                                    <button type="submit" class="button button--primary" disabled=move || busy.get()>
                                        {move || match (busy.get(), sets_password()) {
                                            (true, _) => "Working…",
                                            (false, true) => "Set password",
                                            (false, false) => "Unlock",
                                        }}
                                    </button>
                                </div>
                            </form>
                        }
                            .into_any()
                    }
                }}
            </div>
        </div>
    }
}

/// Wraps the folder's secret for `new_password` and keeps the folder unlocked: `Some` once
/// stored, with the new recovery key when the old one was used; `None` when the user did not
/// confirm it is them.
async fn set_password(
    vault: Vault,
    reconfirm: Reconfirm,
    folder: &ProtectedFolder,
    proof: PasswordProof<'_>,
    new_password: &str,
) -> Result<Option<Option<String>>, String> {
    let message = |err: ProtectedError| err.to_string();
    let keys = unlock_password_folder(folder, proof).map_err(message)?;
    let change = change_folder_password(folder, proof, new_password).map_err(message)?;
    let client = session::client();
    let Some(updated) = reconfirm
        .confirmed(|| client.change_folder_password(folder.node_id, &change.request))
        .await
        .map_err(|err| err.to_string())?
    else {
        return Ok(None);
    };
    vault.remember(updated.clone());
    vault.unlock(&updated, keys).map_err(message)?;
    Ok(Some(
        change.recovery_key.map(|key| key.to_display().to_string()),
    ))
}

#[component]
fn PasswordField(
    label: &'static str,
    value: RwSignal<String>,
    autocomplete: &'static str,
) -> impl IntoView {
    view! {
        <label class="field">
            <span class="field__label">{label}</span>
            <input
                class="field__input"
                type="password"
                autocomplete=autocomplete
                prop:value=move || value.get()
                on:input:target=move |event| value.set(event.target().value())
            />
        </label>
    }
}

/// A folder's recovery key, shown once, with ways to keep it.
#[component]
fn RecoveryKey(name: String, key: String, on_done: Callback<()>) -> impl IntoView {
    let copied = RwSignal::new(false);
    let saved = RwSignal::new(false);
    let copy_key = key.clone();
    let copy = move |_| {
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(&copy_key);
            copied.set(true);
        }
    };
    let download = format!(
        "data:text/plain;charset=utf-8,{}",
        js_sys::encode_uri_component(&format!(
            "Home Space recovery key for the folder “{name}”\n\n{key}\n"
        ))
    );
    view! {
        <header class="modal__header">
            <h2 class="modal__title">"Save the recovery key"</h2>
            <p class="modal__subtitle">
                {format!("If you forget the password, this key is the only way to open “{name}”. Keep it somewhere safe, away from this device. It is shown only now.")}
            </p>
        </header>
        <p class="secret-key">{key}</p>
        <div class="card__actions">
            <button class="button" on:click=copy>
                {move || if copied.get() { "Copied" } else { "Copy" }}
            </button>
            <a class="button" href=download download="home-space-folder-recovery-key.txt">
                "Download"
            </a>
        </div>
        <label class="card__confirm">
            <input type="checkbox" on:change:target=move |event| saved.set(event.target().checked()) />
            "I have saved the recovery key"
        </label>
        <div class="modal__actions">
            <button class="button button--primary" disabled=move || !saved.get() on:click=move |_| on_done.run(())>
                "Done"
            </button>
        </div>
    }
}
