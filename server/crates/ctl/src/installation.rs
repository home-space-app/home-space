//! The installation `homespace-ctl install` made on this machine, and running commands inside
//! its app container, where the database and the storage root are reachable.

use std::{
    ffi::OsString,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_CONFIG: &str = "/etc/homespace/ctl.toml";
const CONFIG_VAR: &str = "HOMESPACE_CTL_CONFIG";
const APP_SERVICE: &str = "app";
const CTL_IN_IMAGE: &str = "homespace-ctl";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Docker,
    Podman,
}

impl Engine {
    pub fn program(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    pub dir: PathBuf,
    pub engine: Engine,
    pub sudo: bool,
}

pub fn config_path() -> PathBuf {
    std::env::var_os(CONFIG_VAR).map_or_else(|| PathBuf::from(DEFAULT_CONFIG), PathBuf::from)
}

impl Installation {
    /// The installation recorded in `path`, `None` when there is none.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("reading {}", path.display()))
                .map(Some),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn to_toml(&self) -> Result<String> {
        let body = toml::to_string(self).context("writing the installation record")?;
        Ok(format!(
            "# Written by `homespace-ctl install`: where homespace-ctl finds this server.\n{body}"
        ))
    }

    /// `[sudo] <engine> compose <args>`, run in the installation's directory.
    pub fn compose<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut command = if self.sudo && !crate::install::system::is_root() {
            let mut command = Command::new("sudo");
            command.arg(self.engine.program());
            command
        } else {
            Command::new(self.engine.program())
        };
        command
            .arg("compose")
            .args(args.into_iter().map(Into::into))
            .current_dir(&self.dir);
        command
    }

    pub fn exec_ctl<I, S>(&self, args: I, tty: bool) -> Command
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut compose_args: Vec<OsString> = vec!["exec".into()];
        if !tty {
            compose_args.push("-T".into());
        }
        compose_args.push(APP_SERVICE.into());
        compose_args.push(CTL_IN_IMAGE.into());
        compose_args.extend(args.into_iter().map(Into::into));
        self.compose(compose_args)
    }

    pub fn forward_command_line(&self) -> Result<ExitStatus> {
        let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        self.exec_ctl(std::env::args_os().skip(1), tty)
            .status()
            .with_context(|| {
                format!(
                    "running {} compose in {}",
                    self.engine.program(),
                    self.dir.display()
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_survives_a_round_trip() {
        let installation = Installation {
            dir: PathBuf::from("/opt/homespace"),
            engine: Engine::Podman,
            sudo: true,
        };
        let text = installation.to_toml().expect("toml");
        assert!(text.contains("engine = \"podman\""), "{text}");
        let parsed: Installation = toml::from_str(&text).expect("parse");
        assert_eq!(parsed, installation);
    }

    #[test]
    fn exec_without_a_tty_passes_minus_t() {
        let installation = Installation {
            dir: PathBuf::from("/opt/homespace"),
            engine: Engine::Docker,
            sudo: false,
        };
        let command = installation.exec_ctl(["user", "list"], false);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(command.get_program(), "docker");
        assert_eq!(
            args,
            [
                "compose",
                "exec",
                "-T",
                "app",
                "homespace-ctl",
                "user",
                "list"
            ]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/opt/homespace")));
    }
}
