//! Creating the first administrator once the new server runs.

use std::{
    io::Write as _,
    process::{Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use inquire::{Confirm, Password, PasswordDisplayMode, Text, validator::Validation};

use super::plan::Auth;
use crate::installation::Installation;

/// How long the server gets to start and migrate the database.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const READY_POLL: Duration = Duration::from_secs(2);
const ATTEMPTS: usize = 3;

/// Offers to create the first administrator, or says how to make one with single sign-on.
pub fn offer(installation: &Installation, auth: &Auth) -> Result<()> {
    if !auth.password_login() {
        println!(
            "\nSign in once through single sign-on, then make yourself an administrator:\n  \
             homespace-ctl user admin <username>"
        );
        return Ok(());
    }
    let create = Confirm::new("Create the first administrator now?")
        .with_default(true)
        .prompt()
        .context("reading the answer")?;
    if !create {
        println!("Create one later with: homespace-ctl user add --admin <username>");
        return Ok(());
    }
    println!("Waiting for the server to start...");
    if let Err(error) = wait_until_ready(installation)? {
        println!(
            "The server did not get ready in time ({error}). Check `compose logs app` in {}, \
             then run: homespace-ctl user add --admin <username>",
            installation.dir.display()
        );
        return Ok(());
    }
    for _ in 0..ATTEMPTS {
        let username = Text::new("Administrator username:")
            .with_default("admin")
            .with_validator(|name: &str| {
                Ok(match protocol::rules::username_problem(name.trim()) {
                    None => Validation::Valid,
                    Some(problem) => Validation::Invalid(problem.into()),
                })
            })
            .prompt()
            .context("reading the username")?;
        let password = Password::new("Password:")
            .with_display_mode(PasswordDisplayMode::Masked)
            .with_custom_confirmation_message("Again:")
            .prompt()
            .context("reading the password")?;
        match add_admin(installation, username.trim(), &password)? {
            None => return Ok(()),
            Some(error) => println!("{error}"),
        }
    }
    println!("Create one later with: homespace-ctl user add --admin <username>");
    Ok(())
}

/// Waits until `homespace-ctl user list` works inside the app container, so the database is
/// migrated; `Err` holds the last error when it never does in time.
fn wait_until_ready(installation: &Installation) -> Result<Result<(), String>> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let out = installation
            .exec_ctl(["user", "list"], false)
            .stdin(Stdio::null())
            .output()
            .context("running compose exec")?;
        if out.status.success() {
            return Ok(Ok(()));
        }
        if Instant::now() >= deadline {
            return Ok(Err(stderr(&out)));
        }
        thread::sleep(READY_POLL);
    }
}

/// Creates the administrator; `Some(error)` when the server refused (say a weak password).
fn add_admin(
    installation: &Installation,
    username: &str,
    password: &str,
) -> Result<Option<String>> {
    let mut child = installation
        .exec_ctl(
            ["user", "add", "--admin", "--password-stdin", "--", username],
            false,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running compose exec")?;
    child
        .stdin
        .take()
        .context("no standard input")?
        .write_all(format!("{password}\n").as_bytes())
        .context("passing the password")?;
    let out = child.wait_with_output().context("running compose exec")?;
    if out.status.success() {
        print!("{}", String::from_utf8_lossy(&out.stdout));
        Ok(None)
    } else {
        Ok(Some(stderr(&out)))
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_owned()
}
