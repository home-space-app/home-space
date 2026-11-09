use leptos::prelude::*;
use leptos_router::components::{Outlet, Redirect};

use super::sidebar::Sidebar;
use crate::session::Auth;

#[component]
pub fn Shell(auth: Auth) -> impl IntoView {
    view! {
        <Show
            when=move || auth.user.get().is_some()
            fallback=|| view! { <Redirect path="/login" /> }
        >
            <div class="layout">
                <Sidebar auth />
                <div class="layout__main">
                    <Outlet />
                </div>
            </div>
        </Show>
    }
}

/// A row of links between the pages of one section, e.g. the settings.
#[component]
pub fn Tabs(children: Children) -> impl IntoView {
    view! {
        <nav class="tabs" aria-label="Sections of this page">
            {children()}
        </nav>
    }
}

/// One link of [`Tabs`]; marked current when its path is open.
#[component]
pub fn Tab(href: &'static str, icon: &'static str, label: &'static str) -> impl IntoView {
    view! {
        <leptos_router::components::A href=href exact=true attr:class="tabs__tab">
            <crate::icon::Icon name=icon />
            {label}
        </leptos_router::components::A>
    }
}
