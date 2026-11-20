use leptos::{
    html::{Button, Div},
    prelude::*,
};
use web_sys::HtmlElement;

use crate::icon::Icon;

const ICON_MORE: &str = "more_horiz";
const ICON_EXPAND: &str = "expand_more";
const TRIGGER_CLASS: &str = "button button--ghost button--small menu__trigger";
const COMBO_CLASS: &str = "combo";
const POPOVER_AUTO: &str = "auto";
const POPOVER_OPEN: &str = ":popover-open";
/// Space between the trigger and the panel, and the panel and the window edge.
const MENU_GAP: f64 = 4.0;

#[derive(Clone, Copy)]
struct MenuPanel(NodeRef<Div>);

/// Hides the popover `panel`, if it is mounted.
pub fn close_popover(panel: NodeRef<Div>) {
    if let Some(element) = panel.get_untracked() {
        let element: &HtmlElement = &element;
        let _ = element.hide_popover();
    }
}

/// A button that opens a popover list of actions anchored under it, right-aligned with it
/// unless `align_start`.
#[component]
pub fn Menu(
    id: String,
    #[prop(into)] label: String,
    #[prop(optional, into)] text: Option<Signal<String>>,
    #[prop(default = ICON_MORE)] icon: &'static str,
    /// Lines the panel up with the trigger's left edge instead of its right.
    #[prop(optional)]
    align_start: bool,
    /// Draws the trigger as a full-width field with a chevron, and opens the panel at least as
    /// wide as it.
    #[prop(optional)]
    combo: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let panel = NodeRef::<Div>::new();
    let trigger = NodeRef::<Button>::new();
    provide_context(MenuPanel(panel));
    let layout = Layout { align_start, combo };
    let open = move |_| {
        layout.place(trigger, panel, false);
        request_animation_frame(move || follow(trigger, panel, layout));
    };

    view! {
        <button
            class=if combo { COMBO_CLASS } else { TRIGGER_CLASS }
            aria-label=label
            aria-haspopup="menu"
            popovertarget=id.clone()
            node_ref=trigger
            on:click=open
        >
            {move || (!icon.is_empty()).then(|| view! { <Icon name=icon /> })}
            {move || {
                text.map(|text| {
                    if combo {
                        view! { <span class="combo__text">{text.get()}</span> }.into_any()
                    } else {
                        text.get().into_any()
                    }
                })
            }}
            {combo.then(|| view! { <Icon name=ICON_EXPAND /> })}
        </button>
        <div id=id class="menu" role="menu" popover=POPOVER_AUTO node_ref=panel>
            {children()}
        </div>
    }
}

/// How a panel lines up with its trigger.
#[derive(Clone, Copy)]
struct Layout {
    align_start: bool,
    combo: bool,
}

impl Layout {
    /// Puts the panel under the trigger, or above it when there is no room below; `shown` says
    /// whether the panel is already open, so its own size can be read.
    fn place(self, trigger: NodeRef<Button>, panel: NodeRef<Div>, shown: bool) {
        let (Some(trigger), Some(element)) = (trigger.get_untracked(), panel.get_untracked())
        else {
            return;
        };
        let rect = trigger.get_bounding_client_rect();
        let (width, height) = web_sys::window()
            .and_then(|window| {
                let width = window.inner_width().ok()?.as_f64()?;
                let height = window.inner_height().ok()?.as_f64()?;
                Some((width, height))
            })
            .unwrap_or_default();
        let style = HtmlElement::style(&element);
        if self.combo {
            let _ = style.set_property("min-width", &format!("{}px", rect.width()));
        }
        let (panel_width, panel_height) = if shown {
            (
                f64::from(element.offset_width()),
                f64::from(element.offset_height()),
            )
        } else {
            measure(&element)
        };
        let y = top(rect.top(), rect.bottom(), panel_height, height);
        let x = if self.align_start {
            left(rect.left() + panel_width, panel_width, width)
        } else {
            left(rect.right(), panel_width, width)
        };
        let _ = style.set_property("top", &format!("{y}px"));
        let _ = style.set_property("left", &format!("{x}px"));
        let _ = style.set_property("right", "auto");
    }
}

/// Keeps an open panel on its trigger every frame, as what is around the trigger can move it:
/// a dialog that grows as people are picked, say. Stops once the panel closes.
fn follow(trigger: NodeRef<Button>, panel: NodeRef<Div>, layout: Layout) {
    let shown = panel
        .get_untracked()
        .is_some_and(|element| element.matches(POPOVER_OPEN).unwrap_or(false));
    if shown {
        layout.place(trigger, panel, true);
        request_animation_frame(move || follow(trigger, panel, layout));
    }
}

/// The panel's size, measured while it is still hidden (before the popover opens).
fn measure(element: &HtmlElement) -> (f64, f64) {
    let style = element.style();
    let _ = style.set_property("display", "block");
    let _ = style.set_property("visibility", "hidden");
    let size = (
        f64::from(element.offset_width()),
        f64::from(element.offset_height()),
    );
    let _ = style.remove_property("display");
    let _ = style.remove_property("visibility");
    size
}

/// Where the panel's left goes: right-aligned with the trigger, kept inside the viewport.
fn left(trigger_right: f64, panel: f64, viewport: f64) -> f64 {
    (trigger_right - panel)
        .min(viewport - panel - MENU_GAP)
        .max(MENU_GAP)
}

/// Where the panel's top goes: under the trigger, or above it when it would run off the bottom.
fn top(trigger_top: f64, trigger_bottom: f64, panel: f64, viewport: f64) -> f64 {
    let below = trigger_bottom + MENU_GAP;
    if below + panel + MENU_GAP <= viewport {
        below
    } else {
        (trigger_top - MENU_GAP - panel).max(MENU_GAP)
    }
}

/// One entry of a [`Menu`]; closes the menu after running `on_select`.
#[component]
pub fn MenuItem(
    icon: &'static str,
    #[prop(into)] label: String,
    #[prop(optional)] danger: bool,
    on_select: Callback<()>,
) -> impl IntoView {
    let panel = expect_context::<MenuPanel>().0;
    view! {
        <button
            class="menu__item"
            class:menu__item--danger=danger
            role="menuitem"
            on:click=move |_| {
                close_popover(panel);
                on_select.run(());
            }
        >
            <Icon name=icon />
            {label}
        </button>
    }
}

/// A menu entry that navigates; closes the menu on click.
#[component]
pub fn MenuLink(href: String, children: Children) -> impl IntoView {
    let panel = expect_context::<MenuPanel>().0;
    view! {
        <leptos_router::components::A
            href=href
            exact=true
            attr:class="menu__item"
            attr:role="menuitem"
            on:click=move |_| close_popover(panel)
        >
            {children()}
        </leptos_router::components::A>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_opens_under_the_trigger_when_it_fits() {
        assert_eq!(top(100.0, 120.0, 200.0, 800.0), 124.0);
    }

    #[test]
    fn menu_opens_above_the_trigger_near_the_bottom() {
        assert_eq!(top(700.0, 720.0, 200.0, 800.0), 496.0);
    }

    #[test]
    fn menu_stays_inside_a_short_window() {
        assert_eq!(top(100.0, 120.0, 400.0, 300.0), MENU_GAP);
    }

    #[test]
    fn menu_right_aligns_with_the_trigger_when_it_fits() {
        assert_eq!(left(300.0, 180.0, 400.0), 120.0);
    }

    #[test]
    fn menu_does_not_run_off_the_left_edge() {
        assert_eq!(left(100.0, 180.0, 400.0), MENU_GAP);
    }
}
