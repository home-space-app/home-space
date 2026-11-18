use std::{collections::HashSet, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};

const MIN_SECRET_LEN: usize = 32;
/// Fewer different characters than this in a secret means it was typed, not generated.
const MIN_SECRET_DISTINCT_CHARS: usize = 10;
/// What the example configuration used to ship as the secret, and so known to everyone.
const PLACEHOLDER_SECRET_MARK: &str = "change-me";
const GENERATE_SECRET: &str = "`openssl rand -base64 48`";
pub const DEFAULT_OIDC_SCOPES: &str = "openid profile email";
pub const DEFAULT_OIDC_GROUPS_CLAIM: &str = "groups";
pub const DEFAULT_OIDC_NAME: &str = "Single sign-on";

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

    /// Take the client's address from the `X-Forwarded-For` header, for a server behind a
    /// reverse proxy that sets it (nginx: `proxy_add_x_forwarded_for`). Failed sign-ins are
    /// counted per client address, and without this every client behind the proxy counts as
    /// one. Leave it off when clients reach the server directly: they could forge the header.
    #[arg(
        long,
        env = "HOMESPACE_TRUST_FORWARDED_FOR",
        default_value_t = false,
        action = clap::ArgAction::Set
    )]
    pub trust_forwarded_for: bool,

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

    /// How long after an administrator deletes a user their account and all their files are
    /// removed. The deletion can be cancelled until then.
    #[arg(
        long,
        env = "HOMESPACE_USER_DELETION_DELAY_MINUTES",
        default_value_t = 10
    )]
    pub user_deletion_delay_minutes: u64,

    /// How long files sent to a device are kept, whether they were received or not.
    #[arg(long, env = "HOMESPACE_TRANSFER_TTL_HOURS", default_value_t = 24)]
    pub transfer_ttl_hours: u64,

    /// The most bytes one transfer to a device may hold; unset for no limit beyond
    /// `HOMESPACE_MAX_UPLOAD_BYTES` per file.
    #[arg(long, env = "HOMESPACE_MAX_TRANSFER_BYTES")]
    pub max_transfer_bytes: Option<u64>,

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
        default_value = DEFAULT_OIDC_SCOPES
    )]
    pub oidc_scopes: String,

    /// Only members of this group may sign in through the provider.
    #[arg(long, env = "HOMESPACE_OIDC_REQUIRED_GROUP")]
    pub oidc_required_group: Option<String>,

    /// ID token claim that lists a user's groups (Auth0 needs a namespaced custom claim).
    #[arg(long, env = "HOMESPACE_OIDC_GROUPS_CLAIM", default_value = DEFAULT_OIDC_GROUPS_CLAIM)]
    pub oidc_groups_claim: String,

    /// Name on the sign-in button.
    #[arg(long, env = "HOMESPACE_OIDC_NAME", default_value = DEFAULT_OIDC_NAME)]
    pub oidc_name: String,

    /// Allow username and password login. Turn off to use only the identity provider.
    #[arg(
        long,
        env = "HOMESPACE_PASSWORD_LOGIN",
        default_value_t = true,
        action = clap::ArgAction::Set
    )]
    pub password_login: bool,

    /// How requests find their tenant. `single` (the default) serves every request as
    /// tenant 1, whatever host it was sent to, as before tenants existed. `host` serves each
    /// tenant on the domains the database lists for it.
    #[arg(long, env = "HOMESPACE_TENANT_MODE", default_value = "single")]
    pub tenant_mode: api::TenantMode,

    /// How long the change journal keeps entries; clients offline for longer resync fully.
    /// Unset, the journal is only compacted to the latest entry of each node and kept for good.
    #[arg(long, env = "HOMESPACE_CHANGES_RETENTION_DAYS")]
    pub changes_retention_days: Option<i64>,
}

impl Config {
    /// Rejects settings that would make the server insecure.
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.transfer_ttl_hours > 0,
            "HOMESPACE_TRANSFER_TTL_HOURS must be at least 1"
        );
        self.tenant_settings()
            .validate()
            .map(drop)
            .context("invalid configuration")
    }

    /// The settings of tenant 1, which are the server's own unsuffixed settings, so that an
    /// install from before tenants needs no change.
    pub fn tenant_settings(&self) -> TenantSettings {
        TenantSettings {
            suffix: String::new(),
            jwt_secret: self.jwt_secret.clone(),
            jwt_secret_file: self.jwt_secret_file.clone(),
            jwt_secret_previous: self.jwt_secret_previous.clone(),
            public_url: self.public_url.clone(),
            oidc_issuer: self.oidc_issuer.clone(),
            oidc_client_id: self.oidc_client_id.clone(),
            oidc_client_secret: self.oidc_client_secret.clone(),
            oidc_scopes: self.oidc_scopes.clone(),
            oidc_required_group: self.oidc_required_group.clone(),
            oidc_groups_claim: self.oidc_groups_claim.clone(),
            oidc_name: self.oidc_name.clone(),
            password_login: self.password_login,
        }
    }

    /// Settings for single sign-on of tenant 1; `None` unless an issuer is configured.
    pub fn oidc(&self) -> Result<Option<auth::OidcConfig>> {
        self.tenant_settings().oidc()
    }

    /// The current signing secret of tenant 1 followed by the previous one, if any.
    pub fn jwt_secrets(&self) -> Result<(Vec<u8>, Vec<Vec<u8>>)> {
        self.tenant_settings().jwt_secrets()
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
            transfer_ttl_hours: self.transfer_ttl_hours,
            max_transfer_bytes: self.max_transfer_bytes,
            changes_retention_days: self.changes_retention_days,
            user_deletion_delay_minutes: self.user_deletion_delay_minutes,
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
}

/// The settings that may differ per tenant, as given and not yet checked. Tenant 1 has the
/// server's unsuffixed settings; tenant *n* has the same names with `_n` appended.
#[derive(Clone, Debug, Default)]
pub struct TenantSettings {
    /// Appended to the variable names in messages: empty for tenant 1, `_2` for tenant 2.
    pub suffix: String,
    pub jwt_secret: Option<String>,
    pub jwt_secret_file: Option<PathBuf>,
    pub jwt_secret_previous: Option<String>,
    pub public_url: Option<String>,
    pub oidc_issuer: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    pub oidc_scopes: String,
    pub oidc_required_group: Option<String>,
    pub oidc_groups_claim: String,
    pub oidc_name: String,
    pub password_login: bool,
}

/// A tenant's settings once checked.
#[derive(Clone, Debug)]
pub struct TenantConfig {
    pub jwt_secret: Vec<u8>,
    pub previous_jwt_secrets: Vec<Vec<u8>>,
    pub public_url: Option<String>,
    pub oidc: Option<auth::OidcConfig>,
    pub password_login: bool,
}

impl TenantSettings {
    /// Checks the settings, refusing those that would make the tenant insecure.
    pub fn validate(&self) -> Result<TenantConfig> {
        let (jwt_secret, previous_jwt_secrets) = self.jwt_secrets()?;
        Ok(TenantConfig {
            jwt_secret,
            previous_jwt_secrets,
            public_url: non_empty(&self.public_url).map(str::to_owned),
            oidc: self.oidc()?,
            password_login: self.password_login,
        })
    }

    /// The name of this tenant's variable `name`.
    fn var(&self, name: &str) -> String {
        format!("{name}{}", self.suffix)
    }

    /// The name of this tenant's variable that holds the path of a file with `name`'s value.
    fn file_var(&self, name: &str) -> String {
        format!("{name}{}_FILE", self.suffix)
    }

    /// Settings for single sign-on; `None` unless an issuer is configured.
    pub fn oidc(&self) -> Result<Option<auth::OidcConfig>> {
        let issuer_var = self.var("HOMESPACE_OIDC_ISSUER");
        let Some(issuer) = non_empty(&self.oidc_issuer) else {
            return Ok(None);
        };
        let client_id = non_empty(&self.oidc_client_id).with_context(|| {
            format!(
                "{} is required with {issuer_var}",
                self.var("HOMESPACE_OIDC_CLIENT_ID")
            )
        })?;
        let public_url_var = self.var("HOMESPACE_PUBLIC_URL");
        let public_url = non_empty(&self.public_url)
            .with_context(|| format!("{public_url_var} is required with {issuer_var}"))?;
        anyhow::ensure!(is_https_or_loopback(issuer), "{issuer_var} must use https");
        anyhow::ensure!(
            is_https_or_loopback(public_url),
            "{public_url_var} must use https"
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

    /// The current signing secret followed by the previous one, if any.
    pub fn jwt_secrets(&self) -> Result<(Vec<u8>, Vec<Vec<u8>>)> {
        let current = match (&self.jwt_secret, &self.jwt_secret_file) {
            (Some(_), Some(_)) => anyhow::bail!(
                "set only one of {} and {}",
                self.var("HOMESPACE_JWT_SECRET"),
                self.file_var("HOMESPACE_JWT_SECRET")
            ),
            (Some(secret), None) => secret.clone(),
            (None, Some(path)) => std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
            (None, None) => anyhow::bail!(
                "{} or {} must be set",
                self.var("HOMESPACE_JWT_SECRET"),
                self.file_var("HOMESPACE_JWT_SECRET")
            ),
        };
        let name = if self.suffix.is_empty() {
            "the JWT secret".to_owned()
        } else {
            self.var("HOMESPACE_JWT_SECRET")
        };
        check_secret(&name, &current)?;
        let previous = match self
            .jwt_secret_previous
            .as_ref()
            .filter(|secret| !secret.is_empty())
        {
            Some(secret) => {
                check_secret(&self.var("HOMESPACE_JWT_SECRET_PREVIOUS"), secret)?;
                vec![secret.clone().into_bytes()]
            }
            None => Vec::new(),
        };
        Ok((current.into_bytes(), previous))
    }
}

/// Refuses a signing secret that is short, a placeholder left in place, or made of a few
/// repeated characters: whoever can guess it can sign tokens.
fn check_secret(name: &str, secret: &str) -> Result<()> {
    let distinct = secret.chars().collect::<HashSet<_>>().len();
    if secret.len() < MIN_SECRET_LEN {
        anyhow::bail!("{name} must be at least {MIN_SECRET_LEN} characters");
    } else if secret
        .to_ascii_lowercase()
        .contains(PLACEHOLDER_SECRET_MARK)
    {
        anyhow::bail!("{name} is still a placeholder; generate one with {GENERATE_SECRET}");
    } else if distinct < MIN_SECRET_DISTINCT_CHARS {
        anyhow::bail!("{name} is too easy to guess; generate one with {GENERATE_SECRET}");
    }
    Ok(())
}

/// The value of an optional setting, treating an empty one (as Compose passes) as unset.
fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// Whether `url` is https, or plain http to this machine for local development.
pub fn is_https_or_loopback(url: &str) -> bool {
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
    use super::{check_secret, is_https_or_loopback};

    #[test]
    fn a_generated_secret_is_accepted() {
        check_secret("secret", "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK").expect("accepted");
    }

    #[test]
    fn short_placeholder_and_repetitive_secrets_are_refused() {
        for secret in [
            "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9Eo",
            "change-me-to-a-long-random-string",
            "Change-Me-kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abcabcabcabcabcabcabcabcabcabcabcabcabc",
        ] {
            assert!(check_secret("secret", secret).is_err(), "{secret}");
        }
    }

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
