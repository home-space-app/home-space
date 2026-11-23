//! Finding the container engines that can run compose here.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use super::plan::Runtime;
use crate::installation::Engine;

/// The engines whose `compose` works, with whether each runs rootless.
pub fn detect() -> Vec<Runtime> {
    [Engine::Docker, Engine::Podman]
        .into_iter()
        .filter(|&engine| succeeds(engine.program(), &["compose", "version"]))
        .map(|engine| Runtime {
            engine,
            rootless: rootless(engine),
        })
        .collect()
}

fn rootless(engine: Engine) -> bool {
    let (program, format) = match engine {
        Engine::Podman => ("podman", "{{.Host.Security.Rootless}}"),
        Engine::Docker => ("docker", "{{json .SecurityOptions}}"),
    };
    Command::new(program)
        .args(["info", "--format", format])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| {
            let text = String::from_utf8_lossy(&out.stdout);
            match engine {
                Engine::Podman => text.trim() == "true",
                Engine::Docker => text.contains("rootless"),
            }
        })
}

fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The directory the compose project `project` was started from, if it has containers.
pub fn project_dir(engine: Engine, project: &str) -> Option<PathBuf> {
    let out = Command::new(engine.program())
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
            "--format",
            "{{index .Labels \"com.docker.compose.project.working_dir\"}}",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|dir| !dir.is_empty())
        .map(PathBuf::from)
}
