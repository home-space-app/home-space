use leptos::{prelude::*, task::spawn_local};
use leptos_router::{
    components::{A, Redirect},
    hooks::use_query_map,
};

use crate::{
    components::ErrorAlert,
    session::{Auth, take_oidc_verifier},
};

const ACCESS_DENIED_MESSAGE: &str = "You are not allowed to sign in with this account.";
const DISABLED_MESSAGE: &str = "This account is disabled. Ask an administrator to enable it.";
const FAILED_MESSAGE: &str = "Signing in failed. Please try again.";
const EXPIRED_MESSAGE: &str = "This sign-in has expired or was already used. Please sign in again.";

/// Where the server sends the browser after single sign-on: trades the one-time `code` in the
/// query for a session, or explains why there is none.
#[component]
pub fn OidcCallback(auth: Auth) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let query = use_query_map().get_untracked();
    match (query.get("code"), query.get("error")) {
        (Some(code), _) if !code.is_empty() => {
            spawn_local(async move {
                if let Err(message) = auth.finish_oidc(&code).await {
                    error.set(Some(message));
                }
            });
        }
        (_, Some(reason)) => {
            // The sign-in is over, so the saved verifier is no longer needed.
            let _ = take_oidc_verifier();
            error.set(Some(
                match reason.as_str() {
                    "access_denied" => ACCESS_DENIED_MESSAGE,
                    "login_expired" => EXPIRED_MESSAGE,
                    "account_disabled" => DISABLED_MESSAGE,
                    _ => FAILED_MESSAGE,
                }
                .to_owned(),
            ));
        }
        _ => error.set(Some(FAILED_MESSAGE.to_owned())),
    }

    view! {
        <Show when=move || auth.user.get().is_some()>
            <Redirect path="/" />
        </Show>
        <main class="login">
            <section class="login__card">
                <header class="login__header">
                    <h1 class="login__title">"home-space"</h1>
                    <p class="login__subtitle">
                        {move || {
                            if error.with(Option::is_some) { "Sign-in failed" } else { "Signing you in…" }
                        }}
                    </p>
                </header>
                <ErrorAlert message=error />
                <Show when=move || error.with(Option::is_some)>
                    <A href="/login" attr:class="button button--primary button--block">
                        "Back to sign in"
                    </A>
                </Show>
            </section>
        </main>
    }
}
