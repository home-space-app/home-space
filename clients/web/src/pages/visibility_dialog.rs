//! Choosing who can see one of the user's devices and send files to it.

use leptos::{prelude::*, task::spawn_local};

use client_core::{
    labels::platform,
    transfers::{VISIBILITIES, visibility_hint, visibility_title},
};
use protocol::{Device, DeviceVisibility, SetDeviceVisibility, UserRef};

use super::sidebar::initial;
use crate::{
    components::{ErrorAlert, Menu},
    icon::Icon,
    load,
    session::{self, Auth},
};

const VISIBILITY_GROUP: &str = "device-visibility";
const PICKER_ID: &str = "visibility-users";

/// The users whose name contains `query`, ignoring case.
fn matching<'a>(users: &'a [UserRef], query: &str) -> Vec<&'a UserRef> {
    let query = query.trim().to_lowercase();
    users
        .iter()
        .filter(|user| query.is_empty() || user.username.to_lowercase().contains(&query))
        .collect()
}

/// Asks who can see `device`; `on_close` gets whether a new setting was saved.
#[component]
pub fn VisibilityDialog(device: Device, auth: Auth, on_close: Callback<bool>) -> impl IntoView {
    let id = device.id;
    let choice = RwSignal::new(device.visibility);
    let picked = RwSignal::new(device.visible_to.clone());
    let users = RwSignal::new(Vec::<UserRef>::new());
    let query = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);

    load::load(
        || async { session::client().users().await },
        move |result| match result {
            Ok(listed) => users.set(listed),
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );

    let toggle = move |user: UserRef| {
        picked.update(|picked| {
            if let Some(at) = picked.iter().position(|chosen| chosen.id == user.id) {
                picked.remove(at);
            } else {
                picked.push(user);
            }
        });
    };
    // Visible to specific users with nobody picked would be the same as only you.
    let can_save = move || {
        !saving.get() && (choice.get() != DeviceVisibility::Users || !picked.read().is_empty())
    };
    let save = move |_| {
        let visibility = choice.get_untracked();
        let users = match visibility {
            DeviceVisibility::Users => picked.get_untracked().iter().map(|user| user.id).collect(),
            _ => Vec::new(),
        };
        saving.set(true);
        spawn_local(async move {
            let request = SetDeviceVisibility { visibility, users };
            match session::client().set_device_visibility(id, &request).await {
                Ok(()) => on_close.run(true),
                Err(err) => {
                    saving.set(false);
                    error.set(Some(auth.fail(&err)));
                }
            }
        });
    };
    let title = format!("Who can see {}?", device.name);
    let subtitle = format!("{} · Only affects this device", platform(&device.platform));

    view! {
        <div class="modal" on:click=move |_| on_close.run(false)>
            <div
                class="modal__panel modal__panel--wide"
                role="dialog"
                aria-modal="true"
                aria-labelledby="visibility-title"
                on:click=|event| event.stop_propagation()
            >
                <header class="modal__header">
                    <h2 id="visibility-title" class="modal__title">{title}</h2>
                    <p class="modal__subtitle">{subtitle}</p>
                </header>
                <div class="choices" role="radiogroup">
                    {VISIBILITIES
                        .into_iter()
                        .map(|visibility| {
                            let selected = move || choice.get() == visibility;
                            view! {
                                <div class="choice" class:choice--selected=selected>
                                    <label class="choice__label">
                                        <input
                                            type="radio"
                                            class="choice__radio"
                                            name=VISIBILITY_GROUP
                                            prop:checked=selected
                                            on:change=move |_| choice.set(visibility)
                                        />
                                        <span class="choice__copy">
                                            <span class="choice__title">
                                                {visibility_title(visibility)}
                                            </span>
                                            <span class="choice__hint">
                                                {visibility_hint(visibility)}
                                            </span>
                                        </span>
                                    </label>
                                    {(visibility == DeviceVisibility::Users)
                                        .then(|| {
                                            view! {
                                                <Show when=selected>
                                                    <UserChips picked users query toggle />
                                                </Show>
                                            }
                                        })}
                                </div>
                            }
                        })
                        .collect_view()}
                </div>
                <p class="modal__note">
                    "New devices start visible only to you. Your other devices keep their own setting."
                </p>
                <ErrorAlert message=error />
                <div class="modal__actions">
                    <button class="button" on:click=move |_| on_close.run(false)>
                        "Cancel"
                    </button>
                    <button
                        class="button button--primary"
                        disabled=move || !can_save()
                        on:click=save
                    >
                        <Icon name="check" />
                        "Save"
                    </button>
                </div>
            </div>
        </div>
    }
}

/// The users picked so far, each removable, and the button that picks more.
#[component]
fn UserChips(
    picked: RwSignal<Vec<UserRef>>,
    users: RwSignal<Vec<UserRef>>,
    query: RwSignal<String>,
    toggle: impl Fn(UserRef) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="chips">
            <For
                each=move || picked.get()
                key=|user| user.id
                children=move |user| {
                    let name = user.username.clone();
                    let label = format!("Remove {name}");
                    view! {
                        <span class="chip">
                            <span class="avatar avatar--small" aria-hidden="true">
                                {initial(&name)}
                            </span>
                            {name.clone()}
                            <button
                                class="chip__remove"
                                aria-label=label
                                on:click=move |_| toggle(user.clone())
                            >
                                <Icon name="close" />
                            </button>
                        </span>
                    }
                }
            />
            <Menu
                id=PICKER_ID.to_owned()
                label="Add user"
                icon="person_add"
                text=Signal::stored("Add user".to_owned())
                align_start=true
            >
                <div class="user-picker">
                    <label class="user-picker__search">
                        <Icon name="search" />
                        <input
                            class="user-picker__input"
                            type="search"
                            placeholder="Search people"
                            aria-label="Search people"
                            prop:value=move || query.get()
                            on:input:target=move |event| query.set(event.target().value())
                        />
                    </label>
                    <ul class="user-picker__list">
                        {move || {
                            let users = users.read();
                            let shown = matching(&users, &query.read());
                            if shown.is_empty() {
                                return view! {
                                    <li class="user-picker__empty">"Nobody else matches."</li>
                                }
                                    .into_any();
                            }
                            shown
                                .into_iter()
                                .cloned()
                                .map(|user| {
                                    let user_id = user.id;
                                    let checked = move || {
                                        picked.read().iter().any(|chosen| chosen.id == user_id)
                                    };
                                    let name = user.username.clone();
                                    view! {
                                        <li>
                                            <label class="user-picker__row">
                                                <input
                                                    type="checkbox"
                                                    class="user-picker__check"
                                                    prop:checked=checked
                                                    on:change=move |_| toggle(user.clone())
                                                />
                                                <span class="avatar avatar--small" aria-hidden="true">
                                                    {initial(&name)}
                                                </span>
                                                {name.clone()}
                                            </label>
                                        </li>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </ul>
                    <p class="user-picker__note">"Only accounts on this server are listed."</p>
                </div>
            </Menu>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use protocol::UserId;
    use uuid::Uuid;

    use super::*;

    fn user(name: &str) -> UserRef {
        UserRef {
            id: UserId(Uuid::nil()),
            username: name.to_owned(),
        }
    }

    #[test]
    fn the_picker_filters_by_name_ignoring_case() {
        let users = [user("dana"), user("Alex"), user("maria")];
        let names = |query| {
            matching(&users, query)
                .into_iter()
                .map(|user| user.username.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(""), ["dana", "Alex", "maria"]);
        assert_eq!(names("a"), ["dana", "Alex", "maria"]);
        assert_eq!(names(" AL "), ["Alex"]);
        assert_eq!(names("zed"), Vec::<&str>::new());
    }
}
