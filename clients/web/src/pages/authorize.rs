//! Where an app's browser sign-in lands (`/authorize?flow=…`): the user signs in here if they
//! have not, then lets the app in or turns it away, and the browser goes back to the app.

use leptos::{prelude::*, task::spawn_local};
use leptos_router::{components::Redirect, hooks::use_query_map};

use api_client::ApiError;
use protocol::{BrowserLoginApp, BrowserLoginFlow, ErrorCode, paths};

use crate::{
    components::ErrorAlert,
    session::{Auth, client, return_after_sign_in},
};

const EXPIRED_MESSAGE: &str =
    "This sign-in request has expired or was already answered. Start signing in again in the app.";
const MISSING_MESSAGE: &str = "This link is incomplete. Start signing in again in the app.";
const APPROVED_MESSAGE: &str = "You are signed in. You can close this tab and go back to the app.";
const DENIED_MESSAGE: &str = "The app was not signed in. You can close this tab.";
const NO_WAY_BACK_MESSAGE: &str = "Could not go back to the app.";

/// How far the user is with the app's request.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Loading,
    Asking(BrowserLoginFlow),
    Sending,
    Done(&'static str),
    Failed(String),
}

#[component]
pub fn AuthorizePage(auth: Auth) -> impl IntoView {
    let flow = use_query_map()
        .get_untracked()
        .get("flow")
        .filter(|flow| is_flow_id(flow));
    let Some(flow) = flow else {
        return view! {
            <Card>
                <p class="alert alert--error" role="alert">{MISSING_MESSAGE}</p>
            </Card>
        }
        .into_any();
    };
    let step = RwSignal::new(Step::Loading);
    let this_page = format!("{}?flow={flow}", paths::WEB_AUTHORIZE);

    // Once signed in (again), ask the server which app is waiting.
    let load = flow.clone();
    Effect::new(move |_| {
        if auth.user.with(Option::is_some) && step.get_untracked() == Step::Loading {
            let flow = load.clone();
            spawn_local(async move {
                step.set(match client().browser_login_flow(&flow).await {
                    Ok(found) => Step::Asking(found),
                    Err(err) => Step::Failed(message(auth, &err)),
                });
            });
        }
    });
    let answer = move |approve: bool| {
        let flow = flow.clone();
        step.set(Step::Sending);
        spawn_local(async move {
            let sent = if approve {
                client().browser_login_approve(&flow).await
            } else {
                client().browser_login_deny(&flow).await
            };
            step.set(match sent {
                Ok(redirect) => go_back_to_app(&redirect, approve),
                Err(err) => Step::Failed(message(auth, &err)),
            });
        });
    };

    view! {
        // Signed out: sign in first, whichever way, and come back here.
        {move || {
            auth.user
                .with(Option::is_none)
                .then(|| {
                    return_after_sign_in(&this_page);
                    view! { <Redirect path="/login" /> }
                })
        }}
        <Card>
            <Question auth step answer />
        </Card>
    }
    .into_any()
}

#[component]
fn Card(children: Children) -> impl IntoView {
    view! {
        <main class="login">
            <section class="login__card">
                <header class="login__header">
                    <h1 class="login__title">"Home Space"</h1>
                    <p class="login__subtitle">"Sign in to an app"</p>
                </header>
                {children()}
            </section>
        </main>
    }
}

/// The question about the app, the buttons that answer it, and how it went.
#[component]
fn Question(
    auth: Auth,
    step: RwSignal<Step>,
    answer: impl Fn(bool) + Clone + Send + Sync + 'static,
) -> impl IntoView {
    let error = Signal::derive(move || match step.get() {
        Step::Failed(message) => Some(message),
        _ => None,
    });
    let asking = move || matches!(step.get(), Step::Asking(_) | Step::Sending);
    let sending = move || step.get() == Step::Sending;
    let app = RwSignal::new(String::new());
    Effect::new(move |_| {
        if let Step::Asking(flow) = step.get() {
            app.set(describe(&flow));
        }
    });
    let username = move || {
        auth.user
            .with(|me| me.as_ref().map(|me| me.username.clone()))
    };
    let use_another_account = move |_| {
        step.set(Step::Loading);
        auth.logout();
    };
    let deny = answer.clone();
    view! {
        <ErrorAlert message=error />
        {move || match step.get() {
            Step::Loading => Some(view! { <p class="login__subtitle">"Loading…"</p> }.into_any()),
            Step::Done(message) => Some(view! { <p class="login__text">{message}</p> }.into_any()),
            _ => None,
        }}
        <Show when=asking>
            <p class="login__text">
                {move || app.get()} " wants to sign in as " <strong>{username}</strong> "."
            </p>
            <p class="login__hint">
                "Continue only if you just started signing in from that app yourself."
            </p>
            <div class="login__actions">
                <button
                    type="button"
                    class="button button--primary button--block"
                    disabled=sending
                    on:click={
                        let answer = answer.clone();
                        move |_| answer(true)
                    }
                >
                    "Continue"
                </button>
                <button
                    type="button"
                    class="button button--ghost button--block"
                    disabled=sending
                    on:click={
                        let deny = deny.clone();
                        move |_| deny(false)
                    }
                >
                    "Cancel"
                </button>
            </div>
            <button
                type="button"
                class="login__switch"
                disabled=sending
                on:click=use_another_account
            >
                "Use another account"
            </button>
        </Show>
    }
}

/// Whether `flow` looks like an id the server hands out: base64url and nothing else, so it
/// can go back into this page's URL as it is.
fn is_flow_id(flow: &str) -> bool {
    !flow.is_empty()
        && flow
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// "The Home Space desktop app on “Laptop”", from what the app said about itself.
fn describe(flow: &BrowserLoginFlow) -> String {
    let app = match flow.app {
        BrowserLoginApp::Desktop => "The Home Space desktop app",
        BrowserLoginApp::Mobile => "The Home Space mobile app",
    };
    match &flow.device_name {
        Some(name) => format!("{app} on “{name}”"),
        None => app.to_owned(),
    }
}

/// Sends the browser to the app's redirect, which hands the app its code or the refusal.
fn go_back_to_app(redirect: &str, approved: bool) -> Step {
    let sent = web_sys::window().is_some_and(|window| window.location().replace(redirect).is_ok());
    match (sent, approved) {
        (false, _) => Step::Failed(NO_WAY_BACK_MESSAGE.to_owned()),
        (true, true) => Step::Done(APPROVED_MESSAGE),
        (true, false) => Step::Done(DENIED_MESSAGE),
    }
}

/// A failed call as the user reads it; an ended session sends them to sign in again.
fn message(auth: Auth, err: &ApiError) -> String {
    match err {
        ApiError::Api {
            code: ErrorCode::Gone,
            ..
        } => EXPIRED_MESSAGE.to_owned(),
        _ => auth.fail(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_is_described_by_kind_and_device() {
        let flow = BrowserLoginFlow {
            app: BrowserLoginApp::Desktop,
            device_name: Some("Laptop".to_owned()),
        };
        assert_eq!(describe(&flow), "The Home Space desktop app on “Laptop”");
        let flow = BrowserLoginFlow {
            app: BrowserLoginApp::Mobile,
            device_name: None,
        };
        assert_eq!(describe(&flow), "The Home Space mobile app");
    }

    #[test]
    fn only_server_flow_ids_are_taken() {
        assert!(is_flow_id("aB3-_x"));
        assert!(!is_flow_id(""));
        assert!(!is_flow_id("abc&next=/evil"));
        assert!(!is_flow_id("a b"));
    }
}
