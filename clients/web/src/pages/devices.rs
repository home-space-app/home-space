use leptos::{html::Input, prelude::*, task::spawn_local};
use uuid::Uuid;
use web_sys::KeyboardEvent;

use client_core::labels::{THIS_DEVICE, device_state, platform, sign_out_question};
use protocol::{Device, Event};

use super::settings::SettingsHeader;
use crate::{
    components::ErrorAlert,
    events,
    icon::Icon,
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
                                "Last seen"
                            </th>
                            <th class="node-table__header-cell node-table__header-cell--modified">
                                "State"
                            </th>
                            <th class="node-table__header-cell node-table__cell--actions"></th>
                        </tr>
                    </thead>
                    <tbody class="node-table__body">
                        <For
                            each=move || devices.get()
                            key=|device| (device.id, device.name.clone(), device.signed_in, device.uploads)
                            children=move |device| {
                                view! {
                                    <DeviceRow device renaming signing_out save cancel />
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
    let meta = format!("{kind} · {last_seen} · {state}");
    // The current device signs out with the app bar's button; one that is signed out and
    // left nothing behind has nothing to end.
    let can_sign_out = !device.current && (device.signed_in || device.uploads > 0);
    let name = device.name.clone();
    let start_rename = move |_| {
        signing_out.set(None);
        renaming.set(Some(Renaming {
            id,
            name: name.clone(),
        }));
    };
    let ask_sign_out = {
        let device = device.clone();
        move |_| {
            renaming.set(None);
            signing_out.set(Some(device.clone()));
        }
    };

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
            <td class="node-table__cell node-table__cell--modified">{last_seen.clone()}</td>
            <td class="node-table__cell node-table__cell--modified">{state.clone()}</td>
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
                            <button class="button button--small button--ghost" on:click=start_rename.clone()>
                                "Rename"
                            </button>
                            <button
                                class="button button--small button--ghost button--danger"
                                disabled=!can_sign_out
                                on:click=ask_sign_out.clone()
                            >
                                <Icon name="logout" />
                                <span class="label">{SIGN_OUT}</span>
                            </button>
                        }
                            .into_any()
                    }}
                </span>
            </td>
        </tr>
    }
}
