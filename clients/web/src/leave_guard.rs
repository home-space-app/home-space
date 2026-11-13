//! Holds navigation back while a page has unsaved edits, so the page can ask first.
//!
//! In-app links are caught before the router follows them, other navigation in the app goes
//! through [`LeaveGuard::allows`], and closing or reloading the tab gets the browser's own
//! question.

use leptos::{ev, prelude::*};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{Element, MouseEvent};

const CLICK: &str = "click";
const ANCHOR: &str = "a[href]";
const HREF: &str = "href";
const TARGET: &str = "target";
const DOWNLOAD: &str = "download";
const SELF_TARGET: &str = "_self";
const MAIN_BUTTON: i16 = 0;

/// Whether a page has unsaved edits, and where the user went while it had them.
#[derive(Clone, Copy)]
pub struct LeaveGuard {
    unsaved: RwSignal<bool>,
    pending: RwSignal<Option<String>>,
}

impl LeaveGuard {
    /// Whether the app may go to `path` now; when unsaved edits hold it back, `path` waits for
    /// the page to ask.
    pub fn allows(self, path: &str) -> bool {
        if self.unsaved.get_untracked() {
            self.pending.set(Some(path.to_owned()));
            false
        } else {
            true
        }
    }

    /// Marks whether the page shown has unsaved edits.
    pub fn set_unsaved(self, unsaved: bool) {
        self.unsaved.set(unsaved);
        if !unsaved {
            self.pending.set(None);
        }
    }

    /// Where the user went while there were unsaved edits.
    pub fn pending(self) -> ReadSignal<Option<String>> {
        self.pending.read_only()
    }

    /// Stays on the page.
    pub fn stay(self) {
        self.pending.set(None);
    }

    /// Drops the edits and takes the path that was held back.
    pub fn take(self) -> Option<String> {
        self.unsaved.set(false);
        self.pending.try_update(Option::take).flatten()
    }
}

/// Starts the guard for the whole app.
pub fn provide() {
    let guard = LeaveGuard {
        unsaved: RwSignal::new(false),
        pending: RwSignal::new(None),
    };
    provide_context(guard);
    catch_links(guard);
    // The browser shows its own question; the page cannot word it.
    let unload = window_event_listener(ev::beforeunload, move |event| {
        if guard.unsaved.get_untracked() {
            event.prevent_default();
        }
    });
    on_cleanup(move || unload.remove());
}

/// The guard of the app.
pub fn use_leave_guard() -> LeaveGuard {
    expect_context()
}

/// Catches clicks on in-app links before the router, which follows them in the bubbling phase.
fn catch_links(guard: LeaveGuard) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let on_click = Closure::<dyn Fn(MouseEvent)>::new(move |event: MouseEvent| {
        if !guard.unsaved.get_untracked() || !plain_click(&event) {
            return;
        }
        if let Some(path) = in_app_path(&event) {
            event.prevent_default();
            guard.pending.set(Some(path));
        }
    });
    let added = window.add_event_listener_with_callback_and_bool(
        CLICK,
        on_click.as_ref().unchecked_ref(),
        true,
    );
    if added.is_ok() {
        // The app lives as long as the tab, and so does the listener.
        on_click.forget();
    }
}

/// A click that follows a link in the same tab.
fn plain_click(event: &MouseEvent) -> bool {
    event.button() == MAIN_BUTTON
        && !event.default_prevented()
        && !(event.ctrl_key() || event.meta_key() || event.shift_key() || event.alt_key())
}

/// The path of the in-app link the click is on.
fn in_app_path(event: &MouseEvent) -> Option<String> {
    let anchor = event
        .target()?
        .dyn_into::<Element>()
        .ok()?
        .closest(ANCHOR)
        .ok()??;
    let opens_here = anchor
        .get_attribute(TARGET)
        .is_none_or(|target| target.is_empty() || target == SELF_TARGET);
    let path = anchor.get_attribute(HREF)?;
    (opens_here && !anchor.has_attribute(DOWNLOAD) && is_in_app(&path)).then_some(path)
}

/// Whether `href` is a path of this app, rather than another site.
fn is_in_app(href: &str) -> bool {
    href.starts_with('/') && !href.starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_of_the_app_are_in_app() {
        assert!(is_in_app("/folder/abc"));
    }

    #[test]
    fn other_sites_are_not_in_app() {
        assert!(!is_in_app("https://example.com/"));
        assert!(!is_in_app("//example.com/"));
    }
}
