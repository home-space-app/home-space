use leptos::{html::Input, prelude::*, task::spawn_local};
use uuid::Uuid;
use web_sys::KeyboardEvent;

use client_core::{
    labels::{THIS_DEVICE, device_state, platform, sign_out_question},
    transfers::visible_to,
};
use protocol::{Device, DeviceVisibility, Event};

use super::{settings::SettingsHeader, visibility_dialog::VisibilityDialog};
use crate::{
    components::{ErrorAlert, Menu, MenuItem},
    events,
    icon::{Icon, platform_icon, visibility_icon},
    load,
    locale::short_date,
    session::{self, Auth},
};

const KEY_ENTER: &str = "Enter";
const KEY_ESCAPE: &str = "Escape";
const SIGN_OUT: &str = "Sign out";

/// A device being renamed and the name typed so far.
#[derive(Clone, PartialEq, Eq)]
struct Renaming {
    id: Uuid,
    name: String,
}

#[component]
pub fn DevicesPage(auth: Auth) -> impl IntoView {
    let devices = RwSignal::new(Vec::<Device>::new());
    let error = RwSignal::new(None::<String>);
    let renaming = RwSignal::new(None::<Renaming>);
    let signing_out = RwSignal::new(None::<Device>);
    let choosing = RwSignal::new(None::<Device>);

    let list = load::load(
        || async { session::client().devices().await },
        move |result| match result {
            Ok(listing) => {
                devices.set(listing);
                error.set(None);
            }
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );

    events::use_events(move |event| {
        if matches!(event, Event::DevicesChanged | Event::Resync) {
            list.refetch();
        }
    });

    let save = Callback::new(move |()| {
        let Some(Renaming { id, name }) = renaming.get_untracked() else {
            return;
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            return;
        }
        renaming.set(None);
        spawn_local(async move {
            if let Err(err) = session::client().rename_device(id, &name).await {
                error.set(Some(auth.fail(&err)));
            }
            list.refetch();
        });
    });
    let cancel = Callback::new(move |()| renaming.set(None));
    let confirm_sign_out = move |_| {
        let Some(device) = signing_out.get_untracked() else {
            return;
        };
        signing_out.set(None);
        spawn_local(async move {
            if let Err(err) = session::client().sign_out_device(device.id).await {
                error.set(Some(auth.fail(&err)));
            }
            list.refetch();
        });
    };
    let close_visibility = Callback::new(move |saved: bool| {
        choosing.set(None);
        if saved {
            list.refetch();
        }
    });

    view! {
        <main class="explorer explorer--tabbed">
            <div class="explorer__header explorer__header--stacked">
                <SettingsHeader />
            </div>
            <ErrorAlert message=error />
            <div class="explorer__scroll">
                <table class="node-table">
                    <thead class="node-table__head">
                        <tr>
                            <th class="node-table__header-cell">"Name"</th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                "State"
                            </th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                "Visible to"
                            </th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                "Last seen"
                            </th>
                            <th class="node-table__header-cell node-table__cell--actions"></th>
                        </tr>
                    </thead>
                    <tbody class="node-table__body">
                        <For
                            each=move || devices.get()
                            key=|device| {
                                (
                                    device.id,
                                    device.name.clone(),
                                    device.signed_in,
                                    device.online,
                                    device.uploads,
                                    device.visibility,
                                    device.visible_to.clone(),
                                    device.last_seen_at,
                                )
                            }
                            children=move |device| {
                                view! {
                                    <DeviceRow device renaming signing_out choosing save cancel />
                                }
                            }
                        />
                    </tbody>
                </table>
                <Show when=move || devices.read().is_empty() && error.read().is_none()>
                    <p class="explorer__empty">"No devices have signed in yet."</p>
                </Show>
            </div>
        </main>
        {move || {
            choosing
                .get()
                .map(|device| view! { <VisibilityDialog device auth on_close=close_visibility /> })
        }}
        {move || {
            signing_out
                .get()
                .map(|device| {
                    view! {
                        <div class="modal" on:click=move |_| signing_out.set(None)>
                            <div
                                class="modal__panel"
                                role="alertdialog"
                                aria-modal="true"
                                on:click=|event| event.stop_propagation()
                            >
                                <header class="modal__header">
                                    <h2 class="modal__title">{sign_out_question(&device)}</h2>
                                    <p class="modal__subtitle">
                                        "It has to sign in again to sync."
                                    </p>
                                </header>
                                <div class="modal__actions">
                                    <button class="button" on:click=move |_| signing_out.set(None)>
                                        "Cancel"
                                    </button>
                                    <button
                                        class="button button--danger"
                                        on:click=confirm_sign_out
                                    >
                                        {SIGN_OUT}
                                    </button>
                                </div>
                            </div>
                        </div>
                    }
                })
        }}
    }
}

#[component]
fn DeviceRow(
    device: Device,
    renaming: RwSignal<Option<Renaming>>,
    signing_out: RwSignal<Option<Device>>,
    choosing: RwSignal<Option<Device>>,
    save: Callback<()>,
    cancel: Callback<()>,
) -> impl IntoView {
    let id = device.id;
    let editing = Memo::new(move |_| renaming.read().as_ref().is_some_and(|r| r.id == id));
    let input = NodeRef::<Input>::new();
    Effect::new(move |_| {
        if editing.get()
            && let Some(input) = input.get()
        {
            let _ = input.focus();
            input.select();
        }
    });
    let kind = if device.current {
        format!("{} · {THIS_DEVICE}", platform(&device.platform))
    } else {
        platform(&device.platform).to_owned()
    };
    let last_seen = short_date(device.last_seen_at);
    let state = device_state(&device);
    let visibility = visible_to(&device);
    let meta = format!("{kind} · {state} · {visibility}");
    // The current device signs out with the account menu; one that is signed out and left
    // nothing behind has nothing to end.
    let can_sign_out = !device.current && (device.signed_in || device.uploads > 0);
    let icon = platform_icon(&device.platform);
    let state_class = if device.online || device.signed_in {
        "pill pill--success"
    } else {
        "pill"
    };
    let start_rename = {
        let name = device.name.clone();
        Callback::new(move |()| {
            signing_out.set(None);
            renaming.set(Some(Renaming {
                id,
                name: name.clone(),
            }));
        })
    };
    let ask_sign_out = {
        let device = device.clone();
        Callback::new(move |()| {
            renaming.set(None);
            signing_out.set(Some(device.clone()));
        })
    };
    let choose_visibility = {
        let device = device.clone();
        Callback::new(move |()| {
            renaming.set(None);
            choosing.set(Some(device.clone()));
        })
    };
    let visibility_icon = visibility_icon(device.visibility);
    let hidden = device.visibility == DeviceVisibility::Hidden;

    view! {
        <tr class="node-table__row">
            <td class="node-table__cell">
                {move || if editing.get() {
                    view! {
                        <span class="node-table__name node-table__name--editing">
                            <input
                                node_ref=input
                                class="field__input node-table__input"
                                aria-label="Device name"
                                prop:value=move || {
                                    renaming.read().as_ref().map(|r| r.name.clone()).unwrap_or_default()
                                }
                                on:input:target=move |event| {
                                    let value = event.target().value();
                                    renaming.update(|r| {
                                        if let Some(r) = r {
                                            r.name = value;
                                        }
                                    });
                                }
                                on:keydown=move |event: KeyboardEvent| {
                                    match event.key().as_str() {
                                        KEY_ENTER => save.run(()),
                                        KEY_ESCAPE => cancel.run(()),
                                        _ => {}
                                    }
                                }
                            />
                        </span>
                    }
                        .into_any()
                } else {
                    view! {
                        <span class="node-table__name">
                            <Icon name=icon />
                            <span class="node-table__text">
                                {device.name.clone()}
                                <span class="device-kind">{kind.clone()}</span>
                                <span class="node-table__meta">{meta.clone()}</span>
                            </span>
                        </span>
                    }
                        .into_any()
                }}
            </td>
            <td class="node-table__cell node-table__cell--modified">
                <span class=state_class>{state.clone()}</span>
            </td>
            <td class="node-table__cell node-table__cell--modified">
                <span class="device-visibility" class:device-visibility--hidden=hidden>
                    <Icon name=visibility_icon />
                    {visibility.clone()}
                </span>
            </td>
            <td class="node-table__cell node-table__cell--modified">{last_seen.clone()}</td>
            <td class="node-table__cell node-table__cell--actions">
                <span class="node-table__actions">
                    {move || if editing.get() {
                        view! {
                            <button
                                class="button button--small button--primary"
                                on:click=move |_| save.run(())
                            >
                                "Save"
                            </button>
                            <button class="button button--small" on:click=move |_| cancel.run(())>
                                "Cancel"
                            </button>
                        }
                            .into_any()
                    } else {
                        view! {
                            <Menu id=format!("device-menu-{id}") label="Device actions">
                                <MenuItem icon="edit" label="Rename" on_select=start_rename />
                                <MenuItem
                                    icon="visibility"
                                    label="Visibility…"
                                    on_select=choose_visibility
                                />
                                {can_sign_out
                                    .then(|| {
                                        view! {
                                            <MenuItem
                                                icon="logout"
                                                label=SIGN_OUT
                                                danger=true
                                                on_select=ask_sign_out
                                            />
                                        }
                                    })}
                            </Menu>
                        }
                            .into_any()
                    }}
                </span>
            </td>
        </tr>
    }
}
