use client_core::selection::shows_bulk_actions;
use leptos::prelude::*;
use web_sys::{File, HtmlInputElement};

use super::file_drop::files_of;
use crate::{
    components::{Menu, MenuItem},
    icon::Icon,
    pages::search_box::SearchBox,
};

#[component]
pub fn Toolbar(
    selected_count: Signal<usize>,
    on_move: Callback<()>,
    on_send: Callback<()>,
    on_send_local: Callback<()>,
    on_delete: Callback<()>,
    can_delete: Signal<bool>,
    can_add: Signal<bool>,
    on_new_folder: Callback<()>,
    on_upload: Callback<Vec<File>>,
) -> impl IntoView {
    view! {
        <div class="explorer__toolbar">
            <SearchBox />
            <Show when=move || shows_bulk_actions(selected_count.get())>
                <Menu
                    id="bulk-menu".to_owned()
                    label="Actions for selected items"
                    text=Signal::derive(move || format!("{} selected", selected_count.get()))
                    icon="more_horiz"
                >
                    <MenuItem icon="send" label="Send to device…" on_select=on_send />
                    <MenuItem icon="drive_file_move" label="Move" on_select=on_move />
                    <Show when=move || can_delete.get()>
                        <MenuItem icon="delete" label="Delete" danger=true on_select=on_delete />
                    </Show>
                </Menu>
            </Show>
            <label
                class="button button--primary"
                aria-label="Upload files"
                class:button--disabled=move || !can_add.get()
            >
                <Icon name="upload" />
                <span class="label">"Upload"</span>
                <input
                    type="file"
                    multiple
                    hidden
                    disabled=move || !can_add.get()
                    on:change:target=move |event| {
                        let input: HtmlInputElement = event.target();
                        if let Some(list) = input.files() {
                            on_upload.run(files_of(&list));
                        }
                        input.set_value("");
                    }
                />
            </label>
            <button
                class="button"
                aria-label="New folder"
                disabled=move || !can_add.get()
                on:click=move |_| on_new_folder.run(())
            >
                <Icon name="create_new_folder" />
                <span class="label">"New folder"</span>
            </button>
            <button
                class="button"
                aria-label="Send files from this computer to a device"
                title="Send files from this computer to a device"
                on:click=move |_| on_send_local.run(())
            >
                <Icon name="send" />
                <span class="label">"Send…"</span>
            </button>
        </div>
    }
}
