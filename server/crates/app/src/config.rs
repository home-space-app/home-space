use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};

const MIN_SECRET_LEN: usize = 32;

/// Where file content is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum StorageBackend {
    /// Files on this machine's disk, under the storage root.
    Local,
}

/// Settings shared by all subcommands, read from flags or environment.
#[derive(Args, Debug)]
pub struct Config {
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,

    /// Secret that signs access tokens; set this or `HOMESPACE_JWT_SECRET_FILE`.
    #[arg(long, env = "HOMESPACE_JWT_SECRET", hide_env_values = true)]
    pub jwt_secret: Option<String>,

    /// File holding the signing secret (e.g. a Docker/Podman secret).
    #[arg(long, env = "HOMESPACE_JWT_SECRET_FILE")]
    pub jwt_secret_file: Option<PathBuf>,

    /// Earlier signing secret that still verifies tokens, so the secret can be rotated
    /// without ending sessions. Remove it once the access tokens it signed have expired.
    #[arg(long, env = "HOMESPACE_JWT_SECRET_PREVIOUS", hide_env_values = true)]
    pub jwt_secret_previous: Option<String>,

    /// Where file content is kept. Only `local` exists today; object storage is planned.
    #[arg(long, env = "HOMESPACE_STORAGE", value_enum, default_value_t = StorageBackend::Local)]
    pub storage: StorageBackend,

    #[arg(long, env = "HOMESPACE_STORAGE_ROOT", default_value = "./data")]
    pub storage_root: PathBuf,

    #[arg(long, env = "HOMESPACE_BIND", default_value = "127.0.0.1:8080")]
    pub bind: SocketAddr,

    /// The host port a container is published on. Compose uses it for the port mapping; the
    /// server only shows it to administrators, since inside the container it listens on
    /// `HOMESPACE_BIND`.
    #[arg(long, env = "HOMESPACE_PORT")]
    pub port: Option<u16>,

    #[arg(long, env = "HOMESPACE_WEB_ROOT")]
    pub web_root: Option<PathBuf>,

    #[arg(long, env = "HOMESPACE_MAX_UPLOAD_BYTES", default_value_t = 10 * 1024 * 1024 * 1024)]
    pub max_upload_bytes: u64,

    /// Largest request body of an upload, reported to clients by `/capabilities`: smaller files
    /// go up in one request, larger ones in chunks of this size. Keep it below the request body
    /// limit of any proxy in front of the server (Cloudflare allows 100 MB).
    #[arg(long, env = "HOMESPACE_UPLOAD_CHUNK_BYTES", default_value_t = files::DEFAULT_CHUNK_BYTES)]
    pub upload_chunk_bytes: u64,

    /// How long an unfinished resumable upload is kept without receiving a chunk.
    #[arg(long, env = "HOMESPACE_UPLOAD_EXPIRY_HOURS", default_value_t = 24)]
    pub upload_expiry_hours: u64,

    #[arg(long, env = "HOMESPACE_ACCESS_TTL_SECS", default_value_t = 900)]
    pub access_ttl_secs: i64,

    #[arg(long, env = "HOMESPACE_REFRESH_TTL_SECS", default_value_t = 30 * 24 * 3600)]
    pub refresh_ttl_secs: i64,

    /// Externally reachable address of this server, e.g. `https://files.example.com`.
    /// Required for single sign-on.
    #[arg(long, env = "HOMESPACE_PUBLIC_URL")]
    pub public_url: Option<String>,

    /// Issuer URL of an OpenID Connect provider such as Auth0 or Keycloak; enables single
    /// sign-on.
    #[arg(long, env = "HOMESPACE_OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,

    #[arg(long, env = "HOMESPACE_OIDC_CLIENT_ID")]
    pub oidc_client_id: Option<String>,

    #[arg(long, env = "HOMESPACE_OIDC_CLIENT_SECRET", hide_env_values = true)]
    pub oidc_client_secret: Option<String>,

    #[arg(
        long,
        env = "HOMESPACE_OIDC_SCOPES",
        default_value = "openid profile email"
    )]
    pub oidc_scopes: String,

    /// Only members of this group may sign in through the provider.
    #[arg(long, env = "HOMESPACE_OIDC_REQUIRED_GROUP")]
    pub oidc_required_group: Option<String>,

    /// ID token claim that lists a user's groups (Auth0 needs a namespaced custom claim).
    #[arg(long, env = "HOMESPACE_OIDC_GROUPS_CLAIM", default_value = "groups")]
    pub oidc_groups_claim: String,

    /// Name on the sign-in button.
    #[arg(long, env = "HOMESPACE_OIDC_NAME", default_value = "Single sign-on")]
    pub oidc_name: String,

    /// Allow username and password login. Turn off to use only the identity provider.
    #[arg(
        long,
        env = "HOMESPACE_PASSWORD_LOGIN",
        default_value_t = true,
        action = clap::ArgAction::Set
    )]
    pub password_login: bool,

    /// How long the change journal keeps entries; clients offline for longer resync fully.
    /// Unset, the journal is only compacted to the latest entry of each node and kept for good.
    #[arg(long, env = "HOMESPACE_CHANGES_RETENTION_DAYS")]
    pub changes_retention_days: Option<i64>,
}

impl Config {
    /// Rejects settings that would make the server insecure.
    pub fn validate(&self) -> Result<()> {
        self.jwt_secrets()
            .map(drop)
            .context("invalid configuration")?;
        self.oidc().map(drop).context("invalid configuration")
    }

    /// Settings for single sign-on; `None` unless an issuer is configured.
    pub fn oidc(&self) -> Result<Option<auth::OidcConfig>> {
        let Some(issuer) = non_empty(&self.oidc_issuer) else {
            return Ok(None);
        };
        let client_id = non_empty(&self.oidc_client_id)
            .context("HOMESPACE_OIDC_CLIENT_ID is required with HOMESPACE_OIDC_ISSUER")?;
        let public_url = non_empty(&self.public_url)
            .context("HOMESPACE_PUBLIC_URL is required with HOMESPACE_OIDC_ISSUER")?;
        anyhow::ensure!(
            is_https_or_loopback(issuer),
            "HOMESPACE_OIDC_ISSUER must use https"
        );
        anyhow::ensure!(
            is_https_or_loopback(public_url),
            "HOMESPACE_PUBLIC_URL must use https"
        );
        Ok(Some(auth::OidcConfig {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            client_secret: non_empty(&self.oidc_client_secret).map(str::to_owned),
            scopes: self
                .oidc_scopes
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            public_url: public_url.to_owned(),
            required_group: non_empty(&self.oidc_required_group).map(str::to_owned),
            groups_claim: self.oidc_groups_claim.clone(),
            display_name: self.oidc_name.clone(),
        }))
    }

    /// The settings shown to administrators, without secrets.
    pub fn summary(&self) -> Result<protocol::ServerConfig> {
        let oidc = self.oidc()?.map(|oidc| protocol::OidcStatus {
            name: oidc.display_name,
            issuer: oidc.issuer,
            required_group: oidc.required_group,
        });
        let (_, previous) = self.jwt_secrets()?;
        Ok(protocol::ServerConfig {
            max_upload_bytes: self.max_upload_bytes,
            upload_chunk_bytes: self.upload_chunk_bytes,
            upload_expiry_hours: self.upload_expiry_hours,
            changes_retention_days: self.changes_retention_days,
            password_login: self.password_login,
            oidc,
            access_ttl_secs: self.access_ttl_secs,
            refresh_ttl_secs: self.refresh_ttl_secs,
            previous_jwt_secret: !previous.is_empty(),
            public_url: non_empty(&self.public_url).map(str::to_owned),
            bind: self.bind.to_string(),
            published_port: self.port,
            storage: match self.storage {
                StorageBackend::Local => protocol::StorageBackend::Local,
            },
        })
    }

    /// The current signing secret followed by the previous one, if any.
    pub fn jwt_secrets(&self) -> Result<(Vec<u8>, Vec<Vec<u8>>)> {
        let current = match (&self.jwt_secret, &self.jwt_secret_file) {
            (Some(_), Some(_)) => {
                anyhow::bail!("set only one of HOMESPACE_JWT_SECRET and HOMESPACE_JWT_SECRET_FILE")
            }
            (Some(secret), None) => secret.clone(),
            (None, Some(path)) => std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
            (None, None) => {
                anyhow::bail!("HOMESPACE_JWT_SECRET or HOMESPACE_JWT_SECRET_FILE must be set")
            }
        };
        check_secret_len("the JWT secret", &current)?;
        let previous = match self
            .jwt_secret_previous
            .as_ref()
            .filter(|secret| !secret.is_empty())
        {
            Some(secret) => {
                check_secret_len("HOMESPACE_JWT_SECRET_PREVIOUS", secret)?;
                vec![secret.clone().into_bytes()]
            }
            None => Vec::new(),
        };
        Ok((current.into_bytes(), previous))
    }
}

fn check_secret_len(name: &str, secret: &str) -> Result<()> {
    if secret.len() < MIN_SECRET_LEN {
        anyhow::bail!("{name} must be at least {MIN_SECRET_LEN} characters");
    }
    Ok(())
}

/// The value of an optional setting, treating an empty one (as Compose passes) as unset.
fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// Whether `url` is https, or plain http to this machine for local development.
fn is_https_or_loopback(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    url.strip_prefix("http://").is_some_and(|rest| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = if authority.starts_with('[') {
            authority.split_inclusive(']').next().unwrap_or_default()
        } else {
            authority.split(':').next().unwrap_or_default()
        };
        matches!(host, "localhost" | "127.0.0.1" | "[::1]")
    })
}

#[cfg(test)]
mod tests {
    use super::is_https_or_loopback;

    #[test]
    fn https_and_loopback_http_are_accepted() {
        for url in [
            "https://idp.example.com",
            "http://localhost:8081/realms/x",
            "http://127.0.0.1:8080",
            "http://[::1]:8080/",
        ] {
            assert!(is_https_or_loopback(url), "{url}");
        }
    }

    #[test]
    fn other_http_is_rejected() {
        for url in [
            "http://idp.example.com",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com/",
            "http://user@evil.com",
            "ftp://localhost",
            "",
        ] {
            assert!(!is_https_or_loopback(url), "{url}");
        }
    }
}
