//! The guided rotation, for a server this program cannot stop or whose secrets it cannot
//! write, such as one that reads them from a secret manager when it starts: the administrator
//! stops the server and stores the new values; this seals everything again and signs everyone
//! out.

use std::io::IsTerminal as _;

use anyhow::{Context, Result};
use inquire::{Confirm, Password, PasswordDisplayMode};

use auth::{
    SecretBox,
    secrets_key::{self, SecretsKeyError},
};
use db::{Db, TenantId};

use super::{JWT_SECRET, SECRETS_KEY, configured_key, report, reseal, var};
use crate::generate;

/// How often the new key may be typed back wrong before giving up.
const ATTEMPTS: usize = 3;

pub async fn run(db: &Db, tenant: TenantId) -> Result<()> {
    anyhow::ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "the guided rotation asks questions: run it in a terminal"
    );
    let (jwt_var, key_var) = (var(tenant, JWT_SECRET), var(tenant, SECRETS_KEY));
    println!(
        "Rotating {jwt_var} and {key_var} of tenant {tenant}.\n\
         Both get new values; everything sealed with the secrets key is sealed again with the new\n\
         one, and every user is signed out on every device. Changing either by hand instead\n\
         would leave authenticator apps and password folders unreadable.\n"
    );
    if !ask("Is the Home Space server stopped? It must stay stopped until this is done.")? {
        println!("Stop it first, then run this again. Nothing was changed.");
        return Ok(());
    }
    let current = match configured_key(tenant)? {
        Some(key) => {
            println!("Using the current {key_var} from the environment.");
            Some(key)
        }
        None => {
            let key = Password::new(&format!(
                "Current {key_var} (empty if the server has none):"
            ))
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()
            .context("reading the current secrets key")?;
            (!key.is_empty()).then_some(key)
        }
    };
    check_current(db, tenant, current.as_deref(), &key_var).await?;

    let jwt = generate::jwt_secret();
    let key = generate::secrets_key();
    println!(
        "\nThe new values. Store each where the server reads it, in place of the old one (or in\n\
         the file its _FILE variable names), and don't start the server yet:\n\n  \
         {jwt_var}={jwt}\n  {key_var}={key}\n"
    );
    confirm_stored(&key, &key_var)?;
    if !ask("Seal everything again with the new key and sign everyone out?")? {
        println!(
            "Nothing was changed: put the old values back before starting the server, or run this \
             again."
        );
        return Ok(());
    }
    let done = reseal(db, tenant, current.as_deref(), Some(&key)).await?;
    report(&done);
    println!(
        "\nStart the server now, with the new values. Remove any HOMESPACE_JWT_SECRET_PREVIOUS or\n\
         HOMESPACE_SECRETS_KEY_PREVIOUS: the server no longer starts with them."
    );
    Ok(())
}

/// Refuses a current key that is not the one the secrets are sealed with, before any new
/// value is handed out. With an old `_PREVIOUS` key set, the rotation itself checks.
async fn check_current(
    db: &Db,
    tenant: TenantId,
    current: Option<&str>,
    key_var: &str,
) -> Result<()> {
    if super::env(&var(tenant, super::SECRETS_KEY_PREVIOUS)).is_some() {
        return Ok(());
    }
    let current = current.map(SecretBox::new);
    match secrets_key::check(db, tenant, current.as_ref()).await {
        Ok(_) => Ok(()),
        Err(SecretsKeyError::Missing) => anyhow::bail!(
            "authenticator apps or password folders are sealed with a secrets key: give the \
             current {key_var}. Nothing was changed"
        ),
        Err(SecretsKeyError::Wrong) => anyhow::bail!(
            "that is not the {key_var} the secrets are sealed with. Nothing was changed"
        ),
        Err(err) => Err(err).context("checking the secrets key"),
    }
}

/// Has the new key typed back as it was stored, so a key copied wrong is caught while the old
/// one still opens everything.
fn confirm_stored(key: &str, key_var: &str) -> Result<()> {
    for _ in 0..ATTEMPTS {
        let typed = Password::new(&format!("The new {key_var} as you stored it:"))
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()
            .context("reading the new secrets key")?;
        if typed.trim() == key {
            return Ok(());
        }
        println!("That is not the new {key_var}. Check what you stored.");
    }
    anyhow::bail!(
        "the new {key_var} was not confirmed. Nothing was changed: put the old values back \
         before starting the server"
    )
}

fn ask(question: &str) -> Result<bool> {
    Confirm::new(question)
        .with_default(false)
        .prompt()
        .context("reading the answer")
}
