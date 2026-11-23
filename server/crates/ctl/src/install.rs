//! `homespace-ctl install`: asks how the server should be set up, writes a compose project for
//! it and starts it.

mod admin;
mod plan;
mod render;
mod runtime;
mod secrets;
pub mod system;
mod wizard;

use std::path::Path;

use anyhow::{Context, Result};

use crate::{
    cli::InstallArgs,
    installation::{self, Engine, Installation},
};
use plan::{InstallPlan, Owner, Proxy, SecretStore, ServiceUser, Storage};

const COMPOSE_FILE: &str = "compose.yaml";
const ENV_FILE: &str = ".env";
const NGINX_DIR: &str = "nginx";

pub fn run(args: &InstallArgs) -> Result<()> {
    let runtimes = runtime::detect();
    let plan = wizard::ask(&runtimes)?;
    if !wizard::confirm_plan(&plan)? {
        println!("Nothing was changed.");
        return Ok(());
    }
    let installation = Installation {
        dir: plan.dir.clone(),
        engine: plan.runtime.engine,
        // Rootful Podman set up by root keeps needing root; Docker users are in its group.
        sudo: plan.runtime.engine == Engine::Podman && system::is_root(),
    };
    if args.dry_run {
        return show(&plan, &installation);
    }
    let owner = service_owner(&plan)?;
    write_files(&plan, owner.as_ref(), &installation)?;
    start(&installation)?;
    admin::offer(&installation, &plan.auth)?;
    finish(&plan);
    Ok(())
}

/// Who the containers run as with host directories, creating that user when asked to.
fn service_owner(plan: &InstallPlan) -> Result<Option<Owner>> {
    let Storage::HostDirs { user, .. } = &plan.storage else {
        return Ok(None);
    };
    Ok(Some(match user {
        ServiceUser::Existing(owner) => owner.clone(),
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
) -> Result<()> {
    let me = system::current_user()?;
    println!("• Creating {}", plan.dir.display());
    system::make_dir(&plan.dir, &me, 0o750)?;
    if let (Storage::HostDirs { db, files, .. }, Some(owner)) = (&plan.storage, owner) {
        for dir in [db, files] {
            println!("• Preparing {} for {}", dir.display(), owner.name);
            system::make_dir(dir, owner, 0o700)?;
        }
    }
    println!("• Writing {COMPOSE_FILE} and {ENV_FILE}");
    system::write_own_file(&plan.dir.join(COMPOSE_FILE), &render::compose(plan), 0o644)?;
    let env = render::env_file(&render::env_vars(plan, owner), false);
    system::write_own_file(&plan.dir.join(ENV_FILE), &env, 0o600)?;
    if plan.secrets == SecretStore::Files {
        let dir = plan.dir.join(render::SECRETS_DIR);
        println!("• Writing the secrets to {}", dir.display());
        system::make_dir(&dir, &me, 0o700)?;
        for (name, value) in render::secrets(plan) {
            // Readable by any container user; the directory keeps everyone else out.
            system::write_own_file(&dir.join(name.file_name()), value, 0o444)?;
        }
    }
    if let Proxy::Nginx { server_name } = &plan.proxy {
        let dir = plan.dir.join(NGINX_DIR);
        system::make_dir(&dir, &me, 0o755)?;
        let path = nginx_path(&plan.dir, server_name);
        println!("• Writing the nginx example {}", path.display());
        system::write_own_file(&path, &render::nginx(server_name, plan.port), 0o644)?;
    }
    let record = installation::config_path();
    println!("• Recording the installation in {}", record.display());
    system::write_file_anywhere(&record, &installation.to_toml()?, 0o644)
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
}

/// `--dry-run`: shows what would be written and run.
fn show(plan: &InstallPlan, installation: &Installation) -> Result<()> {
    let owner = match &plan.storage {
        Storage::HostDirs { user, .. } => Some(match user {
            ServiceUser::Existing(owner) => owner.clone(),
            ServiceUser::Create { name } => {
                println!("Would create the system user {name}; its ids below are placeholders.");
                Owner {
                    name: name.clone(),
                    uid: 0,
                    gid: 0,
                }
            }
        }),
        Storage::Volumes => None,
    };
    println!(
        "\n--- {}/{COMPOSE_FILE}\n{}",
        plan.dir.display(),
        render::compose(plan)
    );
    println!(
        "--- {}/{ENV_FILE} (secrets masked)\n{}",
        plan.dir.display(),
        render::env_file(&render::env_vars(plan, owner.as_ref()), true)
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
    println!(
        "--- {}\n{}",
        installation::config_path().display(),
        installation.to_toml()?
    );
    println!(
        "Would run `{} compose up -d` in {}. Nothing was changed.",
        plan.runtime.engine.program(),
        plan.dir.display()
    );
    Ok(())
}
