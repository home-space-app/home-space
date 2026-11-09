use leptos::prelude::*;
use leptos_router::{
    components::{ParentRoute, Route, Router, Routes},
    path,
};

use crate::{
    events,
    pages::{
        AdminPage, Browser, DevicesPage, Login, OidcCallback, SearchPage, SettingsPage, Shell,
        TransferList, TransfersPage, TrashPage,
    },
    session::Auth,
    uploads::{Sending, UploadItem, UploadPanel},
};

#[component]
pub fn App() -> impl IntoView {
    let auth = Auth::start();
    let uploads = RwSignal::new(Vec::<UploadItem>::new());
    provide_context(uploads);
    provide_context(Sending(RwSignal::new(Default::default())));
    let hub = events::provide_hub();
    Effect::new(move |_| {
        if auth.user.read().is_some() {
            hub.with_value(events::Hub::start);
        } else {
            hub.with_value(events::Hub::stop);
        }
    });
    view! {
        <Router>
            <Show
                when=move || !auth.checking.get()
                fallback=|| view! { <p class="status">"Loading…"</p> }
            >
                <Routes fallback=|| view! { <p class="status">"Page not found"</p> }>
                    <Route path=path!("/login") view=move || view! { <Login auth /> } />
                    <Route
                        path=path!("/login/callback")
                        view=move || view! { <OidcCallback auth /> }
                    />
                    <ParentRoute path=path!("") view=move || view! { <Shell auth /> }>
                        <Route path=path!("/") view=move || view! { <Browser auth /> } />
                        <Route path=path!("/folder/:id") view=move || view! { <Browser auth /> } />
                        <Route path=path!("/search") view=move || view! { <SearchPage auth /> } />
                        <Route path=path!("/trash") view=move || view! { <TrashPage auth /> } />
                        <Route
                            path=path!("/settings")
                            view=move || view! { <SettingsPage auth /> }
                        />
                        <Route
                            path=path!("/settings/devices")
                            view=move || view! { <DevicesPage auth /> }
                        />
                        // Where the devices page used to be.
                        <Route
                            path=path!("/devices")
                            view=move || view! { <DevicesPage auth /> }
                        />
                        <Route
                            path=path!("/transfers")
                            view=move || {
                                view! { <TransfersPage auth list=TransferList::Incoming /> }
                            }
                        />
                        <Route
                            path=path!("/transfers/sent")
                            view=move || view! { <TransfersPage auth list=TransferList::Sent /> }
                        />
                        <Route path=path!("/admin") view=move || view! { <AdminPage auth /> } />
                    </ParentRoute>
                </Routes>
            </Show>
            <Show when=move || auth.user.read().is_some()>
                <UploadPanel uploads />
            </Show>
        </Router>
    }
}
