//! `homespace-ctl install`: asks how the server should be set up, writes a compose project for
//! it and starts it.

mod admin;
mod plan;
mod render;
mod runtime;
pub mod system;
mod wizard;

use std::path::Path;

use anyhow::{Context, Result};

use crate::{
    cli::InstallArgs,
    installation::{self, Installation},
};
use plan::{InstallPlan, Owner, Proxy, SecretStore, ServiceUser, Storage};

const COMPOSE_FILE: &str = "compose.yaml";
const ENV_FILE: &str = ".env";
const NGINX_DIR: &str = "nginx";

pub fn run(args: &InstallArgs) -> Result<()> {
    let record = installation::config_path()?;
    refuse_second_record(&record)?;
    let runtimes = runtime::detect();
    let plan = wizard::ask(&runtimes)?;
    if !wizard::confirm_plan(&plan)? {
        println!("Nothing was changed.");
        return Ok(());
    }
    let installation = Installation {
        dir: plan.dir.clone(),
        engine: plan.runtime.engine,
    };
    let owner = service_owner(&plan, args.dry_run)?;
    if args.dry_run {
        return show(&plan, owner.as_ref(), &installation, &record);
    }
    write_files(&plan, owner.as_ref(), &installation, &record)?;
    start(&installation)?;
    admin::offer(&installation, &plan.auth)?;
    finish(&plan);
    Ok(())
}

/// Installing again over a recorded installation would lose track of it.
fn refuse_second_record(record: &Path) -> Result<()> {
    if let Some(existing) = Installation::load(record)? {
        anyhow::bail!(
            "{} records an installation in {} already; remove that file once the installation \
             is gone, or set HOMESPACE_CTL_CONFIG to record a second one elsewhere",
            record.display(),
            existing.dir.display()
        );
    }
    Ok(())
}

/// Who the containers run as with host directories, creating that user when asked to. A dry
/// run creates nobody and gives a user to be created placeholder ids.
fn service_owner(plan: &InstallPlan, dry_run: bool) -> Result<Option<Owner>> {
    let Storage::HostDirs { user, .. } = &plan.storage else {
        return Ok(None);
    };
    Ok(Some(match user {
        ServiceUser::Existing(owner) => owner.clone(),
        ServiceUser::Create { name } if dry_run => {
            println!("Would create the system user {name}; its ids below are placeholders.");
            Owner {
                name: name.clone(),
                uid: 0,
                gid: 0,
            }
        }
        ServiceUser::Create { name } => {
            println!("• Creating the system user {name}");
            system::create_system_user(name)?
        }
    }))
}

fn write_files(
    plan: &InstallPlan,
    owner: Option<&Owner>,
    installation: &Installation,
    record: &Path,
) -> Result<()> {
    let me = system::current_user()?;
    println!("• Creating {}", plan.dir.display());
    system::create_dir(&plan.dir, &me, 0o750)?;
    if let (Storage::HostDirs { db, files, .. }, Some(owner)) = (&plan.storage, owner) {
        for dir in [db, files] {
            println!("• Creating {} for {}", dir.display(), owner.name);
            system::create_dir(dir, owner, 0o700)?;
        }
    }
    println!("• Writing {COMPOSE_FILE} and {ENV_FILE}");
    system::write_own_file(&plan.dir.join(COMPOSE_FILE), &render::compose(plan), 0o644)?;
    let env = render::env_file(&render::env_vars(plan, owner), false);
    system::write_own_file(&plan.dir.join(ENV_FILE), &env, 0o600)?;
    if plan.secrets == SecretStore::Files {
        let dir = plan.dir.join(render::SECRETS_DIR);
        println!("• Writing the secrets to {}", dir.display());
        system::create_dir(&dir, &me, 0o700)?;
        // The files are readable by any container user (0444); only this directory keeps
        // everyone else on the host out, so it is checked before anything goes in.
        system::ensure_private_dir(&dir, &me)?;
        for (name, value) in render::secrets(plan) {
            system::write_own_file(&dir.join(name.file_name()), value, 0o444)?;
        }
    }
    if let Proxy::Nginx { server_name } = &plan.proxy {
        let dir = plan.dir.join(NGINX_DIR);
        system::create_dir(&dir, &me, 0o755)?;
        let path = nginx_path(&plan.dir, server_name);
        println!("• Writing the nginx example {}", path.display());
        system::write_own_file(&path, &render::nginx(server_name, plan.port), 0o644)?;
    }
    println!("• Recording the installation in {}", record.display());
    if let Some(parent) = record.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    system::write_own_file(record, &installation.to_toml()?, 0o600)
}

fn nginx_path(dir: &Path, server_name: &str) -> std::path::PathBuf {
    dir.join(NGINX_DIR).join(format!("{server_name}.conf"))
}

fn start(installation: &Installation) -> Result<()> {
    println!(
        "• Starting Home Space ({} compose up -d)",
        installation.engine.program()
    );
    let status = installation
        .compose(["up", "-d"])
        .status()
        .context("running compose up")?;
    anyhow::ensure!(
        status.success(),
        "compose up failed ({status}); fix the cause and run `{} compose up -d` in {}",
        installation.engine.program(),
        installation.dir.display()
    );
    Ok(())
}

fn finish(plan: &InstallPlan) {
    println!("\nHome Space is running at {}", plan.public_url);
    println!(
        "  Settings: {}/{ENV_FILE} (then `{} compose up -d` there)",
        plan.dir.display(),
        plan.runtime.engine.program()
    );
    if let Proxy::Nginx { server_name } = &plan.proxy {
        println!(
            "  nginx: copy {} to /etc/nginx/conf.d/, check the certificate paths, then\n    \
             sudo nginx -t && sudo systemctl reload nginx",
            nginx_path(&plan.dir, server_name).display()
        );
    }
    println!("  Manage users: homespace-ctl user list | add | admin | passwd");
    println!(
        "  Rotate the secrets only with: homespace-ctl secrets rotate (changing them by hand \
         loses authenticator apps)"
    );
}

/// `--dry-run`: shows what would be written and run.
fn show(
    plan: &InstallPlan,
    owner: Option<&Owner>,
    installation: &Installation,
    record: &Path,
) -> Result<()> {
    println!(
        "\n--- {}/{COMPOSE_FILE}\n{}",
        plan.dir.display(),
        render::compose(plan)
    );
    println!(
        "--- {}/{ENV_FILE} (secrets masked)\n{}",
        plan.dir.display(),
        render::env_file(&render::env_vars(plan, owner), true)
    );
    if plan.secrets == SecretStore::Files {
        for (name, _) in render::secrets(plan) {
            println!(
                "--- {}/{}/{} (secret)",
                plan.dir.display(),
                render::SECRETS_DIR,
                name.file_name()
            );
        }
    }
    if let Proxy::Nginx { server_name } = &plan.proxy {
        println!(
            "--- {}\n{}",
            nginx_path(&plan.dir, server_name).display(),
            render::nginx(server_name, plan.port)
        );
    }
    println!("--- {}\n{}", record.display(), installation.to_toml()?);
    println!(
        "Would run `{} compose up -d` in {}. Nothing was changed.",
        plan.runtime.engine.program(),
        plan.dir.display()
    );
    Ok(())
}
