use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use leptos_router::components::Redirect;

use api_client::{ApiError, SecondFactor, SecondFactorStep};
use protocol::{ErrorCode, Providers};

use crate::{
    components::ErrorAlert,
    session::{Auth, after_sign_in, client, in_place},
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
                .and_then(Providers::identity_provider)
                .map(|oidc| oidc.name.clone())
        })
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
                                <Show when=show_password>
                                    <p class="login__divider">"or"</p>
                                </Show>
                            }
                        })
                }}
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

/// The second step of a sign-in: a code from the authenticator app, or a recovery code.
#[component]
fn SecondStep(
    auth: Auth,
    step: SecondFactorStep,
    error: RwSignal<Option<String>>,
    restart: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let code = RwSignal::new(String::new());
    let recovery = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let offers_recovery = step.methods.iter().any(|method| method == "recovery");
    let token = StoredValue::new(step.login_token);
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let given = code.get_untracked();
        let by_recovery = recovery.get_untracked();
        spawn_local(async move {
            let factor = if by_recovery {
                SecondFactor::RecoveryCode(&given)
            } else {
                SecondFactor::Code(&given)
            };
            let result = auth.verify(&token.get_value(), factor).await;
            busy.set(false);
            code.set(String::new());
            match result {
                Ok(()) => error.set(None),
                // Too many wrong codes, or too slow: the password has to be given again.
                Err(
                    err @ ApiError::Api {
                        code: ErrorCode::Gone,
                        ..
                    },
                ) => {
                    error.set(Some(err.to_string()));
                    restart();
                }
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };
    view! {
        <form class="login__form" on:submit=submit>
            <p class="login__hint">
                {move || {
                    if recovery.get() {
                        "Enter one of your recovery codes. Each one works once."
                    } else {
                        "Enter the code your authenticator app shows for Home Space."
                    }
                }}
            </p>
            <label class="field">
                <span class="field__label">
                    {move || if recovery.get() { "Recovery code" } else { "Code" }}
                </span>
                <input
                    class="field__input"
                    type="text"
                    inputmode=move || (!recovery.get()).then_some("numeric")
                    autocomplete=move || (!recovery.get()).then_some("one-time-code")
                    autocapitalize="off"
                    spellcheck="false"
                    required
                    autofocus
                    prop:value=move || code.get()
                    on:input:target=move |event| code.set(event.target().value())
                />
            </label>
            <button
                type="submit"
                class="button button--primary button--block"
                disabled=move || busy.get()
            >
                "Continue"
            </button>
            {offers_recovery
                .then(|| {
                    view! {
                        <button
                            type="button"
                            class="button button--ghost button--block"
                            on:click=move |_| {
                                recovery.update(|on| *on = !*on);
                                code.set(String::new());
                                error.set(None);
                            }
                        >
                            {move || {
                                if recovery.get() {
                                    "Use the authenticator app instead"
                                } else {
                                    "Lost your phone? Use a recovery code"
                                }
                            }}
                        </button>
                    }
                })}
            <button
                type="button"
                class="button button--ghost button--block"
                on:click=move |_| {
                    error.set(None);
                    restart();
                }
            >
                "Start over"
            </button>
        </form>
    }
}
