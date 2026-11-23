//! What the wizard decided: everything an installation is made from.

use std::path::PathBuf;

use crate::installation::Engine;

/// The container engine and whether it runs without root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Runtime {
    pub engine: Engine,
    /// Rootless: container uids are mapped to the invoking user's subordinate ids.
    pub rootless: bool,
}

/// Where the secrets go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretStore {
    /// In `.env` next to the compose file (mode 600).
    EnvFile,
    /// One file per secret under `secrets/`, mounted as compose secrets.
    Files,
}

/// A host user that owns the data directories and runs the containers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
}

/// The user the containers run as, when the data lives in host directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceUser {
    Existing(Owner),
    /// A system user to create; its ids are known once it exists.
    Create {
        name: String,
    },
}

impl ServiceUser {
    pub fn name(&self) -> &str {
        match self {
            Self::Existing(owner) => &owner.name,
            Self::Create { name } => name,
        }
    }
}

/// Where the database and the files are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Storage {
    /// Named volumes managed by the container engine; the images' own users own them.
    Volumes,
    /// Host directories, owned by `user`, who the containers run as.
    HostDirs {
        db: PathBuf,
        files: PathBuf,
        user: ServiceUser,
    },
}

/// A secret value and whether it was generated (and so may be shown nowhere).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Secret {
    pub value: String,
    pub generated: bool,
}

/// An OpenID Connect provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Oidc {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub name: String,
}

/// How people sign in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    Passwords,
    Oidc(Oidc),
    OidcAndPasswords(Oidc),
}

impl Auth {
    pub fn oidc(&self) -> Option<&Oidc> {
        match self {
            Self::Passwords => None,
            Self::Oidc(oidc) | Self::OidcAndPasswords(oidc) => Some(oidc),
        }
    }

    pub fn password_login(&self) -> bool {
        !matches!(self, Self::Oidc(_))
    }
}

/// What sits in front of the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proxy {
    /// Clients connect to the published port themselves.
    None,
    /// nginx, with an example site written for `server_name`.
    Nginx { server_name: String },
    /// Another reverse proxy that sets `X-Forwarded-For`.
    Other,
}

impl Proxy {
    pub fn trusts_forwarded_for(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// The installation to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPlan {
    pub runtime: Runtime,
    /// Holds `compose.yaml`, `.env` and the rest.
    pub dir: PathBuf,
    /// The compose project name, which prefixes the containers and volumes.
    pub project: String,
    pub secrets: SecretStore,
    pub storage: Storage,
    pub db_user: String,
    pub db_password: Secret,
    pub jwt_secret: Secret,
    pub proxy: Proxy,
    pub public_url: String,
    pub auth: Auth,
    /// The host port the app is published on.
    pub port: u16,
    /// The host address the port is published on: loopback behind a proxy.
    pub listen: String,
}

impl InstallPlan {
    /// The containers' user is mapped back to the invoking user (rootless Podman with host
    /// directories), so files on the host belong to them.
    pub fn keep_id(&self) -> bool {
        self.runtime.engine == Engine::Podman
            && self.runtime.rootless
            && matches!(self.storage, Storage::HostDirs { .. })
    }
}

/// Settings every installation gets.
pub mod defaults {
    pub const DIR: &str = "/opt/homespace";
    pub const PROJECT: &str = "homespace";
    /// Overrides [`PROJECT`], for a second installation on the same machine.
    pub const PROJECT_VAR: &str = "HOMESPACE_COMPOSE_PROJECT";
    pub const DB_DIR: &str = "/srv/homespace/db";
    pub const FILES_DIR: &str = "/srv/homespace/files";
    pub const SERVICE_USER: &str = "homespace";
    pub const DB_USER: &str = "homespace";
    pub const OIDC_NAME: &str = "Single sign-on";
    pub const MAX_UPLOAD_BYTES: u64 = 10_737_418_240;
    pub const ACCESS_TTL_SECS: u64 = 900;
    pub const REFRESH_TTL_SECS: u64 = 2_592_000;
    pub const DB_IMAGE: &str = "docker.io/library/postgres:18";
    pub const APP_IMAGE: &str = "docker.io/homespaceapp/home-space:latest";
}
