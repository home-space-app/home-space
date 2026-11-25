//! Two-factor sign-in in the user's settings: adding an authenticator app, its recovery codes,
//! and removing it.

use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use qrcode::{QrCode, render::svg};

use api_client::ApiError;
use protocol::{RecoveryCodes, TotpEnrolment, features};

use crate::{
    components::{ErrorAlert, reconfirm::Reconfirm},
    session::{self, Auth},
};

/// Few enough recovery codes left to suggest making new ones.
pub const FEW_RECOVERY_CODES: u32 = 3;

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

/// Whether the server offers two-factor sign-in and the user signs in with a password.
pub fn offered(auth: Auth) -> bool {
    session::client().supports(features::TWO_FACTOR)
        && auth.user.get().is_some_and(|me| !me.external)
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

    view! {
        <section class="card settings-grid__wide">
            <h2 class="card__title">"Two-factor sign-in"</h2>
            <ErrorAlert message=error />
            {move || match step.get() {
                Step::Overview if factors().totp => {
                    let left = factors().recovery_codes_left;
                    view! {
                        <p class="card__text">
                            "On. Signing in takes a code from your authenticator app as well as your password."
                        </p>
                        <p class="card__text" class:card__text--warning=left <= FEW_RECOVERY_CODES>
                            {recovery_left(left)}
                        </p>
                        <div class="card__actions">
                            <button
                                class="button"
                                disabled=move || busy.get()
                                on:click=move |_| run(Action::NewCodes)
                            >
                                "New recovery codes"
                            </button>
                            <Show
                                when=move || confirm_removal.get()
                                fallback=move || {
                                    view! {
                                        <button
                                            class="button button--danger"
                                            on:click=move |_| confirm_removal.set(true)
                                        >
                                            "Turn off"
                                        </button>
                                    }
                                }
                            >
                                <span class="card__confirm">
                                    "Turn off two-factor sign-in?"
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
                                        "Turn off"
                                    </button>
                                </span>
                            </Show>
                        </div>
                    }
                        .into_any()
                }
                Step::Overview => {
                    view! {
                        <p class="card__text">
                            "Off. Add an authenticator app, such as Google Authenticator, Aegis or 1Password, so that a stolen password alone cannot open your account."
                        </p>
                        <button
                            class="button button--primary card__action"
                            disabled=move || busy.get()
                            on:click=move |_| run(Action::Start)
                        >
                            "Set up an authenticator app"
                        </button>
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
        let codes = |RecoveryCodes { codes }| Step::Codes(codes);
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
            "Save these recovery codes somewhere safe. Each one signs you in once if you lose your authenticator app. They are shown only now."
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
        0 => "No recovery codes left. Make new ones so you can still sign in without the app."
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
