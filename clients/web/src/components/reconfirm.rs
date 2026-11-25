//! Confirming it is the user before a sensitive change: when the server answers
//! `reauthentication-required`, a dialog asks for the password, a code from the authenticator
//! app, or a passkey or security key, and the change is sent again.

use std::future::Future;

use futures::channel::oneshot;
use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};

use api_client::{ApiError, Confirmation};

use super::ErrorAlert;
use crate::{session, webauthn};

const PASSWORD: &str = "password";
const TOTP: &str = "totp";
const WEBAUTHN: &str = "webauthn";

/// What the dialog asks for, and where its answer goes.
struct Question {
    message: String,
    methods: Vec<String>,
    answer: oneshot::Sender<bool>,
}

/// The dialog's state, shared through the context; see [`provide`] and
/// [`confirmed`](Self::confirmed).
#[derive(Clone, Copy)]
pub struct Reconfirm(RwSignal<Option<Question>, LocalStorage>);

/// Makes the dialog available to every page; [`ReconfirmDialog`] must be rendered once.
pub fn provide() {
    provide_context(Reconfirm(RwSignal::new_local(None)));
}

impl Reconfirm {
    /// The dialog's state, taken while a component is built: a task spawned later has no
    /// owner to find it through.
    pub fn get() -> Self {
        expect_context::<Self>()
    }

    /// Runs `call`, and when the server wants the user to confirm it is them first, asks them
    /// and runs it once more. `Ok(None)` when they cancelled.
    pub async fn confirmed<T, F, Fut>(self, call: F) -> Result<Option<T>, ApiError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, ApiError>>,
    {
        match call().await {
            Err(ApiError::ReauthenticationRequired { message, methods }) if !methods.is_empty() => {
                let (answer, answered) = oneshot::channel();
                self.0.set(Some(Question {
                    message,
                    methods,
                    answer,
                }));
                if answered.await != Ok(true) {
                    return Ok(None);
                }
                call().await.map(Some)
            }
            other => other.map(Some),
        }
    }
}

/// Confirms it is the user with one of their passkeys or security keys: `Ok(false)` when they
/// cancelled the browser's dialog.
async fn confirm_with_key() -> Result<bool, String> {
    let client = session::client();
    let challenge = client
        .reauth_webauthn_options()
        .await
        .map_err(|err| err.to_string())?;
    let challenge_id = challenge
        .challenge_id
        .ok_or("The server sent no challenge for the key.")?;
    let credential = match webauthn::get(challenge.options, None).await {
        Ok(credential) => credential,
        Err(failure) => return failure.message().map_or(Ok(false), Err),
    };
    client
        .reauthenticate(Confirmation::WebAuthn {
            challenge_id,
            credential: &credential,
        })
        .await
        .map(|()| true)
        .map_err(|err| err.to_string())
}

/// The dialog, shown while a change waits for the user to confirm.
#[component]
pub fn ReconfirmDialog() -> impl IntoView {
    let Reconfirm(question) = expect_context::<Reconfirm>();
    let secret = RwSignal::new(String::new());
    let use_code = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let methods = move || {
        question.with(|asked| {
            asked
                .as_ref()
                .map(|asked| asked.methods.clone())
                .unwrap_or_default()
        })
    };
    let offers = move |method: &str| methods().iter().any(|offered| offered == method);
    let by_code = move || use_code.get() || !offers(PASSWORD);
    let close = move |confirmed: bool| {
        if let Some(asked) = question.write().take() {
            let _ = asked.answer.send(confirmed);
        }
        secret.set(String::new());
        use_code.set(false);
        error.set(None);
    };
    let use_key = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let result = confirm_with_key().await;
            busy.set(false);
            match result {
                Ok(true) => close(true),
                Ok(false) => {}
                Err(message) => error.set(Some(message)),
            }
        });
    };
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let given = secret.get_untracked();
        let code = by_code();
        busy.set(true);
        spawn_local(async move {
            let proof = if code {
                Confirmation::Code(&given)
            } else {
                Confirmation::Password(&given)
            };
            let result = session::client().reauthenticate(proof).await;
            busy.set(false);
            match result {
                Ok(()) => close(true),
                Err(err) => {
                    secret.set(String::new());
                    error.set(Some(err.to_string()));
                }
            }
        });
    };

    move || {
        question.with(Option::is_some).then(|| {
            let message = question.with(|asked| {
                asked
                    .as_ref()
                    .map(|asked| asked.message.clone())
                    .unwrap_or_default()
            });
            view! {
                <div class="modal" on:click=move |_| close(false)>
                    <form
                        class="modal__panel"
                        role="dialog"
                        aria-modal="true"
                        aria-labelledby="reconfirm-title"
                        on:click=|event| event.stop_propagation()
                        on:submit=submit
                    >
                        <header class="modal__header">
                            <h2 class="modal__title" id="reconfirm-title">
                                "Confirm it is you"
                            </h2>
                        </header>
                        <p class="modal__subtitle">{message}</p>
                        <ErrorAlert message=error />
                        <label class="field">
                            <span class="field__label">
                                {move || {
                                    if by_code() { "Code from your authenticator app" } else { "Password" }
                                }}
                            </span>
                            <input
                                class="field__input"
                                type=move || if by_code() { "text" } else { "password" }
                                inputmode=move || by_code().then_some("numeric")
                                autocomplete=move || {
                                    if by_code() { "one-time-code" } else { "current-password" }
                                }
                                required
                                autofocus
                                prop:value=move || secret.get()
                                on:input:target=move |event| secret.set(event.target().value())
                            />
                        </label>
                        <Show when=move || offers(PASSWORD) && offers(TOTP)>
                            <button
                                type="button"
                                class="button button--ghost button--small"
                                on:click=move |_| {
                                    use_code.update(|code| *code = !*code);
                                    secret.set(String::new());
                                }
                            >
                                {move || {
                                    if use_code.get() {
                                        "Use your password instead"
                                    } else {
                                        "Use a code from your authenticator app instead"
                                    }
                                }}
                            </button>
                        </Show>
                        <Show when=move || offers(WEBAUTHN) && webauthn::available()>
                            <button
                                type="button"
                                class="button button--block"
                                disabled=move || busy.get()
                                on:click=use_key
                            >
                                "Use your passkey or security key"
                            </button>
                        </Show>
                        <div class="modal__actions">
                            <button type="button" class="button" on:click=move |_| close(false)>
                                "Cancel"
                            </button>
                            <button
                                type="submit"
                                class="button button--primary"
                                disabled=move || busy.get()
                            >
                                "Confirm"
                            </button>
                        </div>
                    </form>
                </div>
            }
        })
    }
}
