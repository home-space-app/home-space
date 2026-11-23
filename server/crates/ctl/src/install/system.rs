//! Changes to the host: users, directories and files. Whatever needs root runs directly when
//! this process is root and through `sudo` otherwise, so the installer can run as a regular
//! user and asks for a password only when it has to.

use std::{
    fs::DirBuilder,
    io::{ErrorKind, Write as _},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result};
use rustix::process::{getegid, geteuid};

use super::plan::Owner;

pub fn is_root() -> bool {
    geteuid().is_root()
}

/// The user this process runs as.
pub fn current_user() -> Result<Owner> {
    let name = output(Command::new("id").arg("-un"))?;
    Ok(Owner {
        name,
        uid: geteuid().as_raw(),
        gid: getegid().as_raw(),
    })
}

/// The host user `name`, `None` when there is none.
pub fn find_user(name: &str) -> Result<Option<Owner>> {
    let uid = Command::new("id").args(["-u", "--", name]).output();
    match uid {
        Ok(out) if out.status.success() => {
            let uid = parse_id(&out.stdout)?;
            let gid = output(Command::new("id").args(["-g", "--", name]))?
                .parse()
                .context("reading the group id")?;
            Ok(Some(Owner {
                name: name.to_owned(),
                uid,
                gid,
            }))
        }
        Ok(_) => Ok(None),
        Err(err) => Err(err).context("running id"),
    }
}

fn parse_id(stdout: &[u8]) -> Result<u32> {
    String::from_utf8_lossy(stdout)
        .trim()
        .parse()
        .context("reading the user id")
}

/// Creates the system user `name` with a group of its own, no home and no login shell.
pub fn create_system_user(name: &str) -> Result<Owner> {
    privileged(&[
        "useradd",
        "--system",
        "--user-group",
        "--no-create-home",
        "--shell",
        "/usr/sbin/nologin",
        "--",
        name,
    ])?;
    find_user(name)?.with_context(|| format!("user {name} is missing after useradd"))
}

/// Creates the directory `path` owned by `owner` with `mode`, and any missing parents (with
/// the default owner and mode). `path` must not exist yet: the installer never takes over a
/// directory that is there already.
pub fn create_dir(path: &Path, owner: &Owner, mode: u32) -> Result<()> {
    let display = path.display().to_string();
    match std::fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!(
            "{display} exists already; the installer only creates new directories, so pick \
             another path or remove it"
        ),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("checking {display}")),
    }
    if is_self(owner) {
        match create_own_dir(path, mode) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::PermissionDenied => {}
            Err(err) => return Err(err).with_context(|| format!("creating {display}")),
        }
    }
    if let Some(parent) = path.parent().filter(|parent| !parent.exists()) {
        privileged(&["mkdir", "-p", "--", &parent.display().to_string()])?;
    }
    // Without -p, mkdir fails when the directory appeared in the meantime.
    privileged(&["mkdir", "-m", &format!("{mode:o}"), "--", &display])?;
    privileged(&[
        "chown",
        "--",
        &format!("{}:{}", owner.uid, owner.gid),
        &display,
    ])
}

fn is_self(owner: &Owner) -> bool {
    geteuid().as_raw() == owner.uid && getegid().as_raw() == owner.gid
}

fn create_own_dir(path: &Path, mode: u32) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // `create` fails when the directory exists; the mode is set again past the umask.
    DirBuilder::new().mode(mode).create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Fails unless `path` is a directory that only `owner` may enter (mode 700), which is what
/// keeps the world-readable secret files inside it private.
pub fn ensure_private_dir(path: &Path, owner: &Owner) -> Result<()> {
    let meta =
        std::fs::symlink_metadata(path).with_context(|| format!("checking {}", path.display()))?;
    anyhow::ensure!(
        meta.is_dir() && meta.uid() == owner.uid && meta.mode() & 0o777 == 0o700,
        "{} must be a directory of {} with mode 700",
        path.display(),
        owner.name
    );
    Ok(())
}

/// Writes `contents` to `path` (in a directory of this user) with `mode`, replacing any file
/// there.
pub fn write_own_file(path: &Path, contents: &str, mode: u32) -> Result<()> {
    let display = path.display();
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("replacing {display}")),
    }
    // Created owner-only, so a secret is never readable by others, then given its mode.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {display}"))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("writing {display}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting the mode of {display}"))
}

/// Runs `args`, through `sudo` unless this process is root.
pub fn privileged(args: &[&str]) -> Result<()> {
    let (program, rest) = args.split_first().context("no command")?;
    let mut command = if is_root() {
        Command::new(program)
    } else {
        let mut command = Command::new("sudo");
        command.arg(program);
        command
    };
    command.args(rest);
    let shown = if is_root() {
        args.join(" ")
    } else {
        format!("sudo {}", args.join(" "))
    };
    println!("  $ {shown}");
    let status = command
        .status()
        .with_context(|| format!("running {shown}"))?;
    anyhow::ensure!(status.success(), "{shown} failed ({status})");
    Ok(())
}

/// The trimmed standard output of a command that must succeed.
pub fn output(command: &mut Command) -> Result<String> {
    let out = command
        .output()
        .with_context(|| format!("running {:?}", command.get_program()))?;
    anyhow::ensure!(
        out.status.success(),
        "{:?} failed: {}",
        command.get_program(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A port on the loopback interface that nothing listens on right now.
pub fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context("finding a free port")?;
    Ok(listener.local_addr().context("finding a free port")?.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_directory_gets_its_mode_and_an_existing_one_is_refused() {
        let root = tempfile::tempdir().expect("tempdir");
        let me = current_user().expect("current user");
        let secrets = root.path().join("install").join("secrets");
        create_dir(&secrets, &me, 0o700).expect("create");
        ensure_private_dir(&secrets, &me).expect("private");

        let error = create_dir(&secrets, &me, 0o700).expect_err("exists");
        assert!(error.to_string().contains("exists already"), "{error}");
        let error = create_dir(root.path(), &me, 0o750).expect_err("exists");
        assert!(error.to_string().contains("exists already"), "{error}");
    }

    #[test]
    fn a_directory_others_may_enter_is_not_private() {
        let root = tempfile::tempdir().expect("tempdir");
        let me = current_user().expect("current user");
        let open = root.path().join("open");
        create_dir(&open, &me, 0o750).expect("create");
        assert!(ensure_private_dir(&open, &me).is_err());
    }
}
