//! The rotation of a server from `homespace-ctl install`, done for the administrator: the new
//! values are written next to the old ones first, the app is stopped, everything is sealed
//! again inside its container, and only then do the new values replace the old ones and the
//! app start again.
//!
//! A new secrets key is on disk before anything is sealed with it, and the database records
//! which key its secrets are sealed with, so a run cut short at any point is finished by
//! running it again: the values waiting in `.new` files are used, and a database sealed with
//! them already is left as it is.

use std::{
    fs,
    io::{ErrorKind, IsTerminal as _, Write as _},
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result};
use inquire::Confirm;

use super::{JWT_SECRET, SECRETS_KEY};
use crate::{
    generate,
    install::system::write_own_file,
    installation::{self, Installation},
};

const SECRETS_DIR: &str = "secrets";
const ENV_FILE: &str = ".env";
const JWT_FILE: &str = "jwt_secret";
const KEY_FILE: &str = "secrets_key";
/// Appended to a file's name for the new value waiting to replace it.
const PENDING: &str = ".new";
const APP_SERVICE: &str = "app";

pub fn run(yes: bool, tenant: Option<&str>) -> Result<()> {
    let path = installation::config_path()?;
    let installation = Installation::load(&path)?.with_context(|| {
        format!(
            "DATABASE_URL is not set, and this user has no installation from `homespace-ctl \
             install` ({} is missing). For a server set up otherwise, pass --database-url or \
             --env-file for the guided rotation",
            path.display()
        )
    })?;
    let store = Store::find(&installation.dir)?;
    let pending = store.pending()?;
    if pending.jwt.is_some() || pending.key.is_some() {
        println!("Finishing an earlier rotation with the new values it left in place.");
    }
    let jwt = pending.jwt.unwrap_or_else(generate::jwt_secret);
    let key = store
        .has_secrets_key()?
        .then(|| pending.key.unwrap_or_else(generate::secrets_key));
    if key.is_none() {
        println!(
            "This server has no secrets key, so nothing is sealed: only the token signing secret \
             is rotated."
        );
    }
    if !yes && !confirm()? {
        println!("Nothing was changed.");
        return Ok(());
    }

    println!("• Writing the new values next to the old ones");
    store.write_pending(&jwt, key.as_deref())?;
    println!("• Stopping the server");
    compose(&installation, &["stop", APP_SERVICE])?;
    println!("• Sealing everything again with the new key and signing everyone out");
    if let Err(err) = reseal(&installation, tenant, key.as_deref()) {
        // Whatever the database holds now, the server can start: with the old key when nothing
        // was sealed again, and refusing to when it was, as it then needs the new key.
        let restarted = compose(&installation, &["start", APP_SERVICE]).is_ok();
        let state = if restarted {
            "The server was started again with the old values"
        } else {
            "The server could not be started again"
        };
        return Err(err.context(format!(
            "{state}. The new values wait in {} next to the old ones: fix the cause and run \
             `homespace-ctl secrets rotate` again to finish. Don't remove them",
            store.describe_pending()
        )));
    }
    println!("• Replacing the old values");
    store.commit_pending()?;
    println!("• Starting the server");
    compose(
        &installation,
        &["up", "-d", "--no-deps", "--force-recreate", APP_SERVICE],
    )?;
    println!("\nDone: the old token signing secret and secrets key open nothing any more.");
    Ok(())
}

fn confirm() -> Result<bool> {
    anyhow::ensure!(
        std::io::stdin().is_terminal(),
        "rotating signs every user out and restarts the server: pass --yes to go ahead without \
         a terminal"
    );
    Confirm::new(
        "This replaces the token signing secret and the secrets key, signs every user out on \
         every device and restarts the server. Go ahead?",
    )
    .with_default(false)
    .prompt()
    .context("reading the answer")
}

/// `<engine> compose <args>` in the installation's directory, which must succeed.
fn compose(installation: &Installation, args: &[&str]) -> Result<()> {
    let shown = format!(
        "{} compose {}",
        installation.engine.program(),
        args.join(" ")
    );
    let status = installation
        .compose(args)
        .status()
        .with_context(|| format!("running {shown}"))?;
    anyhow::ensure!(status.success(), "{shown} failed ({status})");
    Ok(())
}

/// Runs `homespace-ctl secrets reseal` in a one-off app container, which has the current
/// settings, while the app is stopped. The new key goes in on standard input, never on a
/// command line.
fn reseal(installation: &Installation, tenant: Option<&str>, key: Option<&str>) -> Result<()> {
    let mut args = vec![
        "run",
        "--rm",
        "--no-deps",
        "-T",
        "--entrypoint",
        "homespace-ctl",
        APP_SERVICE,
        "secrets",
        "reseal",
    ];
    if let Some(tenant) = tenant {
        args.extend(["--tenant", tenant]);
    }
    let mut command = installation.compose(args);
    command.stdin(Stdio::piped());
    let mut child = command.spawn().context("running compose run")?;
    let mut stdin = child
        .stdin
        .take()
        .context("opening the container's input")?;
    writeln!(stdin, "{}", key.unwrap_or_default()).context("handing over the new key")?;
    drop(stdin);
    let status = child.wait().context("running compose run")?;
    anyhow::ensure!(
        status.success(),
        "sealing the secrets again failed ({status})"
    );
    Ok(())
}

/// Where the installation keeps its secrets, as `homespace-ctl install` wrote them.
#[derive(Debug, PartialEq, Eq)]
enum Store {
    /// One file per secret in `secrets/`, mounted into the containers.
    Files(PathBuf),
    /// In `.env`, which compose passes to the containers.
    EnvFile(PathBuf),
}

/// New values left by a run that did not finish.
#[derive(Debug, Default, PartialEq, Eq)]
struct Pending {
    jwt: Option<String>,
    key: Option<String>,
}

impl Store {
    fn find(dir: &Path) -> Result<Self> {
        let secrets = dir.join(SECRETS_DIR);
        if secrets.join(JWT_FILE).is_file() {
            return Ok(Self::Files(secrets));
        }
        let env = dir.join(ENV_FILE);
        if read_optional(&env)?.is_some_and(|text| env_value(&text, JWT_SECRET).is_some()) {
            return Ok(Self::EnvFile(env));
        }
        anyhow::bail!(
            "found no token signing secret in {} or {}, so this cannot rotate it for you. Pass \
             --database-url or --env-file for the guided rotation",
            secrets.join(JWT_FILE).display(),
            env.display()
        )
    }

    fn has_secrets_key(&self) -> Result<bool> {
        Ok(match self {
            Self::Files(dir) => dir.join(KEY_FILE).is_file(),
            Self::EnvFile(path) => {
                read_optional(path)?.is_some_and(|text| env_value(&text, SECRETS_KEY).is_some())
            }
        })
    }

    fn pending(&self) -> Result<Pending> {
        let pending = match self {
            Self::Files(dir) => Pending {
                jwt: read_optional(&pending_path(&dir.join(JWT_FILE)))?.map(trimmed),
                key: read_optional(&pending_path(&dir.join(KEY_FILE)))?.map(trimmed),
            },
            Self::EnvFile(path) => match read_optional(&pending_path(path))? {
                Some(text) => Pending {
                    jwt: env_value(&text, JWT_SECRET),
                    key: env_value(&text, SECRETS_KEY),
                },
                None => Pending::default(),
            },
        };
        for value in [&pending.jwt, &pending.key].into_iter().flatten() {
            auth::secrets::check_secret("a new value left by an earlier rotation", value)?;
        }
        Ok(pending)
    }

    fn write_pending(&self, jwt: &str, key: Option<&str>) -> Result<()> {
        match self {
            Self::Files(dir) => {
                // Mode 444 as `install` writes them: any container user reads them, and the
                // directory (mode 700) keeps everyone else out.
                write_own_file(&pending_path(&dir.join(JWT_FILE)), jwt, 0o444)?;
                if let Some(key) = key {
                    write_own_file(&pending_path(&dir.join(KEY_FILE)), key, 0o444)?;
                }
            }
            Self::EnvFile(path) => {
                let text = fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let mut text = replace_env_value(&text, JWT_SECRET, jwt);
                if let Some(key) = key {
                    text = replace_env_value(&text, SECRETS_KEY, key);
                }
                write_own_file(&pending_path(path), &text, 0o600)?;
            }
        }
        Ok(())
    }

    /// Moves the new values over the old ones; each move replaces a file at once.
    fn commit_pending(&self) -> Result<()> {
        let paths = match self {
            Self::Files(dir) => vec![dir.join(KEY_FILE), dir.join(JWT_FILE)],
            Self::EnvFile(path) => vec![path.clone()],
        };
        for path in paths {
            let pending = pending_path(&path);
            match fs::rename(&pending, &path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotFound && !pending.exists() => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("replacing {}", path.display()));
                }
            }
        }
        Ok(())
    }

    fn describe_pending(&self) -> String {
        match self {
            Self::Files(dir) => format!("{}/*{PENDING}", dir.display()),
            Self::EnvFile(path) => pending_path(path).display().to_string(),
        }
    }
}

fn pending_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(PENDING);
    PathBuf::from(name)
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

fn trimmed(text: String) -> String {
    text.trim().to_owned()
}

/// The value of `key` in `.env` text, without the quotes `install` writes; `None` when it
/// is missing or empty.
fn env_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .map(|value| unquote(value.trim()).to_owned())
        .rfind(|value| !value.is_empty())
}

fn unquote(value: &str) -> &str {
    ['\'', '"']
        .into_iter()
        .find_map(|quote| value.strip_prefix(quote)?.strip_suffix(quote))
        .unwrap_or(value)
}

/// `.env` text with every line of `key` set to `value`, single-quoted as `install` writes it.
fn replace_env_value(text: &str, key: &str, value: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let is_key = line
            .strip_prefix(key)
            .is_some_and(|rest| rest.starts_with('='));
        if is_key {
            out.push_str(&format!("{key}='{value}'"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const JWT: &str = "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK";
    const KEY: &str = "Xw7pR2nLq9TzVb4KsYc1MdHg6JfAe3Uo8NiZ5tQrWmEx";

    const ENV: &str = "# written by install\nHOMESPACE_PORT='8080'\n\
        HOMESPACE_JWT_SECRET='old-jwt'\nHOMESPACE_JWT_SECRET_FILE=''\n\
        HOMESPACE_SECRETS_KEY='old-key'\n";

    #[test]
    fn env_values_are_read_without_their_quotes() {
        assert_eq!(env_value(ENV, JWT_SECRET).as_deref(), Some("old-jwt"));
        assert_eq!(env_value(ENV, SECRETS_KEY).as_deref(), Some("old-key"));
        assert_eq!(env_value(ENV, "HOMESPACE_JWT_SECRET_FILE"), None);
        assert_eq!(env_value("A=\"x\"\nB=y\n", "A").as_deref(), Some("x"));
        assert_eq!(env_value("A=\"x\"\nB=y\n", "B").as_deref(), Some("y"));
    }

    #[test]
    fn only_the_rotated_lines_change() {
        let text = replace_env_value(ENV, JWT_SECRET, "new-jwt");
        assert_eq!(
            text,
            ENV.replace(
                "HOMESPACE_JWT_SECRET='old-jwt'",
                "HOMESPACE_JWT_SECRET='new-jwt'"
            )
        );
    }

    fn installation(dir: &Path, files: bool) -> Store {
        if files {
            let secrets = dir.join(SECRETS_DIR);
            fs::create_dir(&secrets).expect("dir");
            fs::write(secrets.join(JWT_FILE), "old-jwt").expect("jwt");
            fs::write(secrets.join(KEY_FILE), "old-key").expect("key");
        } else {
            fs::write(dir.join(ENV_FILE), ENV).expect("env");
        }
        Store::find(dir).expect("store")
    }

    #[test]
    fn new_values_wait_beside_the_old_ones_until_committed() {
        for files in [true, false] {
            let dir = tempfile::tempdir().expect("tempdir");
            let store = installation(dir.path(), files);
            assert!(store.has_secrets_key().expect("read"));
            assert_eq!(store.pending().expect("pending"), Pending::default());

            store.write_pending(JWT, Some(KEY)).expect("write");
            let pending = store.pending().expect("pending");
            assert_eq!(pending.jwt.as_deref(), Some(JWT), "{store:?}");
            assert_eq!(pending.key.as_deref(), Some(KEY), "{store:?}");

            store.commit_pending().expect("commit");
            assert_eq!(store.pending().expect("pending"), Pending::default());
            match &store {
                Store::Files(secrets) => {
                    assert_eq!(
                        fs::read_to_string(secrets.join(JWT_FILE)).expect("jwt"),
                        JWT
                    );
                    assert_eq!(
                        fs::read_to_string(secrets.join(KEY_FILE)).expect("key"),
                        KEY
                    );
                }
                Store::EnvFile(path) => {
                    let text = fs::read_to_string(path).expect("env");
                    assert_eq!(env_value(&text, JWT_SECRET).as_deref(), Some(JWT));
                    assert_eq!(env_value(&text, SECRETS_KEY).as_deref(), Some(KEY));
                    assert!(text.contains("HOMESPACE_PORT='8080'"));
                }
            }
            // Committing again finds nothing to move.
            store.commit_pending().expect("commit again");
        }
    }

    #[test]
    fn an_installation_without_a_secrets_key_rotates_the_signing_secret_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join(ENV_FILE),
            "HOMESPACE_JWT_SECRET='old-jwt'\n",
        )
        .expect("env");
        let store = Store::find(dir.path()).expect("store");
        assert!(!store.has_secrets_key().expect("read"));
        store.write_pending(JWT, None).expect("write");
        assert_eq!(
            store.pending().expect("pending"),
            Pending {
                jwt: Some(JWT.to_owned()),
                key: None
            }
        );
    }

    #[test]
    fn secrets_that_cannot_be_found_are_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = Store::find(dir.path()).unwrap_err().to_string();
        assert!(err.contains("guided rotation"), "{err}");
    }
}
