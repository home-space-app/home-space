//! Administration: the server's status, disk and the storage of every user.

use leptos::prelude::*;
use time::{Duration, OffsetDateTime};

use api_client::ApiError;
use client_core::storage::{
    UserColumn, UserSort, bytes, disk_breakdown, duration, fraction, uptime, usage_breakdown,
    users_total,
};
use protocol::{ErrorCode, ServerConfig, ServerStatus, StorageBackend, UserStorage};

use super::{
    shell::{Tab, Tabs},
    sidebar::initial,
};
use crate::{
    components::ErrorAlert,
    icon::Icon,
    load, route,
    session::{self, Auth},
};

const NOT_AN_ADMIN: &str = "Only administrators can open the administration pages.";

/// One read-only setting: what it is, its value, an optional note and where it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigRow {
    pub label: &'static str,
    pub value: String,
    pub note: Option<String>,
    pub source: &'static str,
}

fn row(label: &'static str, value: impl Into<String>, source: &'static str) -> ConfigRow {
    ConfigRow {
        label,
        value: value.into(),
        note: None,
        source,
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "On" } else { "Off" }
}

/// The settings shown in the configuration card, in groups.
pub fn config_groups(
    config: &ServerConfig,
    storage_root: &str,
    database: &str,
) -> Vec<(&'static str, Vec<ConfigRow>)> {
    let retention = config.changes_retention_days.map_or_else(
        || "Forever".to_owned(),
        |days| duration(Duration::days(days)),
    );
    let mut sign_in = vec![row(
        "Password sign-in",
        on_off(config.password_login),
        "HOMESPACE_PASSWORD_LOGIN",
    )];
    match &config.oidc {
        Some(oidc) => {
            sign_in.push(ConfigRow {
                note: Some(oidc.issuer.clone()),
                ..row("Single sign-on", oidc.name.clone(), "HOMESPACE_OIDC_ISSUER")
            });
            sign_in.push(row(
                "Required group",
                oidc.required_group
                    .clone()
                    .unwrap_or_else(|| "None".to_owned()),
                "HOMESPACE_OIDC_REQUIRED_GROUP",
            ));
        }
        None => sign_in.push(row("Single sign-on", "Off", "HOMESPACE_OIDC_ISSUER")),
    }
    sign_in.push(row(
        "Session lifetime",
        format!(
            "{}, renewed for {}",
            duration(Duration::seconds(config.access_ttl_secs)),
            duration(Duration::seconds(config.refresh_ttl_secs))
        ),
        "HOMESPACE_ACCESS_TTL_SECS · _REFRESH_TTL_SECS",
    ));
    sign_in.push(ConfigRow {
        note: Some(
            if config.previous_jwt_secret {
                "previous secret still accepted"
            } else {
                "no previous secret"
            }
            .to_owned(),
        ),
        ..row("Token signing secret", "Set", "HOMESPACE_JWT_SECRET")
    });
    vec![
        (
            "Uploads and storage",
            vec![
                row(
                    "Maximum upload size",
                    bytes(config.max_upload_bytes),
                    "HOMESPACE_MAX_UPLOAD_BYTES",
                ),
                row(
                    "Upload chunk size",
                    bytes(config.upload_chunk_bytes),
                    "HOMESPACE_UPLOAD_CHUNK_BYTES",
                ),
                row(
                    "Unfinished uploads expire",
                    match config.upload_expiry_hours {
                        1 => "After 1 hour".to_owned(),
                        hours => format!("After {hours} hours"),
                    },
                    "HOMESPACE_UPLOAD_EXPIRY_HOURS",
                ),
                row(
                    "Storage backend",
                    match config.storage {
                        StorageBackend::Local => "Local disk",
                    },
                    "HOMESPACE_STORAGE",
                ),
                row("Storage root", storage_root, "HOMESPACE_STORAGE_ROOT"),
                row(
                    "Change history kept",
                    retention,
                    "HOMESPACE_CHANGES_RETENTION_DAYS",
                ),
            ],
        ),
        ("Sign-in", sign_in),
        (
            "Server",
            vec![
                row(
                    "Public address",
                    config
                        .public_url
                        .clone()
                        .unwrap_or_else(|| "Not set".to_owned()),
                    "HOMESPACE_PUBLIC_URL",
                ),
                row("Listens on", config.bind.clone(), "HOMESPACE_BIND"),
                ConfigRow {
                    note: config
                        .published_port
                        .map(|_| "on the container host".to_owned()),
                    ..row(
                        "Published port",
                        config
                            .published_port
                            .map_or_else(|| "Not set".to_owned(), |port| port.to_string()),
                        "HOMESPACE_PORT",
                    )
                },
                ConfigRow {
                    note: Some(database.to_owned()),
                    ..row("Database", "Connected", "DATABASE_URL")
                },
            ],
        ),
    ]
}

fn now() -> OffsetDateTime {
    let nanos = (js_sys::Date::now() * 1_000_000.0) as i128;
    OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

#[component]
pub fn AdminPage(auth: Auth) -> impl IntoView {
    let status = RwSignal::new(None::<ServerStatus>);
    let error = RwSignal::new(None::<String>);
    let resource = load::load(
        || async { session::client().server_status().await },
        move |result| match result {
            Ok(loaded) => {
                status.set(Some(loaded));
                error.set(None);
            }
            Err(ApiError::Api {
                code: ErrorCode::Forbidden,
                ..
            }) => error.set(Some(NOT_AN_ADMIN.to_owned())),
            Err(err) => error.set(Some(auth.fail(&err))),
        },
    );
    let me = move || auth.user.get().map(|me| me.id);

    view! {
        <main class="page">
            <header class="page-header">
                <h1 class="page-title">"Administration"</h1>
                {move || {
                    status
                        .get()
                        .map(|status| {
                            view! {
                                <span class="pill pill--success">"Running"</span>
                                <span class="page-header__meta">
                                    {format!(
                                        "Version {} · {}",
                                        status.version,
                                        uptime(status.started_at, now()),
                                    )}
                                </span>
                            }
                        })
                }}
                <span class="page-header__spacer"></span>
                <button class="button" on:click=move |_| resource.refetch()>
                    <Icon name="refresh" />
                    <span class="label">"Refresh"</span>
                </button>
            </header>
            <Tabs>
                <Tab href=route::ADMIN_PATH icon="dns" label="Server status" />
            </Tabs>
            <ErrorAlert message=error />
            {move || {
                status
                    .get()
                    .map(|status| {
                        view! {
                            <DiskCard status=status.clone() />
                            <UsersCard status=status.clone() me=me() />
                            <ConfigCard status />
                        }
                    })
            }}
        </main>
    }
}

#[component]
fn DiskCard(status: ServerStatus) -> impl IntoView {
    let usage = status.total_usage();
    let path = status.disk.path.clone();
    let Some(disk) = status.disk.space else {
        return view! {
            <section class="card">
                <h2 class="card__title">"Disk"</h2>
                <p class="card__subtitle">
                    {format!("The server cannot read the size of the disk that holds {path}.")}
                </p>
                <p class="card__note">{format!("Home Space data: {}", usage_breakdown(usage))}</p>
            </section>
        }
        .into_any();
    };
    let split = disk_breakdown(disk, usage);
    let parts = [
        ("Home Space", split.home_space, "segment--home"),
        ("Other data", split.other, "segment--other"),
        ("Free", split.free, "segment--free"),
    ];
    view! {
        <section class="card">
            <h2 class="card__title">"Disk"</h2>
            <p class="card__subtitle">
                {format!(
                    "The filesystem that holds {path} · {} free of {}",
                    bytes(split.free),
                    bytes(split.total),
                )}
            </p>
            <div class="segments segments--large" aria-hidden="true">
                {parts
                    .iter()
                    .map(|(_, size, class)| {
                        view! {
                            <span
                                class=format!("segment {class}")
                                style=format!("width: {:.2}%", fraction(*size, split.total) * 100.0)
                            ></span>
                        }
                    })
                    .collect_view()}
            </div>
            <ul class="legend">
                {parts
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
                    .collect_view()}
            </ul>
            <p class="card__note">{format!("Home Space data: {}", usage_breakdown(usage))}</p>
        </section>
    }
    .into_any()
}

#[component]
fn UsersCard(status: ServerStatus, me: Option<protocol::UserId>) -> impl IntoView {
    let largest = status
        .users
        .iter()
        .map(|user| user.usage.total())
        .max()
        .unwrap_or_default();
    let sort = RwSignal::new(UserSort::default());
    let summary = users_total(&status);
    let users = status.users;
    view! {
        <section class="card card--table">
            <div class="card__head">
                <h2 class="card__title">"Storage by user"</h2>
                <p class="card__subtitle">
                    {format!(
                        "{summary}. Each figure counts current files, previous versions and the recycle bin.",
                    )}
                </p>
            </div>
            <div class="table-scroll">
                <table class="data-table">
                    <thead>
                        <tr>
                            <SortHeader sort column=UserColumn::Name label="User" class="" />
                            <SortHeader sort column=UserColumn::Storage label="Storage" class="data-table__storage" />
                            <SortHeader sort column=UserColumn::Files label="Files" class="data-table__number" />
                            <SortHeader sort column=UserColumn::Versions label="Versions" class="data-table__number" />
                            <SortHeader sort column=UserColumn::Trash label="Recycle bin" class="data-table__number" />
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || {
                                let mut users = users.clone();
                                sort.get().apply(&mut users);
                                users
                            }
                            key=|user| user.id
                            children=move |user| view! { <UserRow user largest you=me /> }
                        />
                    </tbody>
                </table>
            </div>
        </section>
    }
}

/// A column heading that sorts the table by that column; clicking it again reverses the order.
#[component]
fn SortHeader(
    sort: RwSignal<UserSort>,
    column: UserColumn,
    label: &'static str,
    class: &'static str,
) -> impl IntoView {
    let active = move || sort.get().column == column;
    let descending = move || sort.get().descending;
    view! {
        <th
            class=class
            aria-sort=move || match (active(), descending()) {
                (false, _) => "none",
                (true, false) => "ascending",
                (true, true) => "descending",
            }
        >
            <button
                class="sort-button"
                class:sort-button--active=active
                class:sort-button--descending=descending
                on:click=move |_| sort.update(|sort| *sort = sort.toggle(column))
            >
                {label}
                {move || {
                    let name = if active() { "arrow_upward" } else { "unfold_more" };
                    view! { <Icon name /> }
                }}
            </button>
        </th>
    }
}

#[component]
fn UserRow(user: UserStorage, largest: u64, you: Option<protocol::UserId>) -> impl IntoView {
    let method = if user.external {
        "Single sign-on"
    } else {
        "Password"
    };
    let detail = if you == Some(user.id) {
        format!("{method} · You")
    } else {
        method.to_owned()
    };
    let total = user.usage.total();
    view! {
        <tr>
            <td>
                <span class="user-cell">
                    <span class="avatar" aria-hidden="true">{initial(&user.username)}</span>
                    <span class="user-cell__text">
                        <span class="user-cell__name">
                            {user.username.clone()}
                            {user.is_admin.then(|| view! { <span class="pill pill--accent">"Admin"</span> })}
                        </span>
                        <span class="user-cell__detail">{detail}</span>
                    </span>
                </span>
            </td>
            <td class="data-table__storage">
                <span class="usage">
                    <span class="usage__text">{bytes(total)}</span>
                    <span class="meter" aria-hidden="true">
                        <span
                            class="meter__fill"
                            style=format!("width: {:.2}%", fraction(total, largest) * 100.0)
                        ></span>
                    </span>
                </span>
            </td>
            <td class="data-table__number">{bytes(user.usage.files)}</td>
            <td class="data-table__number">{bytes(user.usage.versions)}</td>
            <td class="data-table__number">{bytes(user.usage.trash)}</td>
        </tr>
    }
}

#[component]
fn ConfigCard(status: ServerStatus) -> impl IntoView {
    let database = format!(
        "{} · {} migrations applied",
        status.database.version, status.database.migrations
    );
    let groups = config_groups(&status.config, &status.disk.path, &database);
    view! {
        <section class="card card--table">
            <div class="card__head">
                <h2 class="card__title">"Configuration"</h2>
                <p class="card__subtitle">
                    "Read-only. Set through environment variables; restart the server to apply changes."
                </p>
            </div>
            <div class="table-scroll">
                <table class="data-table config-table">
                    {groups
                        .into_iter()
                        .map(|(group, rows)| {
                            view! {
                                <tbody>
                                    <tr class="config-table__group">
                                        <th colspan="3" scope="colgroup">{group}</th>
                                    </tr>
                                    {rows
                                        .into_iter()
                                        .map(|row| {
                                            view! {
                                                <tr>
                                                    <th scope="row">{row.label}</th>
                                                    <td>
                                                        <span class="config-table__value">{row.value}</span>
                                                        {row.note.map(|note| view! { <span class="config-table__note">{note}</span> })}
                                                    </td>
                                                    <td class="config-table__source">{row.source}</td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            }
                        })
                        .collect_view()}
                </table>
            </div>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use protocol::OidcStatus;

    use super::*;

    fn config() -> ServerConfig {
        ServerConfig {
            max_upload_bytes: 10 * 1024 * 1024 * 1024,
            upload_chunk_bytes: 32 * 1024 * 1024,
            upload_expiry_hours: 24,
            transfer_ttl_hours: 24,
            max_transfer_bytes: None,
            changes_retention_days: None,
            password_login: true,
            oidc: None,
            access_ttl_secs: 900,
            refresh_ttl_secs: 30 * 24 * 3600,
            previous_jwt_secret: false,
            public_url: None,
            bind: "0.0.0.0:8080".to_owned(),
            published_port: None,
            storage: protocol::StorageBackend::Local,
        }
    }

    fn find<'a>(groups: &'a [(&str, Vec<ConfigRow>)], label: &str) -> &'a ConfigRow {
        groups
            .iter()
            .flat_map(|(_, rows)| rows)
            .find(|row| row.label == label)
            .expect(label)
    }

    #[test]
    fn config_reads_like_the_design() {
        let groups = config_groups(&config(), "/data", "PostgreSQL 17 · 16 migrations applied");
        assert_eq!(find(&groups, "Maximum upload size").value, "10.0 GB");
        assert_eq!(
            find(&groups, "Unfinished uploads expire").value,
            "After 24 hours"
        );
        assert_eq!(find(&groups, "Change history kept").value, "Forever");
        assert_eq!(
            find(&groups, "Session lifetime").value,
            "15 minutes, renewed for 30 days"
        );
        assert_eq!(find(&groups, "Single sign-on").value, "Off");
        assert_eq!(find(&groups, "Public address").value, "Not set");
    }

    #[test]
    fn single_sign_on_shows_its_issuer_and_group() {
        let mut config = config();
        config.oidc = Some(OidcStatus {
            name: "Keycloak".to_owned(),
            issuer: "https://auth.example.com/realms/home".to_owned(),
            required_group: Some("home-space-users".to_owned()),
        });
        let groups = config_groups(&config, "/data", "");
        let sso = find(&groups, "Single sign-on");
        assert_eq!(sso.value, "Keycloak");
        assert_eq!(
            sso.note.as_deref(),
            Some("https://auth.example.com/realms/home")
        );
        assert_eq!(find(&groups, "Required group").value, "home-space-users");
    }

    #[test]
    fn storage_backend_and_published_port_are_listed() {
        let mut config = config();
        let groups = config_groups(&config, "/data", "");
        assert_eq!(find(&groups, "Storage backend").value, "Local disk");
        assert_eq!(find(&groups, "Published port").value, "Not set");

        config.published_port = Some(8443);
        let groups = config_groups(&config, "/data", "");
        let port = find(&groups, "Published port");
        assert_eq!(port.value, "8443");
        assert_eq!(port.source, "HOMESPACE_PORT");
    }
}
