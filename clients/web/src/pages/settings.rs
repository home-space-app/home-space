//! The user's own settings: profile, password and storage. Devices are the second tab.

use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use leptos_router::components::A;

use api_client::ApiError;
use client_core::storage::{bytes, fraction};
use protocol::{Event, Storage};

use super::{
    shell::{Tab, Tabs},
    sidebar::initial,
};
use crate::{
    components::ErrorAlert,
    events,
    icon::Icon,
    load, route,
    session::{self, Auth},
};

/// The heading and tabs shared by every settings page.
#[component]
pub fn SettingsHeader() -> impl IntoView {
    view! {
        <header class="page-header">
            <h1 class="page-title">"Settings"</h1>
        </header>
        <Tabs>
            <Tab href=route::SETTINGS_PATH icon="person" label="Account" />
            <Tab href=route::DEVICES_PATH icon="devices" label="Devices" />
        </Tabs>
    }
}

#[component]
pub fn SettingsPage(auth: Auth) -> impl IntoView {
    let storage = RwSignal::new(None::<Storage>);
    let error = RwSignal::new(None::<String>);
    let usage = load::load(
        || async { session::client().storage().await },
        move |result| match result {
            Ok(loaded) => {
                storage.set(Some(loaded));
                error.set(None);
            }
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );
    events::use_events(move |event| {
        if !matches!(event, Event::DevicesChanged) {
            usage.refetch();
        }
    });

    view! {
        <main class="page">
            <SettingsHeader />
            <ErrorAlert message=error />
            <div class="settings-grid">
                <ProfileCard auth />
                <section class="card">
                    <h2 class="card__title">"Storage"</h2>
                    {move || {
                        storage
                            .get()
                            .map(|storage| view! { <StorageBreakdown storage /> })
                    }}
                </section>
                <Show when=move || auth.user.get().is_some_and(|me| !me.external)>
                    <PasswordCard auth />
                </Show>
            </div>
        </main>
    }
}

#[component]
fn ProfileCard(auth: Auth) -> impl IntoView {
    move || {
        auth.user.get().map(|me| {
            let role = if me.is_admin {
                "Administrator"
            } else {
                "Member"
            };
            let method = if me.external {
                "Single sign-on"
            } else {
                "Password"
            };
            view! {
                <section class="card">
                    <h2 class="card__title">"Profile"</h2>
                    <div class="profile">
                        <span class="avatar avatar--large" aria-hidden="true">
                            {initial(&me.username)}
                        </span>
                        <div class="profile__who">
                            <span class="profile__name">{me.username.clone()}</span>
                            <span class="pill" class:pill--accent=me.is_admin>{role}</span>
                        </div>
                    </div>
                    <label class="field">
                        <span class="field__label">"Username"</span>
                        <input class="field__input" value=me.username readonly />
                    </label>
                    <label class="field">
                        <span class="field__label">"Sign-in method"</span>
                        <input class="field__input" value=method readonly />
                    </label>
                </section>
            }
        })
    }
}

/// The shortest password the server accepts.
const MIN_PASSWORD_LEN: usize = 8;

/// What is wrong with the form before it is sent, if anything.
pub fn password_problem(current: &str, new: &str, confirm: &str) -> Option<&'static str> {
    if current.is_empty() {
        Some("Enter your current password.")
    } else if new.len() < MIN_PASSWORD_LEN {
        Some("The new password must be at least 8 characters.")
    } else if new != confirm {
        Some("The new passwords do not match.")
    } else if new == current {
        Some("The new password is the same as the current one.")
    } else {
        None
    }
}

/// A server message as a sentence: capital first letter and a full stop.
fn sentence(message: &str) -> String {
    let mut chars = message.chars();
    let mut text: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !text.is_empty() && !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

#[component]
fn PasswordCard(auth: Auth) -> impl IntoView {
    let current = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let done = RwSignal::new(false);
    let busy = RwSignal::new(false);

    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        done.set(false);
        let (old, fresh) = (current.get_untracked(), new.get_untracked());
        if let Some(problem) = password_problem(&old, &fresh, &confirm.get_untracked()) {
            error.set(Some(problem.to_owned()));
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let result = session::client().change_password(&old, &fresh).await;
            busy.set(false);
            match result {
                Ok(()) => {
                    error.set(None);
                    done.set(true);
                    current.set(String::new());
                    new.set(String::new());
                    confirm.set(String::new());
                }
                Err(err @ ApiError::Api { .. }) => error.set(Some(sentence(&err.to_string()))),
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    };
    let input = move |signal: RwSignal<String>, autocomplete: &'static str| {
        view! {
            <input
                class="field__input"
                type="password"
                autocomplete=autocomplete
                required
                prop:value=move || signal.get()
                on:input:target=move |event| signal.set(event.target().value())
            />
        }
    };

    view! {
        <section class="card settings-grid__wide">
            <h2 class="card__title">"Change password"</h2>
            <ErrorAlert message=error />
            <Show when=move || done.get()>
                <p class="alert alert--success" role="status">
                    "Your password has been changed. Your other devices were signed out."
                </p>
            </Show>
            <form class="password-form" on:submit=submit>
                // Lets password managers file the new password under the right account.
                <input
                    type="text"
                    autocomplete="username"
                    hidden
                    readonly
                    prop:value=move || auth.user.get().map(|me| me.username).unwrap_or_default()
                />
                <div class="password-form__fields">
                    <label class="field">
                        <span class="field__label">"Current password"</span>
                        {input(current, "current-password")}
                    </label>
                    <label class="field">
                        <span class="field__label">"New password"</span>
                        {input(new, "new-password")}
                    </label>
                    <label class="field">
                        <span class="field__label">"Repeat new password"</span>
                        {input(confirm, "new-password")}
                    </label>
                </div>
                <button
                    type="submit"
                    class="button button--primary card__action"
                    disabled=move || busy.get()
                >
                    {move || if busy.get() { "Changing…" } else { "Change password" }}
                </button>
            </form>
        </section>
    }
}

#[component]
fn StorageBreakdown(storage: Storage) -> impl IntoView {
    let usage = storage.usage;
    let total = usage.total();
    let summary = match storage.disk {
        Some(disk) => format!(
            "{} used · {} free on the server",
            bytes(total),
            bytes(disk.available)
        ),
        None => format!("{} used", bytes(total)),
    };
    let parts = [
        ("Files", usage.files, "segment--files"),
        ("Previous versions", usage.versions, "segment--versions"),
        ("Recycle bin", usage.trash, "segment--trash"),
    ];
    view! {
        <p class="card__subtitle">{summary}</p>
        <div class="segments" aria-hidden="true">
            {parts
                .iter()
                .map(|(_, size, class)| {
                    view! {
                        <span
                            class=format!("segment {class}")
                            style=format!("width: {:.2}%", fraction(*size, total) * 100.0)
                        ></span>
                    }
                })
                .collect_view()}
        </div>
        <ul class="legend legend--rows">
            {parts
                .iter()
                .map(|(label, size, class)| {
                    view! {
                        <li class="legend__item">
                            <span class=format!("legend__dot {class}")></span>
                            <span class="legend__label">{*label}</span>
                            <span class="legend__value">{bytes(*size)}</span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
        <A href=route::TRASH_PATH attr:class="button button--ghost card__action">
            <Icon name="delete" />
            "Open recycle bin"
        </A>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_is_checked_before_sending() {
        assert_eq!(
            password_problem("", "long-enough", "long-enough"),
            Some("Enter your current password.")
        );
        assert_eq!(
            password_problem("old-one", "short", "short"),
            Some("The new password must be at least 8 characters.")
        );
        assert_eq!(
            password_problem("old-one", "long-enough", "long-enoug"),
            Some("The new passwords do not match.")
        );
        assert_eq!(
            password_problem("long-enough", "long-enough", "long-enough"),
            Some("The new password is the same as the current one.")
        );
        assert_eq!(
            password_problem("old-one", "long-enough", "long-enough"),
            None
        );
    }

    #[test]
    fn server_messages_read_as_sentences() {
        assert_eq!(
            sentence("the current password is wrong"),
            "The current password is wrong."
        );
        assert_eq!(sentence("Already done."), "Already done.");
        assert_eq!(sentence(""), "");
    }
}
