//! The panel that picks people on the server: a search field over everyone, each with a
//! checkbox.

use leptos::prelude::*;

use client_core::{people::matching, shared_link::initial};
use protocol::{UserId, UserRef};

use crate::icon::Icon;

const SEARCH_PEOPLE: &str = "Search people";
const NOBODY_MATCHES: &str = "Nobody else matches.";
const SERVER_ONLY: &str = "Only accounts on this server are listed.";

/// Lists `users` with the `picked` ones checked; `toggle` runs for the one clicked. Meant to
/// sit in a [`Menu`](super::Menu).
#[component]
pub fn UserPicker(
    #[prop(into)] users: Signal<Vec<UserRef>>,
    #[prop(into)] picked: Signal<Vec<UserId>>,
    toggle: Callback<UserRef>,
) -> impl IntoView {
    let query = RwSignal::new(String::new());
    view! {
        <div class="user-picker">
            <label class="user-picker__search">
                <Icon name="search" />
                <input
                    class="user-picker__input"
                    type="search"
                    placeholder=SEARCH_PEOPLE
                    aria-label=SEARCH_PEOPLE
                    prop:value=move || query.get()
                    on:input:target=move |event| query.set(event.target().value())
                />
            </label>
            <ul class="user-picker__list">
                {move || {
                    let users = users.read();
                    let shown = matching(&users, &query.read());
                    if shown.is_empty() {
                        return view! { <li class="user-picker__empty">{NOBODY_MATCHES}</li> }
                            .into_any();
                    }
                    shown
                        .into_iter()
                        .cloned()
                        .map(|user| {
                            let user_id = user.id;
                            let checked = move || picked.read().contains(&user_id);
                            let name = user.username.clone();
                            view! {
                                <li>
                                    <label class="user-picker__row">
                                        <input
                                            type="checkbox"
                                            class="user-picker__check"
                                            prop:checked=checked
                                            on:change=move |_| toggle.run(user.clone())
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
            <p class="user-picker__note">{SERVER_ONLY}</p>
        </div>
    }
}

#[cfg(test)]
mod tests {
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
