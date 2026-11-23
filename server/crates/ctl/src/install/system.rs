//! Changes to the host: users, directories and files. Whatever needs root runs directly when
//! this process is root and through `sudo` otherwise, so the installer can run as a regular
//! user and asks for a password only when it has to.

use std::{
    io::{ErrorKind, Write as _},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result};

use super::plan::Owner;

/// The effective user and group of this process.
fn effective_ids() -> Option<(u32, u32)> {
    // /proc/self belongs to the effective uid and gid of the process.
    std::fs::metadata("/proc/self")
        .ok()
        .map(|meta| (meta.uid(), meta.gid()))
}

pub fn is_root() -> bool {
    effective_ids().is_some_and(|(uid, _)| uid == 0)
}

/// The user this process runs as.
pub fn current_user() -> Result<Owner> {
    let (uid, gid) = effective_ids().context("reading /proc/self")?;
    let name = output(Command::new("id").arg("-un"))?;
    Ok(Owner { name, uid, gid })
}

/// The host user `name`, `None` when there is none.
pub fn find_user(name: &str) -> Result<Option<Owner>> {
    let uid = Command::new("id").args(["-u", name]).output();
    match uid {
        Ok(out) if out.status.success() => {
            let uid = parse_id(&out.stdout)?;
            let gid = output(Command::new("id").args(["-g", name]))?
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
        name,
    ])?;
    find_user(name)?.with_context(|| format!("user {name} is missing after useradd"))
}

/// Makes `path` a directory owned by `owner` with `mode`, creating it if needed. An existing
/// directory owned by someone else is given to `owner`.
pub fn make_dir(path: &Path, owner: &Owner, mode: u32) -> Result<()> {
    let display = path.display().to_string();
    let mode_arg = format!("{mode:o}");
    match std::fs::metadata(path) {
        Ok(meta) => {
            anyhow::ensure!(meta.is_dir(), "{display} exists and is not a directory");
            if meta.uid() != owner.uid || meta.gid() != owner.gid {
                privileged(&["chown", &format!("{}:{}", owner.uid, owner.gid), &display])?;
            }
            if meta.mode() & 0o7777 != mode {
                privileged(&["chmod", &mode_arg, &display])?;
            }
            Ok(())
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            if is_self(owner) && create_own_dir(path, mode).is_ok() {
                return Ok(());
            }
            privileged(&[
                "install",
                "-d",
                "-o",
                &owner.uid.to_string(),
                "-g",
                &owner.gid.to_string(),
                "-m",
                &mode_arg,
                &display,
            ])
        }
        Err(err) => Err(err).with_context(|| format!("checking {display}")),
    }
}

fn is_self(owner: &Owner) -> bool {
    effective_ids() == Some((owner.uid, owner.gid))
}

fn create_own_dir(path: &Path, mode: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Whether `path` is a directory with anything in it.
pub fn has_entries(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
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

/// Writes `contents` to `path`, through `sudo install` when this user may not write there.
pub fn write_file_anywhere(path: &Path, contents: &str, mode: u32) -> Result<()> {
    let direct = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .map_err(anyhow::Error::from)
        .and_then(|()| write_own_file(path, contents, mode));
    if direct.is_ok() {
        return Ok(());
    }
    let mut temp = tempfile::NamedTempFile::new().context("creating a temporary file")?;
    temp.write_all(contents.as_bytes())
        .context("writing a temporary file")?;
    let temp_path = temp.path().display().to_string();
    privileged(&[
        "install",
        "-D",
        "-m",
        &format!("{mode:o}"),
        &temp_path,
        &path.display().to_string(),
    ])
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
