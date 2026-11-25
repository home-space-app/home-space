use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use leptos_router::components::Redirect;
use web_sys::AbortController;

use api_client::{ApiError, SecondFactorStep};
use protocol::Providers;

use crate::{
    components::ErrorAlert,
    session::{Auth, after_sign_in, client, in_place},
    webauthn,
};

mod second_step;

use second_step::SecondStep;

/// The passkey request that waits in the username field's autofill while the form is open.
#[derive(Clone, Copy)]
struct QuietPasskey(StoredValue<Option<AbortController>, LocalStorage>);

impl QuietPasskey {
    fn new() -> Self {
        Self(StoredValue::new_local(None))
    }

    /// Offers the browser's passkeys in the autofill, where it can; picking one signs in.
    fn start(self, auth: Auth, error: RwSignal<Option<String>>) {
        spawn_local(async move {
            if !webauthn::conditional_available().await {
                return;
            }
            let Ok(controller) = AbortController::new() else {
                return;
            };
            let signal = controller.signal();
            self.stop();
            self.0.set_value(Some(controller));
            if let Err(message) = auth.passkey_sign_in(Some(&signal)).await {
                error.set(Some(message));
            }
        });
    }

    /// Withdraws the autofill request, so another one (the passkey button's) can be made.
    fn stop(self) {
        if let Some(controller) = self.0.try_update_value(Option::take).flatten() {
            controller.abort();
        }
    }
}

#[component]
pub fn Login(auth: Auth) -> impl IntoView {
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    // Until the server answers, show the password form so a server without single sign-on works.
    let providers = RwSignal::new(None::<Providers>);
    let quiet = QuietPasskey::new();
    on_cleanup(move || quiet.stop());
    spawn_local(async move {
        match client().providers().await {
            Ok(found) => {
                // The autofill offers passkeys in the username field, which only the password
                // form has.
                if found.passkeys && found.password && webauthn::available() {
                    quiet.start(auth, error);
                }
                providers.set(Some(found));
            }
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
                .and_then(Providers::identity_provider)
                .map(|oidc| oidc.name.clone())
        })
    };
    let passkeys = move || {
        providers.with(|found| found.as_ref().is_some_and(|found| found.passkeys))
            && webauthn::available()
    };
    let sign_in_with_passkey = move |_| {
        quiet.stop();
        busy.set(true);
        spawn_local(async move {
            let result = auth.passkey_sign_in(None).await;
            busy.set(false);
            match result {
                Ok(true) => {
                    error.set(None);
                    return;
                }
                Ok(false) => {}
                Err(message) => error.set(Some(message)),
            }
            if show_password() {
                quiet.start(auth, error);
            }
        });
    };
    let sign_in_with_provider = move |_| {
        if let Err(message) = Auth::start_oidc() {
            error.set(Some(message));
        }
    };

    // Set once the password was right and the account also needs a second factor.
    let second_step = RwSignal::new(None::<SecondFactorStep>);
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        quiet.stop();
        busy.set(true);
        spawn_local(async move {
            let result = auth
                .login(&username.get_untracked(), &password.get_untracked())
                .await;
            busy.set(false);
            match result {
                Ok(step) => {
                    error.set(None);
                    second_step.set(step);
                }
                Err(message) => error.set(Some(message)),
            }
            password.set(String::new());
        });
    };

    view! {
        <Show when=move || auth.user.get().is_some()>
            <Redirect path=after_sign_in() options=in_place() />
        </Show>
        <main class="login">
            <section class="login__card">
                <header class="login__header">
                    <h1 class="login__title">"Home Space"</h1>
                    <p class="login__subtitle">"Sign in to your files"</p>
                </header>
                <ErrorAlert message=error />
                {move || {
                    second_step.with(Option::is_none).then(oidc_name).flatten()
                        .map(|name| {
                            view! {
                                <button
                                    type="button"
                                    class="button button--primary button--block"
                                    on:click=sign_in_with_provider
                                >
                                    {format!("Sign in with {name}")}
                                </button>
                                <Show when=move || show_password() || passkeys()>
                                    <p class="login__divider">"or"</p>
                                </Show>
                            }
                        })
                }}
                <Show when=move || passkeys() && second_step.with(Option::is_none)>
                    <button
                        type="button"
                        class="button button--block"
                        disabled=move || busy.get()
                        on:click=sign_in_with_passkey
                    >
                        "Sign in with a passkey"
                    </button>
                    <Show when=show_password>
                        <p class="login__divider">"or"</p>
                    </Show>
                </Show>
                {move || {
                    second_step
                        .get()
                        .map(|step| {
                            view! {
                                <SecondStep
                                    auth
                                    step
                                    error
                                    restart=move || second_step.set(None)
                                />
                            }
                        })
                }}
                <form
                    class="login__form"
                    on:submit=submit
                    hidden=move || !show_password() || second_step.with(Option::is_some)
                >
                    <label class="field">
                        <span class="field__label">"Username"</span>
                        <input
                            class="field__input"
                            type="text"
                            autocomplete="username webauthn"
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
