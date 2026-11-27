//! The second step of a sign-in whose password was right: a passkey or security key, a code
//! from the authenticator app, or a recovery code.

use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};

use api_client::{ApiError, SecondFactor, SecondFactorStep};
use protocol::{AuthMethod, ErrorCode};

use crate::{
    session::{Auth, client},
    webauthn,
};

/// What the user finishes the sign-in with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Way {
    Key,
    Code,
    Recovery,
}

impl Way {
    fn method(self) -> AuthMethod {
        match self {
            Self::Key => AuthMethod::Webauthn,
            Self::Code => AuthMethod::Totp,
            Self::Recovery => AuthMethod::Recovery,
        }
    }

    fn switch_label(self) -> &'static str {
        match self {
            Self::Key => "Use your passkey or security key instead",
            Self::Code => "Use the authenticator app instead",
            Self::Recovery => "Lost your device? Use a recovery code",
        }
    }
}

/// The ways the account can finish with, the one to start with first.
fn ways(methods: &[AuthMethod]) -> Vec<Way> {
    let key_works = webauthn::available();
    [Way::Key, Way::Code, Way::Recovery]
        .into_iter()
        .filter(|way| methods.contains(&way.method()))
        .filter(|way| *way != Way::Key || key_works)
        .collect()
}

#[component]
pub(super) fn SecondStep(
    auth: Auth,
    step: SecondFactorStep,
    error: RwSignal<Option<String>>,
    restart: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let offered = StoredValue::new(ways(&step.methods));
    let way = RwSignal::new(offered.with_value(|ways| ways.first().copied().unwrap_or(Way::Code)));
    let code = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let token = StoredValue::new(step.login_token);

    // Too many wrong codes, or too slow: the password has to be given again.
    let finished = move |result: Result<(), ApiError>| match result {
        Ok(()) => error.set(None),
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
    };
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let given = code.get_untracked();
        let by_recovery = way.get_untracked() == Way::Recovery;
        spawn_local(async move {
            let factor = if by_recovery {
                SecondFactor::RecoveryCode(&given)
            } else {
                SecondFactor::Code(&given)
            };
            let result = auth.verify(&token.get_value(), factor).await;
            busy.set(false);
            code.set(String::new());
            finished(result);
        });
    };
    let use_key = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let login_token = token.get_value();
            let result = match client().login_webauthn_options(&login_token).await {
                Ok(challenge) => match webauthn::get(challenge.options, None).await {
                    Ok(answer) => {
                        auth.verify(&login_token, SecondFactor::WebAuthn(Box::new(answer)))
                            .await
                    }
                    Err(failure) => {
                        if let Some(message) = failure.message() {
                            error.set(Some(message));
                        }
                        busy.set(false);
                        return;
                    }
                },
                Err(err) => Err(err),
            };
            busy.set(false);
            finished(result);
        });
    };

    view! {
        <div class="login__form">
            <Show
                when=move || way.get() == Way::Key
                fallback=move || {
                    view! {
                        <form class="login__form" on:submit=submit>
                            <p class="login__hint">
                                {move || {
                                    if way.get() == Way::Recovery {
                                        "Enter one of your recovery codes. Each one works once."
                                    } else {
                                        "Enter the code your authenticator app shows for Home Space."
                                    }
                                }}
                            </p>
                            <CodeField code recovery=Signal::derive(move || way.get() == Way::Recovery) />
                            <button
                                type="submit"
                                class="button button--primary button--block"
                                disabled=move || busy.get()
                            >
                                "Continue"
                            </button>
                        </form>
                    }
                }
            >
                <p class="login__hint">"Use your passkey or security key to finish signing in."</p>
                <button
                    type="button"
                    class="button button--primary button--block"
                    disabled=move || busy.get()
                    on:click=use_key
                >
                    "Use passkey or security key"
                </button>
            </Show>
            <For
                each=move || {
                    let current = way.get();
                    offered.with_value(|ways| {
                        ways.iter().copied().filter(|other| *other != current).collect::<Vec<_>>()
                    })
                }
                key=|other| *other
                let(other)
            >
                <button
                    type="button"
                    class="button button--ghost button--block"
                    on:click=move |_| {
                        way.set(other);
                        code.set(String::new());
                        error.set(None);
                    }
                >
                    {other.switch_label()}
                </button>
            </For>
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
        </div>
    }
}

/// The field for a code from the authenticator app, or a recovery code.
#[component]
fn CodeField(code: RwSignal<String>, recovery: Signal<bool>) -> impl IntoView {
    view! {
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
    }
}
