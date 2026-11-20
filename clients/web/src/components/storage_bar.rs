//! A bar split into what takes up storage, with a legend naming each part and its size.

use leptos::prelude::*;

use client_core::storage::{bytes, fraction};

/// One part of the bar: its label, its size and the class that colours it.
type StoragePart = (&'static str, u64, &'static str);

/// `parts` as segments of a bar out of `total`, then a legend. `large` draws the taller bar;
/// `rows` stacks the legend one part per line.
#[component]
pub fn StorageBar(
    parts: Vec<StoragePart>,
    total: u64,
    #[prop(optional)] large: bool,
    #[prop(optional)] rows: bool,
) -> impl IntoView {
    let segments = parts
        .iter()
        .map(|(_, size, class)| {
            view! {
                <span
                    class=format!("segment {class}")
                    style=format!("width: {:.2}%", fraction(*size, total) * 100.0)
                ></span>
            }
        })
        .collect_view();
    let legend = parts
        .iter()
        .map(|(label, size, class)| {
            view! {
                <li class="legend__item">
                    <span class=format!("legend__dot {class}")></span>
                    <span class="legend__label">{*label}</span>
                    <span class="legend__value">{bytes(*size)}</span>
                </li>
            }
        })
        .collect_view();
    view! {
        <div class=if large { "segments segments--large" } else { "segments" } aria-hidden="true">
            {segments}
        </div>
        <ul class=if rows { "legend legend--rows" } else { "legend" }>{legend}</ul>
    }
}
