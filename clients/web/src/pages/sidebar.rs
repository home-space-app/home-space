//! The navigation down the left edge: sections, the user's storage and the account menu.

use leptos::{html::Div, prelude::*};
use leptos_router::{components::A, hooks::use_location};
use web_sys::HtmlElement;

use client_core::storage::{fraction, usage_summary};
use protocol::{Event, Me, Storage};

use crate::{
    events,
    icon::Icon,
    load, route,
    session::{self, Auth},
};

const ACCOUNT_MENU_ID: &str = "account-menu";
const POPOVER_AUTO: &str = "auto";
/// Space between the account button and its menu.
const MENU_GAP: f64 = 6.0;
const HOME_PATH: &str = "/";
const FOLDER_PREFIX: &str = "/folder/";

/// The first letter of `name`, for the round avatar.
pub fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|letter| letter.to_uppercase().collect())
        .unwrap_or_default()
}

/// How the user signs in and their role, e.g. `Local account · administrator`.
pub fn account_kind(me: &Me) -> String {
    let method = if me.external {
        "Single sign-on"
    } else {
        "Local account"
    };
    if me.is_admin {
        format!("{method} · administrator")
    } else {
        method.to_owned()
    }
}

/// The narrowest the account menu gets, and the room it keeps from the window's edges.
const MENU_MIN_WIDTH: f64 = 200.0;
const MENU_MARGIN: f64 = 8.0;

/// Where a button sits in the window, in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub bottom: f64,
    pub width: f64,
}

/// Whether the menu opens above or below its button, with its distance from that window edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vertical {
    /// `bottom` is measured from the window's bottom edge.
    Above { bottom: f64 },
    /// `top` is measured from the window's top edge.
    Below { top: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuPlacement {
    pub left: f64,
    pub width: f64,
    pub vertical: Vertical,
}

/// Where the account menu goes: above the button at the foot of the sidebar, below it in
/// the top bar of narrow windows, and never past the window's right edge.
pub fn menu_placement(button: Rect, viewport_width: f64, viewport_height: f64) -> MenuPlacement {
    let width = button
        .width
        .max(MENU_MIN_WIDTH)
        .min((viewport_width - 2.0 * MENU_MARGIN).max(0.0));
    let left = button
        .left
        .min(viewport_width - width - MENU_MARGIN)
        .max(MENU_MARGIN);
    let room_above = button.top;
    let room_below = viewport_height - button.bottom;
    let vertical = if room_above >= room_below {
        Vertical::Above {
            bottom: viewport_height - button.top + MENU_GAP,
        }
    } else {
        Vertical::Below {
            top: button.bottom + MENU_GAP,
        }
    };
    MenuPlacement {
        left,
        width,
        vertical,
    }
}

#[component]
pub fn Sidebar(auth: Auth) -> impl IntoView {
    let location = use_location();
    let storage = RwSignal::new(None::<Storage>);
    let lost_and_found = Signal::derive(move || auth.user.get().map(|me| me.lost_and_found_id));
    let found_count = RwSignal::new(0_usize);

    let usage = load::load(
        || async { session::client().storage().await },
        move |result| {
            if let Ok(loaded) = result {
                storage.set(Some(loaded));
            }
        },
    );
    let found = load::load(
        move || {
            let folder = lost_and_found.get();
            async move {
                match folder {
                    Some(id) => session::client()
                        .list(Some(id))
                        .await
                        .map(|nodes| nodes.len()),
                    None => Ok(0),
                }
            }
        },
        move |result| {
            if let Ok(count) = result {
                found_count.set(count);
            }
        },
    );
    events::use_events(move |event| {
        if !matches!(event, Event::DevicesChanged) {
            usage.refetch();
            found.refetch();
        }
    });

    let found_path = move || lost_and_found.get().map(route::folder);
    let in_found = move || {
        let path = location.pathname.get();
        found_path().is_some_and(|found| path == found)
    };
    let in_files = move || {
        let path = location.pathname.get();
        (path == HOME_PATH || path.starts_with(FOLDER_PREFIX) || path == route::SEARCH_PATH)
            && !in_found()
    };
    let in_trash = move || location.pathname.get() == route::TRASH_PATH;

    view! {
        <aside class="sidebar">
            <A href=HOME_PATH attr:class="sidebar__brand">
                <span class="sidebar__logo" aria-hidden="true">
                    <Icon name="home" />
                </span>
                "Home Space"
            </A>
            <nav class="sidebar__nav" aria-label="Sections">
                <NavLink href=HOME_PATH.to_owned() icon="folder" label="Files" active=Signal::derive(in_files) />
                <NavLink href=route::TRASH_PATH.to_owned() icon="delete" label="Recycle bin" active=Signal::derive(in_trash) />
                {move || {
                    let path = found_path()?;
                    (found_count.get() > 0 || in_found())
                        .then(|| {
                            view! {
                                <NavLink href=path icon="inventory_2" label="Lost & Found" active=Signal::derive(in_found) />
                            }
                        })
                }}
            </nav>
            <div class="sidebar__footer">
                {move || storage.get().map(|storage| view! { <StorageCard storage /> })}
                <AccountMenu auth />
            </div>
        </aside>
    }
}

#[component]
fn NavLink(
    href: String,
    icon: &'static str,
    label: &'static str,
    active: Signal<bool>,
) -> impl IntoView {
    view! {
        <a
            href=href
            class="sidebar__link"
            class:sidebar__link--active=move || active.get()
            aria-current=move || active.get().then_some("page")
            title=label
        >
            <Icon name=icon />
            <span class="sidebar__label">{label}</span>
        </a>
    }
}

#[component]
fn StorageCard(storage: Storage) -> impl IntoView {
    let used = storage.usage.total();
    let share = storage
        .disk
        .map_or(0.0, |disk| fraction(used, used + disk.available));
    view! {
        <A href=route::SETTINGS_PATH attr:class="storage-card" attr:title="Storage details">
            <span class="storage-card__head">
                <Icon name="hard_drive" />
                "Storage"
            </span>
            <span class="meter" aria-hidden="true">
                <span class="meter__fill" style=format!("width: {:.2}%", share * 100.0)></span>
            </span>
            <span class="storage-card__text">{usage_summary(&storage)}</span>
        </A>
    }
}

fn close(panel: NodeRef<Div>) {
    if let Some(element) = panel.get_untracked() {
        let element: &HtmlElement = &element;
        let _ = element.hide_popover();
    }
}

#[component]
fn AccountMenu(auth: Auth) -> impl IntoView {
    let panel = NodeRef::<Div>::new();
    let place = move |event: leptos::ev::MouseEvent| {
        use wasm_bindgen::JsCast;
        let Some(target) = event
            .current_target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let rect = target.get_bounding_client_rect();
        let window = web_sys::window();
        let size = |value: Option<wasm_bindgen::JsValue>| {
            value.and_then(|value| value.as_f64()).unwrap_or_default()
        };
        let viewport_width = size(window.as_ref().and_then(|window| window.inner_width().ok()));
        let viewport_height = size(window.as_ref().and_then(|window| window.inner_height().ok()));
        let place = menu_placement(
            Rect {
                left: rect.left(),
                top: rect.top(),
                bottom: rect.bottom(),
                width: rect.width(),
            },
            viewport_width,
            viewport_height,
        );
        if let Some(element) = panel.get_untracked() {
            let style = HtmlElement::style(&element);
            let px = |value: f64| format!("{value}px");
            let _ = style.set_property("left", &px(place.left));
            let _ = style.set_property("width", &px(place.width));
            match place.vertical {
                Vertical::Above { bottom } => {
                    let _ = style.set_property("top", "auto");
                    let _ = style.set_property("bottom", &px(bottom));
                }
                Vertical::Below { top } => {
                    let _ = style.set_property("top", &px(top));
                    let _ = style.set_property("bottom", "auto");
                }
            }
        }
    };
    let user = move || auth.user.get();
    let is_admin = move || user().is_some_and(|me| me.is_admin);

    view! {
        <button
            class="account"
            aria-haspopup="menu"
            popovertarget=ACCOUNT_MENU_ID
            on:click=place
        >
            <span class="avatar" aria-hidden="true">
                {move || user().map(|me| initial(&me.username))}
            </span>
            <span class="account__name">{move || user().map(|me| me.username)}</span>
            <Icon name="unfold_more" />
        </button>
        <div id=ACCOUNT_MENU_ID class="menu account-menu" role="menu" popover=POPOVER_AUTO node_ref=panel>
            <div class="account-menu__who">
                <span class="account-menu__name">{move || user().map(|me| me.username)}</span>
                <span class="account-menu__kind">{move || user().map(|me| account_kind(&me))}</span>
            </div>
            <hr class="menu__divider" />
            <A href=route::SETTINGS_PATH attr:class="menu__item" attr:role="menuitem" on:click=move |_| close(panel)>
                <Icon name="settings" />
                "Settings"
            </A>
            <Show when=is_admin>
                <A href=route::ADMIN_PATH attr:class="menu__item" attr:role="menuitem" on:click=move |_| close(panel)>
                    <Icon name="admin_panel_settings" />
                    "Administration"
                </A>
            </Show>
            <hr class="menu__divider" />
            <button
                class="menu__item menu__item--danger"
                role="menuitem"
                on:click=move |_| {
                    close(panel);
                    auth.logout();
                }
            >
                <Icon name="logout" />
                "Log out"
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use protocol::{NodeId, UserId};
    use uuid::Uuid;

    use super::*;

    fn me(is_admin: bool, external: bool) -> Me {
        Me {
            id: UserId(Uuid::nil()),
            username: "sam".to_owned(),
            lost_and_found_id: NodeId(Uuid::nil()),
            is_admin,
            external,
        }
    }

    #[test]
    fn avatar_shows_the_capital_first_letter() {
        assert_eq!(initial("sam"), "S");
        assert_eq!(initial("élise"), "É");
        assert_eq!(initial(""), "");
    }

    #[test]
    fn menu_opens_above_the_button_at_the_foot_of_the_sidebar() {
        let button = Rect {
            left: 12.0,
            top: 850.0,
            bottom: 886.0,
            width: 216.0,
        };
        let place = menu_placement(button, 1440.0, 900.0);
        assert_eq!(place.left, 12.0);
        assert_eq!(place.width, 216.0);
        assert_eq!(place.vertical, Vertical::Above { bottom: 56.0 });
    }

    #[test]
    fn menu_opens_below_the_button_in_the_top_bar_and_stays_on_screen() {
        let button = Rect {
            left: 300.0,
            top: 8.0,
            bottom: 44.0,
            width: 60.0,
        };
        let place = menu_placement(button, 375.0, 812.0);
        assert_eq!(place.vertical, Vertical::Below { top: 50.0 });
        assert_eq!(place.width, 200.0);
        assert_eq!(place.left, 375.0 - 200.0 - 8.0);
    }

    #[test]
    fn account_kind_names_method_and_role() {
        assert_eq!(
            account_kind(&me(true, false)),
            "Local account · administrator"
        );
        assert_eq!(account_kind(&me(false, true)), "Single sign-on");
    }
}
