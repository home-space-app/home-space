use std::time::Duration;

use gloo_timers::future::sleep;
use leptos::{prelude::*, task::spawn_local};
use leptos_router::{
    NavigateOptions,
    hooks::{use_location, use_navigate, use_query_map},
};
use web_sys::KeyboardEvent;

use crate::route;

const KEY_ENTER: &str = "Enter";
const KEY_ESCAPE: &str = "Escape";
const HOME_PATH: &str = "/";
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);

/// Searches as the user types and opens the results page.
#[component]
pub fn SearchBox() -> impl IntoView {
    let navigate = use_navigate();
    let location = use_location();
    let query = use_query_map();
    let text = RwSignal::new(String::new());
    let pushed = StoredValue::new(String::new());
    let generation = StoredValue::new(0_u32);
    let on_search_page = move || {
        location
            .pathname
            .with_untracked(|path| path == route::SEARCH_PATH)
    };

    Effect::new(move |_| {
        let searching = location.pathname.with(|path| path == route::SEARCH_PATH);
        let current = query.read().get(route::SEARCH_PARAM).unwrap_or_default();
        if !searching {
            pushed.set_value(String::new());
            text.set(String::new());
        } else if current != pushed.get_value() {
            pushed.set_value(current.clone());
            text.set(current);
        }
    });

    let go = Callback::new(move |value: String| {
        let value = value.trim().to_owned();
        pushed.set_value(value.clone());
        if !value.is_empty() {
            let options = NavigateOptions {
                replace: on_search_page(),
                ..NavigateOptions::default()
            };
            navigate(&route::search(&value), options);
        } else if on_search_page() {
            navigate(HOME_PATH, NavigateOptions::default());
        }
    });
    let cancel_pending = move || generation.update_value(|count| *count += 1);

    view! {
        <input
            class="field__input search-box"
            type="search"
            placeholder="Search files and folders"
            aria-label="Search files and folders"
            autocomplete="off"
            prop:value=move || text.get()
            on:input:target=move |event| {
                let value = event.target().value();
                text.set(value.clone());
                cancel_pending();
                let ticket = generation.get_value();
                spawn_local(async move {
                    sleep(SEARCH_DEBOUNCE).await;
                    if generation.get_value() == ticket {
                        go.run(value);
                    }
                });
            }
            on:keydown=move |event: KeyboardEvent| {
                match event.key().as_str() {
                    KEY_ENTER => {
                        cancel_pending();
                        go.run(text.get_untracked());
                    }
                    KEY_ESCAPE => {
                        cancel_pending();
                        text.set(String::new());
                        go.run(String::new());
                    }
                    _ => {}
                }
            }
        />
    }
}
