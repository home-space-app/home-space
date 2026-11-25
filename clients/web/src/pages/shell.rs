use leptos::prelude::*;
use leptos_router::components::{Outlet, Redirect};

use super::{
    security::{FEW_RECOVERY_CODES, recovery_left},
    sidebar::Sidebar,
    viewer::FileViewer,
};
use crate::{route, session::Auth};

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
                    <FewRecoveryCodes auth />
                    <Outlet />
                </div>
            </div>
            <FileViewer auth />
        </Show>
    }
}

/// A reminder to make new recovery codes once few are left, so the user can still sign in
/// when they lose the authenticator app.
#[component]
fn FewRecoveryCodes(auth: Auth) -> impl IntoView {
    let left = move || {
        auth.user.with(|me| {
            me.as_ref()
                .map(|me| me.second_factors)
                .filter(|factors| factors.totp && factors.recovery_codes_left <= FEW_RECOVERY_CODES)
                .map(|factors| factors.recovery_codes_left)
        })
    };
    move || {
        left().map(|left| {
            view! {
                <p class="alert alert--warning banner" role="status">
                    {recovery_left(left)}
                    " "
                    <leptos_router::components::A href=route::SETTINGS_PATH>
                        "Make new ones in Settings."
                    </leptos_router::components::A>
                </p>
            }
        })
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
