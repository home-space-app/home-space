use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use leptos_router::components::Redirect;

use api_client::ApiError;
use protocol::Providers;

use crate::{
    components::ErrorAlert,
    session::{Auth, after_sign_in, client},
};

#[component]
pub fn Login(auth: Auth) -> impl IntoView {
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    // Until the server answers, show the password form so a server without single sign-on works.
    let providers = RwSignal::new(None::<Providers>);
    spawn_local(async move {
        match client().providers().await {
            Ok(found) => providers.set(Some(found)),
            Err(
                ApiError::Unauthorized
                | ApiError::Api { .. }
                | ApiError::Status { .. }
                | ApiError::Decode(_),
            ) => {}
            Err(err) => error.set(Some(err.to_string())),
        }
    });
    let show_password =
        move || providers.with(|found| found.as_ref().is_none_or(|found| found.password));
    let oidc_name = move || {
        providers.with(|found| {
            found
                .as_ref()
                .and_then(|found| found.oidc.as_ref())
                .map(|oidc| oidc.name.clone())
        })
    };
    let sign_in_with_provider = move |_| {
        if let Err(message) = Auth::start_oidc() {
            error.set(Some(message));
        }
    };

    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        spawn_local(async move {
            let result = auth
                .login(&username.get_untracked(), &password.get_untracked())
                .await;
            busy.set(false);
            match result {
                Ok(()) => error.set(None),
                Err(message) => error.set(Some(message)),
            }
            password.set(String::new());
        });
    };

    view! {
        <Show when=move || auth.user.get().is_some()>
            <Redirect path=after_sign_in() />
        </Show>
        <main class="login">
            <section class="login__card">
                <header class="login__header">
                    <h1 class="login__title">"Home Space"</h1>
                    <p class="login__subtitle">"Sign in to your files"</p>
                </header>
                <ErrorAlert message=error />
                {move || {
                    oidc_name()
                        .map(|name| {
                            view! {
                                <button
                                    type="button"
                                    class="button button--primary button--block"
                                    on:click=sign_in_with_provider
                                >
                                    {format!("Sign in with {name}")}
                                </button>
                                <Show when=show_password>
                                    <p class="login__divider">"or"</p>
                                </Show>
                            }
                        })
                }}
                <form class="login__form" on:submit=submit hidden=move || !show_password()>
                    <label class="field">
                        <span class="field__label">"Username"</span>
                        <input
                            class="field__input"
                            type="text"
                            autocomplete="username"
                            required
                            prop:value=move || username.get()
                            on:input:target=move |event| username.set(event.target().value())
                        />
                    </label>
                    <label class="field">
                        <span class="field__label">"Password"</span>
                        <input
                            class="field__input"
                            type="password"
                            autocomplete="current-password"
                            required
                            prop:value=move || password.get()
                            on:input:target=move |event| password.set(event.target().value())
                        />
                    </label>
                    <button
                        type="submit"
                        class="button button--primary button--block"
                        disabled=move || busy.get()
                    >
                        "Sign in"
                    </button>
                </form>
            </section>
        </main>
    }
}
