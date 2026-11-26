//! `homespace-ctl secrets rotate`: replacing the token signing secret and the secrets key, the
//! only way to change them without losing what the secrets key seals.
//!
//! Both are always replaced together: after a breach nobody can tell which one leaked. Every
//! user is signed out, on every device. Where this program can find the secrets and stop the
//! server, in an installation from `homespace-ctl install`, it does it all; anywhere else it
//! guides the administrator through it, who keeps the secrets wherever they like.

#[cfg(target_os = "linux")]
mod installed;
mod wizard;

use std::{io::BufRead as _, path::PathBuf};

use anyhow::{Context, Result};

use auth::{
    SecretBox,
    secrets_key::{self, KeyCheck, Rotation, SecretsKeyError},
};
use db::{Db, TenantId};

use crate::{Database, cli::SecretsCommand, tenant};

pub const JWT_SECRET: &str = "HOMESPACE_JWT_SECRET";
pub const SECRETS_KEY: &str = "HOMESPACE_SECRETS_KEY";
/// What earlier versions kept a rotated-out secrets key in; still read here so that what is
/// sealed with it can be sealed again once.
const SECRETS_KEY_PREVIOUS: &str = "HOMESPACE_SECRETS_KEY_PREVIOUS";

pub async fn run(command: SecretsCommand, database: Database) -> Result<()> {
    match command {
        SecretsCommand::Rotate { tenant, .. } if database.is_given() => {
            let db = database.connect().await?;
            let tenant = tenant::resolve(&db, tenant.tenant.as_deref()).await?;
            wizard::run(&db, tenant).await
        }
        SecretsCommand::Rotate { yes, tenant } => rotate_installation(yes, tenant.tenant),
        SecretsCommand::Reseal { tenant } => {
            let db = database.connect().await?;
            let tenant = tenant::resolve(&db, tenant.tenant.as_deref()).await?;
            reseal_from_stdin(&db, tenant).await
        }
    }
}

#[cfg(target_os = "linux")]
fn rotate_installation(yes: bool, tenant: Option<String>) -> Result<()> {
    installed::run(yes, tenant.as_deref())
}

#[cfg(not(target_os = "linux"))]
fn rotate_installation(_: bool, _: Option<String>) -> Result<()> {
    anyhow::bail!(
        "no database given: pass --database-url or --env-file for the guided rotation, or run \
         this on the Linux machine `homespace-ctl install` set the server up on"
    )
}

/// The name of `tenant`'s variable `name`, as the server reads it: tenant 1's unsuffixed,
/// tenant 2's `NAME_2`.
pub fn var(tenant: TenantId, name: &str) -> String {
    if tenant == TenantId::DEFAULT {
        name.to_owned()
    } else {
        format!("{name}_{tenant}")
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// `tenant`'s secrets key as the environment gives it, from its variable or its `_FILE`.
fn configured_key(tenant: TenantId) -> Result<Option<String>> {
    let (name, file) = (
        var(tenant, SECRETS_KEY),
        format!("{}_FILE", var(tenant, SECRETS_KEY)),
    );
    Ok(auth::secrets::value_or_file(
        &name,
        env(&name).as_deref(),
        &file,
        env(&file).map(PathBuf::from).as_deref(),
    )?)
}

/// What [`reseal`] did.
enum Resealed {
    Done(Rotation),
    /// The tenant has no secrets key and nothing sealed: only everyone was signed out.
    SignedOut(u64),
}

/// Seals everything of `tenant` again with `new` and signs everyone out, in one transaction.
/// What is sealed now opens with `current` or, once, the old `_PREVIOUS` key. Without `new`
/// the tenant has no secrets key, and only everyone is signed out.
async fn reseal(
    db: &Db,
    tenant: TenantId,
    current: Option<&str>,
    new: Option<&str>,
) -> Result<Resealed> {
    let current: Vec<SecretBox> = [
        current.map(str::to_owned),
        env(&var(tenant, SECRETS_KEY_PREVIOUS)),
    ]
    .into_iter()
    .flatten()
    .map(|key| SecretBox::new(&key))
    .collect();
    let Some(new) = new else {
        match secrets_key::check(db, tenant, None).await {
            Ok(KeyCheck::Unused) => {}
            Ok(_) | Err(SecretsKeyError::Missing) => anyhow::bail!(
                "authenticator apps or password folders are sealed with a secrets key, and no new \
                 one was given; nothing was changed"
            ),
            Err(err) => return Err(err).context("checking the secrets key"),
        }
        let ended = db
            .end_tenant_sessions(tenant)
            .await
            .context("signing everyone out")?;
        return Ok(Resealed::SignedOut(ended));
    };
    let key_var = var(tenant, SECRETS_KEY);
    match secrets_key::rotate(db, tenant, &current, &SecretBox::new(new)).await {
        Ok(done) => Ok(Resealed::Done(done)),
        Err(SecretsKeyError::Missing) => anyhow::bail!(
            "{key_var} is not set, but the secrets are sealed with a key: give the key they are \
             sealed with. Nothing was changed"
        ),
        Err(SecretsKeyError::Wrong) => anyhow::bail!(
            "{key_var} is not the key the secrets are sealed with. Nothing was changed"
        ),
        Err(SecretsKeyError::Unopened(what)) => anyhow::bail!(
            "{what} opens neither with {key_var} nor with {}. Nothing was changed",
            var(tenant, SECRETS_KEY_PREVIOUS)
        ),
        Err(err) => Err(err).context("sealing the secrets again"),
    }
}

fn report(done: &Resealed) {
    match done {
        Resealed::Done(Rotation::Rotated {
            authenticator_apps,
            password_folders,
            sessions_ended,
        }) => println!(
            "Sealed again with the new key: {authenticator_apps} authenticator app(s) and \
             {password_folders} password folder key(s). Ended {sessions_ended} session(s): \
             everyone signs in again."
        ),
        Resealed::Done(Rotation::AlreadyDone) => println!(
            "The secrets are sealed with the new key already: an earlier run got this far."
        ),
        Resealed::SignedOut(ended) => println!(
            "No secrets key is set, so nothing is sealed. Ended {ended} session(s): everyone \
             signs in again."
        ),
    }
}

/// `reseal` as `rotate` runs it in the app container: the current key from the container's
/// settings, the new one from the first line of standard input, empty when there is none.
async fn reseal_from_stdin(db: &Db, tenant: TenantId) -> Result<()> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading the new secrets key from standard input")?;
    let new = line.trim_end_matches(['\r', '\n']);
    let new = (!new.is_empty()).then_some(new);
    if let Some(new) = new {
        auth::secrets::check_secret("the new secrets key", new)?;
    }
    let current = configured_key(tenant)?;
    let done = reseal(db, tenant, current.as_deref(), new).await?;
    report(&done);
    Ok(())
}
