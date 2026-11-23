//! The questions `homespace-ctl install` asks, turned into an [`InstallPlan`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use inquire::{Confirm, Password, PasswordDisplayMode, Select, Text, validator::Validation};

use super::{
    plan::{
        Auth, InstallPlan, Oidc, Proxy, Runtime, Secret, SecretStore, ServiceUser, Storage,
        defaults,
    },
    render::env_safe,
    runtime, secrets, system,
};
use crate::installation::Engine;

type Check = Result<Validation, inquire::CustomUserError>;

fn valid_if(ok: bool, message: &str) -> Check {
    Ok(if ok {
        Validation::Valid
    } else {
        Validation::Invalid(message.into())
    })
}

/// Asks the user to pick one of `choices` (label and value); the first is the default.
fn choose<T: Clone>(message: &str, help: &str, choices: &[(&str, T)]) -> Result<T> {
    let labels: Vec<&str> = choices.iter().map(|(label, _)| *label).collect();
    let picked = Select::new(message, labels)
        .with_help_message(help)
        .raw_prompt()
        .context("reading the answer")?;
    Ok(choices[picked.index].1.clone())
}

fn text(
    message: &str,
    default: &str,
    check: impl Fn(&str) -> Check + Clone + 'static,
) -> Result<String> {
    let mut prompt = Text::new(message).with_validator(check);
    if !default.is_empty() {
        prompt = prompt.with_default(default);
    }
    Ok(prompt
        .prompt()
        .context("reading the answer")?
        .trim()
        .to_owned())
}

fn confirm(message: &str, default: bool) -> Result<bool> {
    Confirm::new(message)
        .with_default(default)
        .prompt()
        .context("reading the answer")
}

/// Asks everything and returns the installation to make.
pub fn ask(runtimes: &[Runtime]) -> Result<InstallPlan> {
    println!("Home Space installation. Press Enter to take the default answer.\n");
    let runtime = ask_runtime(runtimes)?;
    let project = std::env::var(defaults::PROJECT_VAR)
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| defaults::PROJECT.to_owned());
    if let Some(existing) = runtime::project_dir(runtime.engine, &project) {
        anyhow::bail!(
            "a compose project named {project} exists already, from {}: manage that one, \
             remove it first (compose down there), or set {} to install a second one",
            existing.display(),
            defaults::PROJECT_VAR
        );
    }
    let dir = ask_dir()?;
    let secrets = choose(
        "Where should the secrets be kept?",
        "secrets: one file per secret, mounted into the containers; .env: next to compose.yaml, mode 600",
        &[
            ("Secret files (compose secrets)", SecretStore::Files),
            (".env file next to compose.yaml", SecretStore::EnvFile),
        ],
    )?;
    let storage = ask_storage(runtime)?;
    let db_user = text("Database user:", defaults::DB_USER, |user: &str| {
        valid_if(
            is_db_identifier(user),
            "lowercase letters, digits and _, starting with a letter",
        )
    })?;
    let db_password = ask_secret("Database password", secrets::db_password, |value: &str| {
        valid_if(
            value.len() >= 12 && env_safe(value),
            "at least 12 characters, without ' or line breaks",
        )
    })?;
    let jwt_secret = ask_secret("JWT signing secret", secrets::jwt_secret, |value: &str| {
        if !env_safe(value) {
            return valid_if(false, "no ' or line breaks");
        }
        Ok(match auth::secrets::check_secret("the secret", value) {
            Ok(()) => Validation::Valid,
            Err(err) => Validation::Invalid(err.to_string().into()),
        })
    })?;
    let port = system::free_port()?;
    let proxy = ask_proxy()?;
    let mut public_url = ask_public_url(&proxy, port, false)?;
    let auth = ask_auth()?;
    if auth.oidc().is_some() && !auth::is_https_or_loopback(&public_url) {
        println!("Single sign-on needs the public URL to use https.");
        public_url = ask_public_url(&proxy, port, true)?;
    }
    let listen = if proxy == Proxy::None && !is_loopback_url(&public_url) {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    Ok(InstallPlan {
        runtime,
        dir,
        project,
        secrets,
        storage,
        db_user,
        db_password,
        jwt_secret,
        proxy,
        public_url,
        auth,
        port,
        listen: listen.to_owned(),
    })
}

fn ask_runtime(runtimes: &[Runtime]) -> Result<Runtime> {
    let runtime = match runtimes {
        [] => anyhow::bail!(
            "neither `docker compose` nor `podman compose` works here; install one of them first"
        ),
        [only] => *only,
        several => {
            let choices: Vec<(&str, Runtime)> = several
                .iter()
                .map(|runtime| (runtime.engine.program(), *runtime))
                .collect();
            choose(
                "Both Docker and Podman are available. Which one should run Home Space?",
                "the containers and `homespace-ctl` commands will use it",
                &choices,
            )?
        }
    };
    let mode = if runtime.rootless { " (rootless)" } else { "" };
    println!("Using {}{mode}.", runtime.engine.program());
    Ok(runtime)
}

fn ask_dir() -> Result<PathBuf> {
    ask_new_dir("Where should compose.yaml go?", defaults::DIR, None)
}

/// A directory that does not exist yet (and is not `other`): the installer creates it and
/// never takes over one that is there already.
fn ask_new_dir(message: &str, default: &str, other: Option<PathBuf>) -> Result<PathBuf> {
    text(message, default, move |dir: &str| {
        let path = Path::new(dir.trim());
        if !path.is_absolute() {
            return valid_if(false, "an absolute path");
        }
        if other.as_deref() == Some(path) {
            return valid_if(false, "a different directory");
        }
        valid_if(
            std::fs::symlink_metadata(path).is_err(),
            "exists already: the installer only creates new directories",
        )
    })
    .map(PathBuf::from)
}

fn ask_storage(runtime: Runtime) -> Result<Storage> {
    if runtime.engine == Engine::Docker && runtime.rootless {
        println!(
            "Rootless Docker remaps the containers' users, so the data goes into Docker volumes."
        );
        return Ok(Storage::Volumes);
    }
    let host_dirs = choose(
        "Where should the database and the files be stored?",
        "directories: plain folders you can back up; volumes: managed by the container engine",
        &[
            ("Directories on this machine", true),
            ("Container volumes", false),
        ],
    )?;
    if !host_dirs {
        return Ok(Storage::Volumes);
    }
    let db = ask_new_dir("Database directory:", defaults::DB_DIR, None)?;
    let files = ask_new_dir("Files directory:", defaults::FILES_DIR, Some(db.clone()))?;
    let user = ask_service_user(runtime)?;
    Ok(Storage::HostDirs { db, files, user })
}

fn ask_service_user(runtime: Runtime) -> Result<ServiceUser> {
    if runtime.engine == Engine::Podman && runtime.rootless {
        let me = system::current_user()?;
        println!(
            "Rootless Podman: the containers run as you ({}), who will own the directories. \
             For a dedicated user, run the installer with sudo.",
            me.name
        );
        return Ok(ServiceUser::Existing(me));
    }
    let create = choose(
        "Which user should own the data and run the containers?",
        "only this user (and root) can read the database and the files",
        &[
            ("Create a new system user", true),
            ("Use an existing user", false),
        ],
    )?;
    if create {
        let name = text(
            "Name of the new user:",
            defaults::SERVICE_USER,
            |name: &str| valid_if(is_user_name(name), "lowercase letters, digits, _ and -"),
        )?;
        return Ok(match system::find_user(&name)? {
            Some(owner) => {
                println!("{name} exists already and will be used.");
                ServiceUser::Existing(owner)
            }
            None => ServiceUser::Create { name },
        });
    }
    let name = text("Existing user:", defaults::SERVICE_USER, |name: &str| {
        valid_if(
            system::find_user(name).ok().flatten().is_some(),
            "no such user",
        )
    })?;
    let owner = system::find_user(&name)?.with_context(|| format!("no such user: {name}"))?;
    Ok(ServiceUser::Existing(owner))
}

fn ask_secret(
    what: &str,
    generate: fn() -> String,
    check: impl Fn(&str) -> Check + Clone + 'static,
) -> Result<Secret> {
    let generated = choose(
        &format!("{what}:"),
        "a generated one is strong and you never need to type it",
        &[("Generate a strong one", true), ("Enter my own", false)],
    )?;
    if generated {
        return Ok(Secret {
            value: generate(),
            generated: true,
        });
    }
    let value = Password::new(&format!("{what}:"))
        .with_display_mode(PasswordDisplayMode::Masked)
        .with_validator(check)
        .with_custom_confirmation_message("Again:")
        .prompt()
        .context("reading the secret")?;
    Ok(Secret {
        value,
        generated: false,
    })
}

fn ask_proxy() -> Result<Proxy> {
    let choice = choose(
        "Will a reverse proxy (nginx, Caddy, Traefik, ...) sit in front of the server?",
        "behind a proxy the server trusts X-Forwarded-For and listens on 127.0.0.1 only",
        &[
            ("No, clients connect to the server directly", 0),
            ("Yes, nginx: write an example site config", 1),
            ("Yes, another proxy", 2),
        ],
    )?;
    Ok(match choice {
        0 => Proxy::None,
        1 => Proxy::Nginx {
            server_name: text("Server name (domain) for nginx:", "", |name: &str| {
                valid_if(
                    is_host_name(name),
                    "a domain name such as files.example.com",
                )
            })?,
        },
        _ => Proxy::Other,
    })
}

fn ask_public_url(proxy: &Proxy, port: u16, https_only: bool) -> Result<String> {
    let default = match proxy {
        Proxy::Nginx { server_name } => format!("https://{server_name}"),
        Proxy::None | Proxy::Other => format!("http://localhost:{port}"),
    };
    let url = text(
        "Public URL (the address people open):",
        &default,
        move |url: &str| {
            let url = url.trim();
            if https_only && !auth::is_https_or_loopback(url) {
                return valid_if(false, "https://...");
            }
            valid_if(
                url.strip_prefix("https://")
                    .or_else(|| url.strip_prefix("http://"))
                    .is_some_and(|rest| !rest.is_empty())
                    && env_safe(url),
                "http://... or https://...",
            )
        },
    )?;
    Ok(url.trim_end_matches('/').to_owned())
}

fn ask_auth() -> Result<Auth> {
    let choice = choose(
        "How should people sign in?",
        "single sign-on uses an OpenID Connect provider such as Keycloak, Authentik or Auth0",
        &[
            ("Usernames and passwords", 0),
            ("Single sign-on (OIDC) only", 1),
            ("Single sign-on and passwords", 2),
        ],
    )?;
    if choice == 0 {
        return Ok(Auth::Passwords);
    }
    let issuer = text("OIDC issuer URL:", "", |url: &str| {
        valid_if(
            auth::is_https_or_loopback(url.trim()) && env_safe(url),
            "https://...",
        )
    })?;
    let client_id = text("OIDC client id:", "", |id: &str| {
        valid_if(!id.trim().is_empty() && env_safe(id), "required, without '")
    })?;
    let client_secret = Password::new("OIDC client secret (empty for a public client):")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation()
        .with_validator(|secret: &str| valid_if(env_safe(secret), "no ' or line breaks"))
        .prompt()
        .context("reading the secret")?;
    let name = text(
        "Name on the sign-in button:",
        defaults::OIDC_NAME,
        |name: &str| {
            valid_if(
                !name.trim().is_empty() && env_safe(name),
                "required, without '",
            )
        },
    )?;
    let oidc = Oidc {
        issuer: issuer.trim_end_matches('/').to_owned(),
        client_id,
        client_secret: Some(client_secret).filter(|secret| !secret.is_empty()),
        name,
    };
    Ok(if choice == 1 {
        Auth::Oidc(oidc)
    } else {
        Auth::OidcAndPasswords(oidc)
    })
}

/// Prints what will be done, secrets masked, and asks to go ahead.
pub fn confirm_plan(plan: &InstallPlan) -> Result<bool> {
    let secret = |secret: &Secret| {
        if secret.generated {
            "generated"
        } else {
            "entered"
        }
    };
    println!("\nSummary");
    println!("  Engine:          {}", plan.runtime.engine.program());
    println!("  Directory:       {}", plan.dir.display());
    println!(
        "  Secrets:         {}",
        match plan.secrets {
            SecretStore::Files => "secret files in secrets/",
            SecretStore::EnvFile => ".env",
        }
    );
    match &plan.storage {
        Storage::Volumes => println!("  Storage:         container volumes"),
        Storage::HostDirs { db, files, user } => {
            println!("  Database dir:    {}", db.display());
            println!("  Files dir:       {}", files.display());
            let new = if matches!(user, ServiceUser::Create { .. }) {
                " (new)"
            } else {
                ""
            };
            println!("  Runs as:         {}{new}", user.name());
        }
    }
    println!("  Database user:   {}", plan.db_user);
    println!("  DB password:     {}", secret(&plan.db_password));
    println!("  JWT secret:      {}", secret(&plan.jwt_secret));
    println!("  Public URL:      {}", plan.public_url);
    println!("  Published on:    {}:{}", plan.listen, plan.port);
    println!(
        "  Reverse proxy:   {}",
        match &plan.proxy {
            Proxy::None => "none".to_owned(),
            Proxy::Nginx { server_name } => format!("nginx ({server_name})"),
            Proxy::Other => "yes".to_owned(),
        }
    );
    println!(
        "  Sign-in:         {}",
        match &plan.auth {
            Auth::Passwords => "passwords".to_owned(),
            Auth::Oidc(oidc) => format!("single sign-on ({})", oidc.issuer),
            Auth::OidcAndPasswords(oidc) =>
                format!("single sign-on ({}) and passwords", oidc.issuer),
        }
    );
    println!();
    confirm("Install Home Space with these settings?", true)
}

/// At most `max_len` lowercase letters, digits, `_` and the `extra` characters, starting with a
/// letter or `_` (so never with `-`).
fn is_plain_name(name: &str, max_len: usize, extra: &[char]) -> bool {
    let mut chars = name.chars();
    name.len() <= max_len
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || extra.contains(&c))
}

/// A Postgres role name that needs no quoting.
fn is_db_identifier(name: &str) -> bool {
    is_plain_name(name, 63, &[])
}

/// A portable Linux user name.
fn is_user_name(name: &str) -> bool {
    is_plain_name(name, 32, &['-'])
}

fn is_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Whether `url` points at this machine (`localhost`, `127.0.0.1` or `[::1]`).
fn is_loopback_url(url: &str) -> bool {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .is_some_and(|rest| auth::is_https_or_loopback(&format!("http://{rest}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert!(is_db_identifier("homespace"));
        assert!(is_db_identifier("_hs2"));
        assert!(!is_db_identifier("Home"));
        assert!(!is_db_identifier("2hs"));
        assert!(is_user_name("home-space"));
        assert!(!is_user_name("home space"));
        assert!(!is_user_name("-home"));
        assert!(is_host_name("files.example.com"));
        assert!(!is_host_name("files..example.com"));
        assert!(!is_host_name("https://files.example.com"));
    }

    #[test]
    fn loopback_urls_are_recognised() {
        assert!(is_loopback_url("http://localhost:8080"));
        assert!(is_loopback_url("https://127.0.0.1"));
        assert!(!is_loopback_url("http://192.168.1.5:8080"));
        assert!(!is_loopback_url("https://files.example.com"));
    }
}
