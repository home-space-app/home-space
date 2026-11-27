//! Two-factor sign-in in the user's settings: the authenticator app, sign-in and folder keys
//! (in `keys`), and the recovery codes. A user linked to the identity provider has only their
//! folder keys here: the provider keeps the rest.

use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use qrcode::{QrCode, render::svg};

use api_client::ApiError;
use protocol::{RecoveryCodes, TotpEnrolment, features};

use crate::{
    components::{ErrorAlert, reconfirm::Reconfirm},
    session::{self, Auth},
};

mod keys;

use keys::{FolderKeysSection, KeysSection};

/// Few enough recovery codes left to suggest making new ones.
pub const FEW_RECOVERY_CODES: u32 = 3;
/// What the provider is called until the server names it.
const SOME_PROVIDER: &str = "your identity provider";

/// Where the card is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// Showing whether the app is on.
    Overview,
    /// A new app's secret, waiting for its first code.
    Adding(TotpEnrolment),
    /// Recovery codes to write down, shown once.
    Codes(Vec<String>),
}

/// Whether the server offers two-factor sign-in. A user linked to the identity provider is
/// shown that the provider keeps theirs, beside their folder keys.
pub fn offered(auth: Auth) -> bool {
    auth.user.get().is_some() && session::client().supports(features::TWO_FACTOR)
}

/// Where a user linked to the identity provider manages how they sign in.
fn managed_by(provider: Option<&str>) -> String {
    format!(
        "Signed in through {}. Your sign-in, two-factor and passkeys are managed there.",
        provider.unwrap_or(SOME_PROVIDER)
    )
}

#[component]
pub fn SecurityCard(auth: Auth) -> impl IntoView {
    let step = RwSignal::new(Step::Overview);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let confirm_removal = RwSignal::new(false);
    let reconfirm = Reconfirm::get();
    let factors = move || {
        auth.user
            .get()
            .map(|me| me.second_factors)
            .unwrap_or_default()
    };

    let run = move |action: Action| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let result = action.run(reconfirm).await;
            busy.set(false);
            match result {
                Ok(Some(next)) => {
                    let _ = auth.reload().await;
                    step.set(next);
                }
                Ok(None) => {}
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    };

    let passkeys = move || session::client().supports(features::PASSKEYS);
    let folders = move || session::client().supports(features::PROTECTED_FOLDERS);
    let show_codes = Callback::new(move |codes: Vec<String>| step.set(Step::Codes(codes)));
    let external = move || auth.user.get().is_some_and(|me| me.external);
    let provider = RwSignal::new(None::<String>);
    if auth.user.get_untracked().is_some_and(|me| me.external) {
        spawn_local(async move {
            if let Ok(found) = session::client().providers().await {
                provider.set(found.identity_provider().map(|oidc| oidc.name.clone()));
            }
        });
    }

    view! {
        <section class="card settings-grid__wide">
            <h2 class="card__title">"Two-factor sign-in and passkeys"</h2>
            <ErrorAlert message=error />
            {move || match step.get() {
                Step::Overview if external() => {
                    view! {
                        <p class="card__text">{move || managed_by(provider.get().as_deref())}</p>
                        <Show when=folders>
                            <FolderKeysSection auth error />
                        </Show>
                    }
                        .into_any()
                }
                Step::Overview => {
                    view! {
                        <p class="card__text">
                            {move || {
                                if factors().enabled() {
                                    "On. Signing in with your password also takes a passkey, a security key or a code from your authenticator app."
                                } else {
                                    "Off. Add an authenticator app or a passkey, so that a stolen password alone cannot open your account."
                                }
                            }}
                        </p>
                        <h3 class="card__heading">"Authenticator app"</h3>
                        <AppSection
                            on=Signal::derive(move || factors().totp)
                            busy
                            confirm_removal
                            run
                        />
                        <Show when=passkeys>
                            <KeysSection auth error on_codes=show_codes />
                        </Show>
                        <Show when=folders>
                            <FolderKeysSection auth error />
                        </Show>
                        <Show when=move || factors().enabled()>
                            <h3 class="card__heading">"Recovery codes"</h3>
                            <p
                                class="card__text"
                                class:card__text--warning=move || {
                                    factors().recovery_codes_left <= FEW_RECOVERY_CODES
                                }
                            >
                                {move || recovery_left(factors().recovery_codes_left)}
                            </p>
                            <div class="card__actions">
                                <button
                                    class="button"
                                    disabled=move || busy.get()
                                    on:click=move |_| run(Action::NewCodes)
                                >
                                    "New recovery codes"
                                </button>
                            </div>
                        </Show>
                    }
                        .into_any()
                }
                Step::Adding(enrolment) => {
                    view! { <AddApp enrolment busy run cancel=move || step.set(Step::Overview) /> }
                        .into_any()
                }
                Step::Codes(codes) => {
                    view! { <ShowCodes codes done=move || step.set(Step::Overview) /> }.into_any()
                }
            }}
        </section>
    }
}

/// Whether the authenticator app is set up, with the button to add or remove it.
#[component]
fn AppSection(
    on: Signal<bool>,
    busy: RwSignal<bool>,
    confirm_removal: RwSignal<bool>,
    run: impl Fn(Action) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    move || {
        if !on.get() {
            return view! {
                <p class="card__text">"Codes from an app such as Google Authenticator, Aegis or 1Password."</p>
                <div class="card__actions">
                    <button
                        class="button"
                        disabled=move || busy.get()
                        on:click=move |_| run(Action::Start)
                    >
                        "Set up an authenticator app"
                    </button>
                </div>
            }
                .into_any();
        }
        view! {
            <p class="card__text">"Set up. Signing in can take a code from it."</p>
            <div class="card__actions">
                <Show
                    when=move || confirm_removal.get()
                    fallback=move || {
                        view! {
                            <button class="button button--danger" on:click=move |_| confirm_removal.set(true)>
                                "Remove"
                            </button>
                        }
                    }
                >
                    <span class="card__confirm">
                        "Remove the authenticator app?"
                        <button class="button" on:click=move |_| confirm_removal.set(false)>
                            "Keep it"
                        </button>
                        <button
                            class="button button--danger"
                            disabled=move || busy.get()
                            on:click=move |_| {
                                confirm_removal.set(false);
                                run(Action::Remove);
                            }
                        >
                            "Remove"
                        </button>
                    </span>
                </Show>
            </div>
        }
            .into_any()
    }
}

/// What the card asks the server for.
#[derive(Clone)]
enum Action {
    Start,
    Confirm(String),
    NewCodes,
    Remove,
}

impl Action {
    /// Runs it, asking the user to confirm it is them when the server wants that. The step
    /// to show next, or `None` when they cancelled.
    async fn run(self, reconfirm: Reconfirm) -> Result<Option<Step>, ApiError> {
        let client = session::client();
        // Confirming an app on an account that has a key already brings no new codes.
        let codes = |RecoveryCodes { codes }| {
            if codes.is_empty() {
                Step::Overview
            } else {
                Step::Codes(codes)
            }
        };
        Ok(match self {
            Self::Start => reconfirm
                .confirmed(|| client.start_totp())
                .await?
                .map(Step::Adding),
            Self::Confirm(code) => reconfirm
                .confirmed(|| client.confirm_totp(&code))
                .await?
                .map(codes),
            Self::NewCodes => reconfirm
                .confirmed(|| client.new_recovery_codes())
                .await?
                .map(codes),
            Self::Remove => reconfirm
                .confirmed(|| client.remove_totp())
                .await?
                .map(|()| Step::Overview),
        })
    }
}

/// The new app's QR code and secret, and the field for its first code.
#[component]
fn AddApp(
    enrolment: TotpEnrolment,
    busy: RwSignal<bool>,
    run: impl Fn(Action) + Copy + 'static,
    cancel: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let code = RwSignal::new(String::new());
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        run(Action::Confirm(code.get_untracked()));
        code.set(String::new());
    };
    view! {
        <ol class="steps">
            <li>"Scan this code with your authenticator app."</li>
        </ol>
        <div class="qr" inner_html=qr_svg(&enrolment.uri)></div>
        <p class="card__text">
            "Can't scan it? Enter this key instead: "
            <code class="secret-key">{grouped(&enrolment.secret)}</code>
        </p>
        <form class="inline-form" on:submit=submit>
            <label class="field">
                <span class="field__label">"Code from the app"</span>
                <input
                    class="field__input field__input--code"
                    type="text"
                    inputmode="numeric"
                    autocomplete="one-time-code"
                    maxlength="7"
                    required
                    prop:value=move || code.get()
                    on:input:target=move |event| code.set(event.target().value())
                />
            </label>
            <div class="card__actions">
                <button type="button" class="button" on:click=move |_| cancel()>
                    "Cancel"
                </button>
                <button type="submit" class="button button--primary" disabled=move || busy.get()>
                    "Turn on"
                </button>
            </div>
        </form>
    }
}

/// Recovery codes, shown once, with ways to keep them.
#[component]
fn ShowCodes(codes: Vec<String>, done: impl Fn() + Copy + 'static) -> impl IntoView {
    let text = codes.join("\n");
    let copied = RwSignal::new(false);
    let copy_text = text.clone();
    let copy = move |_| {
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(&copy_text);
            copied.set(true);
        }
    };
    let download = format!(
        "data:text/plain;charset=utf-8,{}",
        js_sys::encode_uri_component(&format!("Home Space recovery codes\n\n{text}\n"))
    );
    view! {
        <p class="alert alert--success" role="status">
            "Save these recovery codes somewhere safe. Each one signs you in once if you lose your passkeys, security keys and authenticator app. They are shown only now."
        </p>
        <ul class="recovery-codes">
            {codes.into_iter().map(|code| view! { <li><code>{code}</code></li> }).collect_view()}
        </ul>
        <div class="card__actions">
            <button class="button" on:click=copy>
                {move || if copied.get() { "Copied" } else { "Copy" }}
            </button>
            <a class="button" href=download download="home-space-recovery-codes.txt">
                "Download"
            </a>
            <button class="button button--primary" on:click=move |_| done()>
                "I saved them"
            </button>
        </div>
    }
}

/// How many recovery codes are left, as a sentence.
pub fn recovery_left(left: u32) -> String {
    match left {
        0 => "No recovery codes left. Make new ones so you can still sign in if you lose your devices."
            .to_owned(),
        1 => "1 recovery code left.".to_owned(),
        _ => format!("{left} recovery codes left."),
    }
}

/// The key in groups of four, as apps show them, so it is easier to type.
fn grouped(secret: &str) -> String {
    secret
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The QR code of `uri` as SVG markup for the page, without the XML declaration a file
/// would start with.
fn qr_svg(uri: &str) -> String {
    let Ok(code) = QrCode::new(uri.as_bytes()) else {
        return String::new();
    };
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(192, 192)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    match svg.find("<svg") {
        Some(start) => svg[start..].to_owned(),
        None => svg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_user_is_told_where_they_sign_in() {
        assert_eq!(
            managed_by(Some("Keycloak")),
            "Signed in through Keycloak. Your sign-in, two-factor and passkeys are managed there."
        );
        assert!(managed_by(None).starts_with("Signed in through your identity provider."));
    }

    #[test]
    fn the_key_reads_in_groups_of_four() {
        assert_eq!(grouped("GEZDGNBVGY3TQOJQ"), "GEZD GNBV GY3T QOJQ");
        assert_eq!(grouped("ABCDEF"), "ABCD EF");
    }

    #[test]
    fn the_codes_left_read_as_a_sentence() {
        assert_eq!(recovery_left(1), "1 recovery code left.");
        assert_eq!(recovery_left(9), "9 recovery codes left.");
        assert!(recovery_left(0).starts_with("No recovery codes left"));
    }

    #[test]
    fn a_qr_code_is_svg() {
        let svg = qr_svg("otpauth://totp/Home%20Space:alice?secret=GEZD");
        assert!(svg.starts_with("<svg"), "{svg}");
        assert!(svg.ends_with("</svg>"));
    }
}
