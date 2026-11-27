//! `homespace-ctl user`: managing the users of a tenant.

use anyhow::{Context, Result};

use db::{Db, PasswordSet, TenantId, User};

use crate::{
    cli::{TenantArg, UserCommand},
    tenant,
};

pub async fn run(command: UserCommand, db: &Db) -> Result<()> {
    match command {
        UserCommand::Add {
            username,
            admin,
            password_stdin,
            tenant,
        } => {
            let tenant = resolve(db, &tenant).await?;
            add(db, tenant, &username, admin, password_stdin).await
        }
        UserCommand::List { tenant } => list(db, resolve(db, &tenant).await?).await,
        UserCommand::Admin {
            username,
            revoke,
            tenant,
        } => set_admin(db, resolve(db, &tenant).await?, &username, !revoke).await,
        UserCommand::Passwd {
            username,
            password_stdin,
            tenant,
        } => {
            let tenant = resolve(db, &tenant).await?;
            set_password(db, tenant, &username, password_stdin).await
        }
        UserCommand::Reset2fa { username, tenant } => {
            reset_second_factors(db, resolve(db, &tenant).await?, &username).await
        }
    }
}

async fn resolve(db: &Db, arg: &TenantArg) -> Result<TenantId> {
    tenant::resolve(db, arg.tenant.as_deref()).await
}

async fn add(
    db: &Db,
    tenant: TenantId,
    username: &str,
    admin: bool,
    password_stdin: bool,
) -> Result<()> {
    let password = read_new_password(password_stdin)?;
    let user = auth::offline::create_user(db, tenant, username, &password)
        .await
        .context("creating user")?;
    if admin {
        db.set_admin(user.id, true)
            .await
            .context("making the user an administrator")?;
    }
    let role = if admin { " as an administrator" } else { "" };
    println!("created user {} ({}){role}", user.username, user.id);
    Ok(())
}

async fn list(db: &Db, tenant: TenantId) -> Result<()> {
    let users = db.list_users(tenant).await.context("listing users")?;
    for user in users {
        let role = if user.is_admin { "\tadmin" } else { "" };
        let state = match (user.disabled_at, user.delete_at) {
            (_, Some(at)) => format!("\tdeleted at {at}"),
            (Some(_), None) => "\tdisabled".to_owned(),
            (None, None) => String::new(),
        };
        println!("{}\t{}{role}{state}", user.id, user.username);
    }
    Ok(())
}

async fn set_admin(db: &Db, tenant: TenantId, username: &str, is_admin: bool) -> Result<()> {
    let user = find(db, tenant, username).await?;
    db.set_admin(user.id, is_admin)
        .await
        .context("changing administrator rights")?;
    if is_admin {
        println!("{username} is now an administrator");
    } else {
        println!("{username} is no longer an administrator");
    }
    Ok(())
}

async fn set_password(
    db: &Db,
    tenant: TenantId,
    username: &str,
    password_stdin: bool,
) -> Result<()> {
    let user = find(db, tenant, username).await?;
    let password = read_new_password(password_stdin)?;
    let set = auth::offline::set_password(db, user.id, &password)
        .await
        .context("changing password")?;
    match set {
        PasswordSet::Replaced => println!("password updated for {username}"),
        PasswordSet::MadeLocal => println!(
            "{username} is now a local account with this password; the identity provider \
             signs that person in as a new user from now on"
        ),
    }
    Ok(())
}

async fn reset_second_factors(db: &Db, tenant: TenantId, username: &str) -> Result<()> {
    let user = find(db, tenant, username).await?;
    db.reset_second_factors(tenant, user.id)
        .await
        .context("resetting two-factor sign-in")?;
    println!("two-factor sign-in reset for {username}; they were signed out everywhere");
    Ok(())
}

async fn find(db: &Db, tenant: TenantId, username: &str) -> Result<User> {
    db.find_user_by_username(tenant, username)
        .await
        .context("looking up user")?
        .with_context(|| format!("no such user in tenant {tenant}: {username}"))
}

fn read_new_password(from_stdin: bool) -> Result<String> {
    if from_stdin {
        read_password_line()
    } else {
        prompt_new_password()
    }
}

fn read_password_line() -> Result<String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading password from standard input")?;
    let password = line.trim_end_matches(['\r', '\n']);
    anyhow::ensure!(!password.is_empty(), "no password on standard input");
    Ok(password.to_owned())
}

fn prompt_new_password() -> Result<String> {
    let password = rpassword::prompt_password("Password: ").context("reading password")?;
    let confirmation =
        rpassword::prompt_password("Confirm password: ").context("reading password")?;
    anyhow::ensure!(password == confirmation, "passwords do not match");
    Ok(password)
}
