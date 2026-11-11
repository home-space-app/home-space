//! Administration › Users: the accounts on the server, and adding, editing, disabling and
//! deleting them. Deleting marks an account; the server removes it and its files after a delay
//! during which the deletion can be cancelled.

use std::time::Duration;

use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};

use api_client::ApiError;
use client_core::{
    accounts::{
        AccountFilter, NOT_A_QUOTA, QuotaUnit, deletion_countdown, matches_search, near_quota,
        parse_quota, password_problem, quota_input, sign_in_method, storage_fill, storage_label,
    },
    labels::platform,
    storage::{bytes, usage_breakdown},
};
use protocol::{Account, AccountDevice, CreateAccount, ServerStatus, UpdateAccount, UserId};

use super::sidebar::initial;
use crate::{
    components::{ErrorAlert, Menu, MenuItem},
    icon::{Icon, platform_icon},
    load,
    locale::{medium_date, now, short_date, short_time},
    session::{self, Auth},
};

/// How often the countdowns of accounts marked for deletion are updated, and the list
/// reloaded so that deleted accounts disappear.
const TICK: Duration = Duration::from_secs(30);
const FILTERS: [AccountFilter; 3] = [
    AccountFilter::All,
    AccountFilter::Admins,
    AccountFilter::Disabled,
];
const FOOTNOTE: &str = "Usage counts current files, previous versions and the recycle bin. \
                        Admins manage accounts here but cannot open other users' files.";

/// The state pill of an account: its class and label.
fn state_pill(account: &Account) -> (&'static str, &'static str) {
    if account.delete_at.is_some() {
        ("pill pill--danger", "Deleting")
    } else if account.disabled {
        ("pill", "Disabled")
    } else {
        ("pill pill--success", "Active")
    }
}

/// When a marked account goes, e.g. `Deleted at 14:32 · in 9 minutes`.
fn deletion_note(account: &Account, now: time::OffsetDateTime) -> Option<String> {
    account.delete_at.map(|at| {
        format!(
            "Deleted at {} · {}",
            short_time(at),
            deletion_countdown(at, now)
        )
    })
}

fn delay_label(minutes: u64) -> String {
    match minutes {
        0 => "in a moment".to_owned(),
        1 => "in 1 minute".to_owned(),
        minutes => format!("in {minutes} minutes"),
    }
}

#[component]
pub fn UsersTab(auth: Auth, status: RwSignal<Option<ServerStatus>>) -> impl IntoView {
    let accounts = RwSignal::new(Vec::<Account>::new());
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let filter = RwSignal::new(AccountFilter::All);
    let query = RwSignal::new(String::new());
    let adding = RwSignal::new(false);
    let editing = RwSignal::new(None::<UserId>);
    let deleting = RwSignal::new(None::<Account>);
    let clock = RwSignal::new(now());

    let list = load::load(
        || async { session::client().accounts().await },
        move |result| match result {
            Ok(found) => {
                accounts.set(found);
                loaded.set(true);
                error.set(None);
            }
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );
    if let Ok(handle) = set_interval_with_handle(
        move || {
            clock.set(now());
            if accounts.with_untracked(|all| all.iter().any(|a| a.delete_at.is_some())) {
                list.refetch();
            }
        },
        TICK,
    ) {
        on_cleanup(move || handle.clear());
    }

    let me = auth.user.get_untracked().map(|me| me.id);
    let provider = Memo::new(move |_| {
        status.with(|status| {
            status
                .as_ref()
                .and_then(|status| status.config.oidc.as_ref())
                .map(|oidc| oidc.name.clone())
        })
    });
    let delay = Memo::new(move |_| {
        status.with(|status| {
            status
                .as_ref()
                .map_or(10, |status| status.config.user_deletion_delay_minutes)
        })
    });
    let disk_free = Memo::new(move |_| {
        status.with(|status| {
            status
                .as_ref()
                .and_then(|status| status.disk.space)
                .map(|space| space.available)
        })
    });

    let replace = Callback::new(move |updated: Account| {
        accounts.update(
            |all| match all.iter_mut().find(|account| account.id == updated.id) {
                Some(slot) => *slot = updated,
                None => {
                    all.push(updated);
                    all.sort_by(|a, b| a.username.cmp(&b.username));
                }
            },
        );
    });
    let update = Callback::new(move |(id, change): (UserId, UpdateAccount)| {
        spawn_local(async move {
            match session::client().update_account(id, &change).await {
                Ok(updated) => replace.run(updated),
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    });
    let cancel_deletion = Callback::new(move |id: UserId| {
        spawn_local(async move {
            match session::client().cancel_account_deletion(id).await {
                Ok(updated) => replace.run(updated),
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    });
    let confirm_delete = Callback::new(move |account: Account| {
        deleting.set(None);
        spawn_local(async move {
            match session::client().delete_account(account.id).await {
                Ok(updated) => replace.run(updated),
                Err(err) => error.set(Some(auth.fail(&err))),
            }
        });
    });
    let ask_delete = Callback::new(move |account: Account| deleting.set(Some(account)));
    let edit = Callback::new(move |id: UserId| editing.set(Some(id)));

    let visible = move || {
        let query = query.get();
        let filter = filter.get();
        accounts.with(|all| {
            all.iter()
                .filter(|account| filter.matches(account) && matches_search(account, &query))
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let largest = Memo::new(move |_| {
        accounts.with(|all| {
            all.iter()
                .map(|account| account.usage.total())
                .max()
                .unwrap_or_default()
        })
    });

    view! {
        <div class="admin-toolbar">
            <label class="search-field">
                <Icon name="search" />
                <input
                    class="search-field__input"
                    type="search"
                    placeholder="Search users"
                    aria-label="Search users"
                    prop:value=move || query.get()
                    on:input:target=move |event| query.set(event.target().value())
                />
            </label>
            <div class="filter-chips" role="group" aria-label="Show">
                {FILTERS
                    .into_iter()
                    .map(|choice| {
                        let count = move || {
                            accounts.with(|all| all.iter().filter(|a| choice.matches(a)).count())
                        };
                        view! {
                            <button
                                class="filter-chip"
                                class:filter-chip--active=move || filter.get() == choice
                                aria-pressed=move || (filter.get() == choice).to_string()
                                on:click=move |_| filter.set(choice)
                            >
                                {choice.label()}
                                <span class="filter-chip__count">{count}</span>
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
            <span class="page-header__spacer"></span>
            <button class="button button--primary" on:click=move |_| adding.set(true)>
                <Icon name="person_add" />
                <span class="label">"Add user"</span>
            </button>
        </div>
        <ErrorAlert message=error />
        <section class="card card--table">
            <div class="table-scroll">
                <table class="data-table accounts-table">
                    <thead>
                        <tr>
                            <th>"User"</th>
                            <th>"Role"</th>
                            <th class="data-table__storage">"Storage"</th>
                            <th class="data-table__number">"Devices"</th>
                            <th>"Last seen"</th>
                            <th>"State"</th>
                            <th class="accounts-table__actions">
                                <span class="visually-hidden">"Actions"</span>
                            </th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let rows = visible();
                            if rows.is_empty() && loaded.get() {
                                return view! {
                                    <tr>
                                        <td colspan="7" class="accounts-table__empty">
                                            "No users match."
                                        </td>
                                    </tr>
                                }
                                    .into_any();
                            }
                            rows.into_iter()
                                .map(|account| {
                                    view! {
                                        <AccountRow
                                            account
                                            me
                                            largest=largest.get()
                                            provider=provider.get()
                                            clock
                                            edit
                                            update
                                            ask_delete
                                            cancel_deletion
                                        />
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>
        </section>
        <p class="card__note">{FOOTNOTE}</p>
        {move || {
            adding
                .get()
                .then(|| {
                    view! {
                        <AddUserDialog
                            auth
                            disk_free=disk_free.get()
                            on_close=Callback::new(move |created: Option<Account>| {
                                adding.set(false);
                                if let Some(created) = created {
                                    replace.run(created);
                                }
                            })
                        />
                    }
                })
        }}
        {move || {
            // Keyed by the id alone, so that the panel stays open, with what was typed into it,
            // while the account changes underneath.
            editing
                .get()
                .and_then(|id| {
                    let first = accounts.with_untracked(|all| all.iter().find(|a| a.id == id).cloned())?;
                    Some(Signal::derive(move || {
                        accounts
                            .with(|all| all.iter().find(|a| a.id == id).cloned())
                            .unwrap_or_else(|| first.clone())
                    }))
                })
                .map(|account| {
                    view! {
                        <EditUserPanel
                            auth
                            account
                            me
                            provider=provider.get_untracked()
                            clock
                            on_close=Callback::new(move |()| editing.set(None))
                            on_saved=replace
                            ask_delete
                            cancel_deletion
                        />
                    }
                })
        }}
        {move || {
            deleting
                .get()
                .map(|account| {
                    view! {
                        <DeleteUserDialog
                            account
                            delay=delay.get()
                            on_cancel=Callback::new(move |()| deleting.set(None))
                            on_confirm=confirm_delete
                        />
                    }
                })
        }}
    }
}

#[component]
fn AccountRow(
    account: Account,
    me: Option<UserId>,
    largest: u64,
    provider: Option<String>,
    clock: RwSignal<time::OffsetDateTime>,
    edit: Callback<UserId>,
    update: Callback<(UserId, UpdateAccount)>,
    ask_delete: Callback<Account>,
    cancel_deletion: Callback<UserId>,
) -> impl IntoView {
    let id = account.id;
    let you = me == Some(id);
    let method = sign_in_method(&account, provider.as_deref());
    let detail = if you {
        format!("{method} · You")
    } else {
        method
    };
    let (pill_class, pill_label) = state_pill(&account);
    let marked = account.delete_at.is_some();
    let last_seen = if you {
        "Now".to_owned()
    } else {
        account
            .last_seen_at
            .map_or_else(|| "Never".to_owned(), short_date)
    };
    let fill = storage_fill(&account, largest) * 100.0;
    let near = near_quota(&account);
    let note_account = account.clone();
    let deletion = move || deletion_note(&note_account, clock.get());
    let is_admin = account.is_admin;
    let disabled = account.disabled;
    let delete_target = account.clone();

    view! {
        <tr class:accounts-table__row--inactive=!account.active()>
            <td>
                <button class="user-cell user-cell--button" on:click=move |_| edit.run(id)>
                    <span class="avatar" aria-hidden="true">{initial(&account.username)}</span>
                    <span class="user-cell__text">
                        <span class="user-cell__name">{account.username.clone()}</span>
                        <span class="user-cell__detail">{detail}</span>
                    </span>
                </button>
            </td>
            <td>
                {if is_admin {
                    view! { <span class="pill pill--accent">"Admin"</span> }.into_any()
                } else {
                    view! { <span class="pill">"Member"</span> }.into_any()
                }}
            </td>
            <td class="data-table__storage">
                <span class="usage">
                    <span class="usage__text">{storage_label(&account)}</span>
                    <span class="meter" aria-hidden="true">
                        <span
                            class="meter__fill"
                            class:meter__fill--danger=near
                            style=format!("width: {fill:.2}%")
                        ></span>
                    </span>
                </span>
            </td>
            <td class="data-table__number">{account.devices}</td>
            <td class="accounts-table__seen">{last_seen}</td>
            <td>
                <span class="account-state">
                    <span class=pill_class>{pill_label}</span>
                    {marked.then(|| view! { <span class="account-state__note">{deletion}</span> })}
                </span>
            </td>
            <td class="accounts-table__actions">
                <span class="accounts-table__buttons">
                    {marked
                        .then(|| {
                            view! {
                                <button
                                    class="button button--small"
                                    on:click=move |_| cancel_deletion.run(id)
                                >
                                    <Icon name="restore_from_trash" />
                                    "Cancel deletion"
                                </button>
                            }
                        })}
                    <Menu id=format!("account-menu-{id}") label="User actions">
                        <MenuItem icon="edit" label="Edit" on_select=Callback::new(move |()| edit.run(id)) />
                        {(!you && !marked)
                            .then(|| {
                                let (icon, label) = if is_admin {
                                    ("person", "Remove admin")
                                } else {
                                    ("admin_panel_settings", "Make admin")
                                };
                                let toggle = Callback::new(move |()| {
                                    update.run((
                                        id,
                                        UpdateAccount {
                                            is_admin: Some(!is_admin),
                                            ..UpdateAccount::default()
                                        },
                                    ))
                                });
                                let (state_icon, state_label) = if disabled {
                                    ("done", "Enable")
                                } else {
                                    ("block", "Disable")
                                };
                                let switch_state = Callback::new(move |()| {
                                    update.run((
                                        id,
                                        UpdateAccount {
                                            disabled: Some(!disabled),
                                            ..UpdateAccount::default()
                                        },
                                    ))
                                });
                                let target = delete_target.clone();
                                view! {
                                    <MenuItem icon label on_select=toggle />
                                    <MenuItem icon=state_icon label=state_label on_select=switch_state />
                                    <div class="menu__divider" role="separator"></div>
                                    <MenuItem
                                        icon="delete"
                                        label="Delete"
                                        danger=true
                                        on_select=Callback::new(move |()| ask_delete.run(target.clone()))
                                    />
                                }
                            })}
                        {marked
                            .then(|| {
                                view! {
                                    <MenuItem
                                        icon="restore_from_trash"
                                        label="Cancel deletion"
                                        on_select=Callback::new(move |()| cancel_deletion.run(id))
                                    />
                                }
                            })}
                    </Menu>
                </span>
            </td>
        </tr>
    }
}

/// A switch for a yes/no setting.
#[component]
fn Switch(
    checked: RwSignal<bool>,
    #[prop(into)] label: String,
    #[prop(optional)] disabled: bool,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="switch"
            role="switch"
            aria-label=label
            aria-checked=move || checked.get().to_string()
            disabled=disabled
            on:click=move |_| checked.update(|on| *on = !*on)
        >
            <span class="switch__thumb"></span>
        </button>
    }
}

/// A quota typed as a number with a GB/TB choice; empty means no quota.
/// The quota as it is being typed: an empty field means no quota.
#[derive(Clone, Copy)]
struct QuotaInput {
    text: RwSignal<String>,
    unit: RwSignal<QuotaUnit>,
    /// The field holds something the browser does not take for a number.
    bad: RwSignal<bool>,
}

impl QuotaInput {
    fn new(quota: Option<u64>) -> Self {
        let (text, unit) = quota_input(quota);
        Self {
            text: RwSignal::new(text),
            unit: RwSignal::new(unit),
            bad: RwSignal::new(false),
        }
    }

    fn parse(self) -> Result<Option<u64>, &'static str> {
        if self.bad.get_untracked() {
            return Err(NOT_A_QUOTA);
        }
        parse_quota(&self.text.get_untracked(), self.unit.get_untracked())
    }
}

#[component]
fn QuotaField(quota: QuotaInput) -> impl IntoView {
    let QuotaInput { text, unit, bad } = quota;
    view! {
        <span class="quota-field">
            <input
                type="number"
                min="0"
                step="any"
                class="field__input quota-field__input"
                inputmode="decimal"
                placeholder="No quota"
                aria-label="Storage quota"
                prop:value=move || text.get()
                on:input:target=move |event| {
                    let input = event.target();
                    // What is no number reads as an empty value, which must not mean no
                    // quota. Leaving the text alone also keeps it in the field.
                    let is_bad = input.validity().bad_input();
                    bad.set(is_bad);
                    if !is_bad {
                        text.set(input.value());
                    }
                }
            />
            <span class="segmented" role="group" aria-label="Unit">
                {[QuotaUnit::Gb, QuotaUnit::Tb]
                    .into_iter()
                    .map(|choice| {
                        view! {
                            <button
                                type="button"
                                class="segmented__option"
                                class:segmented__option--active=move || unit.get() == choice
                                aria-pressed=move || (unit.get() == choice).to_string()
                                on:click=move |_| unit.set(choice)
                            >
                                {choice.label()}
                            </button>
                        }
                    })
                    .collect_view()}
            </span>
        </span>
    }
}

#[component]
fn AddUserDialog(
    auth: Auth,
    disk_free: Option<u64>,
    on_close: Callback<Option<Account>>,
) -> impl IntoView {
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let repeat = RwSignal::new(String::new());
    let quota = QuotaInput::new(None);
    let is_admin = RwSignal::new(false);
    let problem = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let quota_hint = match disk_free {
        Some(free) => format!(
            "Leave empty for no quota. {} free on the disk.",
            bytes(free)
        ),
        None => "Leave empty for no quota.".to_owned(),
    };

    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let name = username.get_untracked().trim().to_owned();
        if name.is_empty() {
            problem.set(Some("Enter a username.".to_owned()));
            return;
        }
        let secret = password.get_untracked();
        if let Some(message) = password_problem(&secret, &repeat.get_untracked()) {
            problem.set(Some(message.to_owned()));
            return;
        }
        let quota = match quota.parse() {
            Ok(quota) => quota,
            Err(message) => {
                problem.set(Some(message.to_owned()));
                return;
            }
        };
        let request = CreateAccount {
            username: name,
            password: secret,
            is_admin: is_admin.get_untracked(),
            quota_bytes: quota,
        };
        busy.set(true);
        spawn_local(async move {
            match session::client().create_account(&request).await {
                Ok(created) => on_close.run(Some(created)),
                Err(err) => {
                    busy.set(false);
                    problem.set(Some(match err {
                        ApiError::Api {
                            code: protocol::ErrorCode::Conflict,
                            ..
                        } => "That username is taken.".to_owned(),
                        other => auth.fail(&other),
                    }));
                }
            }
        });
    };

    view! {
        <div class="modal" on:click=move |_| on_close.run(None)>
            <form
                class="modal__panel modal__panel--form"
                role="dialog"
                aria-modal="true"
                aria-labelledby="add-user-title"
                on:click=|event| event.stop_propagation()
                on:submit=submit
            >
                <header class="modal__header modal__header--close">
                    <h2 class="modal__title" id="add-user-title">"Add user"</h2>
                    <button
                        type="button"
                        class="button button--ghost button--small"
                        aria-label="Close"
                        on:click=move |_| on_close.run(None)
                    >
                        <Icon name="close" />
                    </button>
                    <p class="modal__subtitle">
                        "Creates an account that signs in with a password. Single sign-on users are created on their first sign-in."
                    </p>
                </header>
                <label class="field">
                    <span class="field__label">"Username"</span>
                    <input
                        class="field__input"
                        autocomplete="off"
                        autofocus
                        prop:value=move || username.get()
                        on:input:target=move |event| username.set(event.target().value())
                    />
                </label>
                <label class="field">
                    <span class="field__label">"Password"</span>
                    <input
                        class="field__input"
                        type="password"
                        autocomplete="new-password"
                        prop:value=move || password.get()
                        on:input:target=move |event| password.set(event.target().value())
                    />
                </label>
                <label class="field">
                    <span class="field__label">"Repeat password"</span>
                    <input
                        class="field__input"
                        type="password"
                        autocomplete="new-password"
                        prop:value=move || repeat.get()
                        on:input:target=move |event| repeat.set(event.target().value())
                    />
                </label>
                <div class="field">
                    <span class="field__label">"Storage quota"</span>
                    <QuotaField quota />
                    <span class="field__hint">{quota_hint}</span>
                </div>
                <div class="setting-row">
                    <span class="setting-row__text">
                        <span class="setting-row__title">"Administrator"</span>
                        <span class="setting-row__detail">
                            "Can manage users and see the server status."
                        </span>
                    </span>
                    <Switch checked=is_admin label="Administrator" />
                </div>
                {move || {
                    problem
                        .get()
                        .map(|message| {
                            view! { <p class="alert alert--error" role="alert">{message}</p> }
                        })
                }}
                <div class="modal__actions">
                    <button type="button" class="button" on:click=move |_| on_close.run(None)>
                        "Cancel"
                    </button>
                    <button type="submit" class="button button--primary" disabled=move || busy.get()>
                        "Add user"
                    </button>
                </div>
            </form>
        </div>
    }
}

#[component]
fn EditUserPanel(
    auth: Auth,
    account: Signal<Account>,
    me: Option<UserId>,
    provider: Option<String>,
    clock: RwSignal<time::OffsetDateTime>,
    on_close: Callback<()>,
    on_saved: Callback<Account>,
    ask_delete: Callback<Account>,
    cancel_deletion: Callback<UserId>,
) -> impl IntoView {
    let first = account.get_untracked();
    let id = first.id;
    let you = me == Some(id);
    let name = first.username.clone();
    let has_password = !first.external;
    let method = sign_in_method(&first, provider.as_deref());
    let since = format!("{method} · since {}", medium_date(first.created_at));
    let quota = QuotaInput::new(first.quota_bytes);
    let is_admin = RwSignal::new(first.is_admin);
    let password = RwSignal::new(String::new());
    let repeat = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let devices = RwSignal::new(None::<Vec<AccountDevice>>);
    load::load(
        move || async move { session::client().account_devices(id).await },
        move |result| match result {
            Ok(found) => devices.set(Some(found)),
            Err(err) => problem.set(Some(auth.fail(&err))),
        },
    );

    let pill = move || account.with(state_pill);
    let usage_text = move || {
        account.with(|account| {
            let usage = account.usage;
            match account.quota_bytes {
                Some(quota) => format!(
                    "{} of {} used · {}",
                    bytes(usage.total()),
                    bytes(quota),
                    usage_breakdown(usage)
                ),
                None => format!(
                    "{} used, no quota · {}",
                    bytes(usage.total()),
                    usage_breakdown(usage)
                ),
            }
        })
    };
    let fill = move || account.with(|a| storage_fill(a, a.usage.total().max(1))) * 100.0;
    let near = move || account.with(near_quota);
    let marked = move || account.with(|a| a.delete_at.is_some());
    let disabled = move || account.with(|a| a.disabled);
    let deletion = move || account.with(|a| deletion_note(a, clock.get()));

    let save = move |event: SubmitEvent| {
        event.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let quota = match quota.parse() {
            Ok(quota) => quota,
            Err(message) => {
                problem.set(Some(message.to_owned()));
                return;
            }
        };
        let new_password = password.get_untracked();
        let repeated = repeat.get_untracked();
        let change_password = has_password && !(new_password.is_empty() && repeated.is_empty());
        if change_password && let Some(message) = password_problem(&new_password, &repeated) {
            problem.set(Some(message.to_owned()));
            return;
        }
        let admin = is_admin.get_untracked();
        let current = account.get_untracked();
        let change = UpdateAccount {
            is_admin: (admin != current.is_admin).then_some(admin),
            disabled: None,
            quota_bytes: (quota != current.quota_bytes).then_some(quota),
        };
        busy.set(true);
        spawn_local(async move {
            let client = session::client();
            let result = async {
                let updated = if change == UpdateAccount::default() {
                    None
                } else {
                    Some(client.update_account(id, &change).await?)
                };
                if change_password {
                    client.set_account_password(id, &new_password).await?;
                }
                Ok::<_, ApiError>(updated)
            }
            .await;
            busy.set(false);
            match result {
                Ok(updated) => {
                    on_close.run(());
                    if let Some(updated) = updated {
                        on_saved.run(updated);
                    }
                }
                Err(err) => problem.set(Some(auth.fail(&err))),
            }
        });
    };
    let switch_disabled = move |_| {
        let disabled = disabled();
        spawn_local(async move {
            let change = UpdateAccount {
                disabled: Some(!disabled),
                ..UpdateAccount::default()
            };
            match session::client().update_account(id, &change).await {
                Ok(updated) => on_saved.run(updated),
                Err(err) => problem.set(Some(auth.fail(&err))),
            }
        });
    };

    view! {
        <div class="sheet" on:click=move |_| on_close.run(())>
            <form
                class="sheet__panel"
                role="dialog"
                aria-modal="true"
                aria-labelledby="edit-user-title"
                on:click=|event| event.stop_propagation()
                on:submit=save
            >
                <header class="sheet__header">
                    <span class="avatar avatar--large" aria-hidden="true">{initial(&name)}</span>
                    <span class="sheet__who">
                        <h2 class="sheet__title" id="edit-user-title">{name.clone()}</h2>
                        <span class="sheet__meta">
                            <span class=move || pill().0>{move || pill().1}</span>
                            {since}
                        </span>
                    </span>
                    <button
                        type="button"
                        class="button button--ghost button--small"
                        aria-label="Close"
                        on:click=move |_| on_close.run(())
                    >
                        <Icon name="close" />
                    </button>
                </header>
                <div class="sheet__body">
                    <section class="sheet__section">
                        <label class="field">
                            <span class="field__label">"Username"</span>
                            <input class="field__input" value=name.clone() readonly />
                        </label>
                    </section>
                    <section class="sheet__section">
                        <div class="setting-row">
                            <span class="setting-row__text">
                                <span class="setting-row__title">"Administrator"</span>
                                <span class="setting-row__detail">
                                    {if you {
                                        "You cannot remove your own administrator rights."
                                    } else {
                                        "Can manage users and see the server status."
                                    }}
                                </span>
                            </span>
                            <Switch checked=is_admin label="Administrator" disabled=you />
                        </div>
                    </section>
                    <section class="sheet__section">
                        <h3 class="sheet__heading">"Storage"</h3>
                        <p class="sheet__text">{usage_text}</p>
                        <span class="meter meter--large" aria-hidden="true">
                            <span
                                class="meter__fill"
                                class:meter__fill--danger=near
                                style=move || format!("width: {:.2}%", fill())
                            ></span>
                        </span>
                        <QuotaField quota />
                        <span class="field__hint">
                            "Leave empty for no quota. Quotas are recorded but not enforced yet."
                        </span>
                    </section>
                    <section class="sheet__section">
                        <h3 class="sheet__heading">"Password"</h3>
                        {if has_password {
                            view! {
                                <p class="sheet__text">
                                    {format!(
                                        "Leave empty to keep the current password. A new password signs {name} out on every device.",
                                    )}
                                </p>
                                <label class="field">
                                    <span class="field__label">"New password"</span>
                                    <input
                                        class="field__input"
                                        type="password"
                                        autocomplete="new-password"
                                        prop:value=move || password.get()
                                        on:input:target=move |event| password.set(event.target().value())
                                    />
                                </label>
                                <label class="field">
                                    <span class="field__label">"Repeat new password"</span>
                                    <input
                                        class="field__input"
                                        type="password"
                                        autocomplete="new-password"
                                        prop:value=move || repeat.get()
                                        on:input:target=move |event| repeat.set(event.target().value())
                                    />
                                </label>
                            }
                                .into_any()
                        } else {
                            view! {
                                <p class="sheet__text">
                                    {format!(
                                        "{name} signs in through {method} and has no password here. Manage it at the provider.",
                                    )}
                                </p>
                            }
                                .into_any()
                        }}
                    </section>
                    <section class="sheet__section">
                        <h3 class="sheet__heading">"Devices"</h3>
                        {move || match devices.get() {
                            None => view! { <p class="sheet__text">"Loading…"</p> }.into_any(),
                            Some(found) if found.is_empty() => {
                                view! { <p class="sheet__text">"No devices have signed in yet."</p> }
                                    .into_any()
                            }
                            Some(found) => {
                                view! {
                                    <ul class="device-list">
                                        {found
                                            .into_iter()
                                            .map(|device| {
                                                view! {
                                                    <li class="device-list__item">
                                                        <Icon name=platform_icon(&device.platform) />
                                                        <span class="device-list__text">
                                                            {format!(
                                                                "{} · {} · {}",
                                                                device.name,
                                                                platform(&device.platform),
                                                                short_date(device.last_seen_at),
                                                            )}
                                                        </span>
                                                        {device
                                                            .signed_in
                                                            .then(|| {
                                                                view! {
                                                                    <span class="pill pill--success">"Signed in"</span>
                                                                }
                                                            })}
                                                    </li>
                                                }
                                            })
                                            .collect_view()}
                                    </ul>
                                }
                                    .into_any()
                            }
                        }}
                    </section>
                    <section class="sheet__section">
                        <h3 class="sheet__heading">"Account"</h3>
                        {move || if you {
                            view! {
                                <p class="sheet__text">
                                    "You cannot disable or delete your own account."
                                </p>
                            }
                                .into_any()
                        } else if marked() {
                            view! {
                                <p class="alert alert--error account-deletion" role="status">
                                    <span>
                                        <strong>"Marked for deletion. "</strong>
                                        {deletion}
                                    </span>
                                </p>
                                <div class="sheet__buttons">
                                    <button
                                        type="button"
                                        class="button"
                                        on:click=move |_| cancel_deletion.run(id)
                                    >
                                        <Icon name="restore_from_trash" />
                                        "Cancel deletion"
                                    </button>
                                </div>
                            }
                                .into_any()
                        } else {
                            let disabled = disabled();
                            let name = name.clone();
                            view! {
                                <div class="sheet__buttons">
                                    <button type="button" class="button" on:click=switch_disabled>
                                        <Icon name=if disabled { "done" } else { "block" } />
                                        {if disabled { "Enable account" } else { "Disable account" }}
                                    </button>
                                    <button
                                        type="button"
                                        class="button button--ghost button--danger"
                                        on:click=move |_| ask_delete.run(account.get_untracked())
                                    >
                                        <Icon name="delete" />
                                        "Delete user…"
                                    </button>
                                </div>
                                <p class="sheet__text">
                                    {if disabled {
                                        format!("{name} cannot sign in until the account is enabled.")
                                    } else {
                                        format!(
                                            "Disabling signs {name} out and blocks sign-in; files are kept. Deleting removes {name}'s files, versions and recycle bin.",
                                        )
                                    }}
                                </p>
                            }
                                .into_any()
                        }}
                    </section>
                    {move || {
                        problem
                            .get()
                            .map(|message| {
                                view! { <p class="alert alert--error" role="alert">{message}</p> }
                            })
                    }}
                </div>
                <footer class="sheet__footer">
                    <button type="button" class="button" on:click=move |_| on_close.run(())>
                        "Cancel"
                    </button>
                    <button type="submit" class="button button--primary" disabled=move || busy.get()>
                        "Save"
                    </button>
                </footer>
            </form>
        </div>
    }
}

#[component]
fn DeleteUserDialog(
    account: Account,
    delay: u64,
    on_cancel: Callback<()>,
    on_confirm: Callback<Account>,
) -> impl IntoView {
    let name = account.username.clone();
    let used = bytes(account.usage.total());
    view! {
        <div class="modal modal--top" on:click=move |_| on_cancel.run(())>
            <div
                class="modal__panel"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="delete-user-title"
                on:click=|event| event.stop_propagation()
            >
                <header class="modal__header">
                    <h2 class="modal__title" id="delete-user-title">
                        {format!("Delete {name}?")}
                    </h2>
                    <p class="modal__subtitle">
                        {format!(
                            "All of {name}'s data will be permanently deleted: files, previous versions and the recycle bin ({used} in all), and their devices.",
                        )}
                    </p>
                    <p class="modal__subtitle">
                        {format!(
                            "{name} is signed out now and the account is deleted {}. Until then you can cancel the deletion.",
                            delay_label(delay),
                        )}
                    </p>
                </header>
                <div class="modal__actions">
                    <button class="button" on:click=move |_| on_cancel.run(())>
                        "Cancel"
                    </button>
                    <button
                        class="button button--danger"
                        on:click=move |_| on_confirm.run(account.clone())
                    >
                        "Delete user"
                    </button>
                </div>
            </div>
        </div>
    }
}
