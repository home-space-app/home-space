//! Loading the tenants to serve: their rows from the database, their settings and secrets
//! from outside it.

use std::{collections::HashMap, future::Future, path::PathBuf};

use anyhow::{Context, Result};
use api::{SharedServices, TenantContext, TenantMode, Tenants};
use auth::{AuthConfig, AuthService, OidcConfig, OidcService};
use db::{Db, Tenant, TenantDomain, TenantId};
use tokio::task::JoinSet;

use crate::config::{
    Config, DEFAULT_OIDC_GROUPS_CLAIM, DEFAULT_OIDC_NAME, DEFAULT_OIDC_SCOPES, TenantConfig,
    TenantSettings,
};

/// Where per-tenant settings and secrets come from: environment variables now, a vault later.
pub trait TenantSecrets: Send + Sync {
    /// The value of the setting known to tenant 1 as the variable `name`, for `tenant`;
    /// `None` when it is unset or empty.
    fn get(
        &self,
        tenant: TenantId,
        name: &str,
    ) -> impl Future<Output = Result<Option<String>>> + Send;
}

/// Per-tenant settings from environment variables: `HOMESPACE_JWT_SECRET_2`,
/// `HOMESPACE_JWT_SECRET_2_FILE`, … Tenant 1 has the unsuffixed names. No other tenant falls
/// back to them, so a tenant without its own secret never signs with tenant 1's.
pub struct EnvSecrets;

impl EnvSecrets {
    /// The variable that holds `name` for `tenant`.
    pub fn var_name(tenant: TenantId, name: &str) -> String {
        if tenant == TenantId::DEFAULT {
            name.to_owned()
        } else if let Some(base) = name.strip_suffix("_FILE") {
            format!("{base}_{tenant}_FILE")
        } else {
            format!("{name}_{tenant}")
        }
    }
}

impl TenantSecrets for EnvSecrets {
    async fn get(&self, tenant: TenantId, name: &str) -> Result<Option<String>> {
        let var = Self::var_name(tenant, name);
        match std::env::var(&var) {
            Ok(value) if !value.is_empty() => Ok(Some(value)),
            Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => anyhow::bail!("{var} is not valid UTF-8"),
        }
    }
}

/// The settings of `tenant` as `secrets` has them.
pub async fn tenant_settings(
    secrets: &impl TenantSecrets,
    tenant: TenantId,
) -> Result<TenantSettings> {
    let get = |name| secrets.get(tenant, name);
    let password_login_var = EnvSecrets::var_name(tenant, "HOMESPACE_PASSWORD_LOGIN");
    let password_login = match get("HOMESPACE_PASSWORD_LOGIN").await?.as_deref() {
        None | Some("true") => true,
        Some("false") => false,
        Some(other) => anyhow::bail!("{password_login_var} must be true or false, not {other:?}"),
    };
    Ok(TenantSettings {
        suffix: EnvSecrets::var_name(tenant, ""),
        jwt_secret: get("HOMESPACE_JWT_SECRET").await?,
        jwt_secret_file: get("HOMESPACE_JWT_SECRET_FILE").await?.map(PathBuf::from),
        jwt_secret_previous: get("HOMESPACE_JWT_SECRET_PREVIOUS").await?,
        public_url: get("HOMESPACE_PUBLIC_URL").await?,
        oidc_issuer: get("HOMESPACE_OIDC_ISSUER").await?,
        oidc_client_id: get("HOMESPACE_OIDC_CLIENT_ID").await?,
        oidc_client_secret: get("HOMESPACE_OIDC_CLIENT_SECRET").await?,
        oidc_scopes: get("HOMESPACE_OIDC_SCOPES")
            .await?
            .unwrap_or_else(|| DEFAULT_OIDC_SCOPES.to_owned()),
        oidc_required_group: get("HOMESPACE_OIDC_REQUIRED_GROUP").await?,
        oidc_groups_claim: get("HOMESPACE_OIDC_GROUPS_CLAIM")
            .await?
            .unwrap_or_else(|| DEFAULT_OIDC_GROUPS_CLAIM.to_owned()),
        oidc_name: get("HOMESPACE_OIDC_NAME")
            .await?
            .unwrap_or_else(|| DEFAULT_OIDC_NAME.to_owned()),
        password_login,
    })
}

/// Loads the tenants to serve. `base` is tenant 1's auth service, built from the server's
/// own settings; the other tenants' services share its process-wide state.
///
/// In single mode the database must hold tenant 1 alone, and it must be active; anything
/// wrong with it stops the server, as before tenants. In host mode a tenant whose settings
/// are missing or invalid, or whose identity provider can't be reached, is left out and
/// logged, so that one tenant's mistake does not take the others down. Two tenants with the
/// same signing secret stop the server: a token of one would verify on the other.
pub async fn load(
    db: &Db,
    config: &Config,
    base: AuthService,
    secrets: &impl TenantSecrets,
    shared: &SharedServices,
) -> Result<Tenants> {
    let tenants = db.list_tenants().await.context("listing tenants")?;
    let domains = db
        .list_tenant_domains()
        .await
        .context("listing tenant domains")?;
    match config.tenant_mode {
        TenantMode::Single => load_single(config, base, &tenants, &domains, shared).await,
        TenantMode::Host => load_hosts(config, base, secrets, &tenants, &domains, shared).await,
    }
}

async fn load_single(
    config: &Config,
    base: AuthService,
    tenants: &[Tenant],
    domains: &[TenantDomain],
    shared: &SharedServices,
) -> Result<Tenants> {
    let tenant = single_tenant(tenants)?;
    let settings = config.tenant_settings();
    let tenant_config = settings.validate()?;
    let auth = match tenant_config.oidc.clone() {
        Some(oidc_config) => {
            let oidc = OidcService::discover(oidc_config)
                .await
                .context("discovering the OpenID Connect provider")?;
            tracing::info!(issuer = oidc.issuer(), "single sign-on enabled");
            base.with_oidc(oidc, tenant_config.password_login)
        }
        None => base,
    };
    Ok(Tenants::single(TenantContext::new(
        tenant.id,
        tenant.name.clone(),
        domains_of(domains, tenant.id),
        tenant_config.public_url,
        auth,
        shared,
    )))
}

/// Tenant 1, when it is the only tenant and active, as single mode needs.
fn single_tenant(tenants: &[Tenant]) -> Result<&Tenant> {
    match tenants {
        [tenant] if tenant.id == TenantId::DEFAULT => {
            anyhow::ensure!(
                tenant.is_active(),
                "tenant 1 is disabled or being deleted, so there is nothing to serve"
            );
            Ok(tenant)
        }
        [] | [_] => anyhow::bail!("tenant 1 is missing from the database"),
        _ => anyhow::bail!(
            "the database holds {} tenants; serve them with HOMESPACE_TENANT_MODE=host",
            tenants.len()
        ),
    }
}

async fn load_hosts(
    config: &Config,
    base: AuthService,
    secrets: &impl TenantSecrets,
    tenants: &[Tenant],
    domains: &[TenantDomain],
    shared: &SharedServices,
) -> Result<Tenants> {
    let configured = validate_tenants(config, secrets, tenants, domains).await;
    ensure_distinct_secrets(&configured)?;
    let providers = discover_all(&configured).await;
    let ready = contexts(config, &base, configured, providers, domains, shared);
    Ok(Tenants::new(TenantMode::Host, ready, domains))
}

/// The active tenants whose settings are complete and valid, with those settings. A tenant
/// left out is logged, so that one tenant's mistake does not take the others down.
async fn validate_tenants<'a>(
    config: &Config,
    secrets: &impl TenantSecrets,
    tenants: &'a [Tenant],
    domains: &[TenantDomain],
) -> Vec<(&'a Tenant, TenantConfig)> {
    let mut configured = Vec::new();
    for tenant in tenants {
        if !tenant.is_active() {
            tracing::info!(tenant = %tenant.id, "tenant is disabled; it is not served");
            continue;
        }
        match validate_tenant(config, secrets, tenant, domains).await {
            Ok(tenant_config) => configured.push((tenant, tenant_config)),
            Err(err) => tracing::error!(
                tenant = %tenant.id,
                error = format!("{err:#}"),
                "tenant settings are missing or invalid; it is not served"
            ),
        }
    }
    configured
}

/// The settings of `tenant`: tenant 1's from the server configuration, any other's from its
/// stored settings and secrets. Without a public URL, it is reached at its primary domain.
async fn validate_tenant(
    config: &Config,
    secrets: &impl TenantSecrets,
    tenant: &Tenant,
    domains: &[TenantDomain],
) -> Result<TenantConfig> {
    let primary = domains
        .iter()
        .find(|domain| domain.tenant_id == tenant.id && domain.is_primary);
    if primary.is_none() {
        tracing::warn!(tenant = %tenant.id, "tenant has no primary domain; nobody can reach it");
    }
    let mut settings = if tenant.id == TenantId::DEFAULT {
        config.tenant_settings()
    } else {
        tenant_settings(secrets, tenant.id).await?
    };
    if settings.public_url.as_deref().is_none_or(str::is_empty) {
        settings.public_url = primary.map(|domain| public_url(&domain.domain));
    }
    settings.validate()
}

/// The tenants ready to serve: each `configured` one whose identity provider, if it has one,
/// was discovered.
fn contexts(
    config: &Config,
    base: &AuthService,
    configured: Vec<(&Tenant, TenantConfig)>,
    mut providers: HashMap<TenantId, Result<OidcService>>,
    domains: &[TenantDomain],
    shared: &SharedServices,
) -> Vec<TenantContext> {
    let mut ready = Vec::new();
    for (tenant, tenant_config) in configured {
        let provider = providers.remove(&tenant.id);
        let Some(auth) = tenant_auth(config, base, tenant.id, &tenant_config, provider) else {
            continue;
        };
        tracing::info!(tenant = %tenant.id, name = tenant.name, "tenant loaded");
        ready.push(TenantContext::new(
            tenant.id,
            tenant.name.clone(),
            domains_of(domains, tenant.id),
            tenant_config.public_url,
            auth,
            shared,
        ));
    }
    ready
}

/// The auth service of a tenant, with its own keys and its identity provider when it has
/// one; `None`, logged, when the provider could not be discovered.
fn tenant_auth(
    config: &Config,
    base: &AuthService,
    tenant: TenantId,
    tenant_config: &TenantConfig,
    provider: Option<Result<OidcService>>,
) -> Option<AuthService> {
    let auth = base.for_tenant(AuthConfig {
        tenant,
        jwt_secret: tenant_config.jwt_secret.clone(),
        previous_jwt_secrets: tenant_config.previous_jwt_secrets.clone(),
        access_ttl: time::Duration::seconds(config.access_ttl_secs),
        refresh_ttl: time::Duration::seconds(config.refresh_ttl_secs),
    });
    match provider {
        None => Some(auth),
        Some(Ok(oidc)) => {
            tracing::info!(%tenant, issuer = oidc.issuer(), "single sign-on enabled");
            Some(auth.with_oidc(oidc, tenant_config.password_login))
        }
        Some(Err(err)) => {
            tracing::error!(
                %tenant,
                error = format!("{err:#}"),
                "discovering the OpenID Connect provider failed; the tenant is not served"
            );
            None
        }
    }
}

/// Discovers the identity provider of every tenant that has one, all at once.
async fn discover_all(
    configured: &[(&Tenant, TenantConfig)],
) -> HashMap<TenantId, Result<OidcService>> {
    let mut discoveries = JoinSet::new();
    for (tenant, tenant_config) in configured {
        if let Some(oidc_config) = tenant_config.oidc.clone() {
            let id = tenant.id;
            discoveries.spawn(async move { (id, discover(oidc_config).await) });
        }
    }
    let mut providers = HashMap::new();
    while let Some(joined) = discoveries.join_next().await {
        if let Ok((id, provider)) = joined {
            providers.insert(id, provider);
        }
    }
    providers
}

async fn discover(config: OidcConfig) -> Result<OidcService> {
    Ok(OidcService::discover(config).await?)
}

/// Refuses two tenants that share a signing secret, current or previous.
fn ensure_distinct_secrets(configured: &[(&Tenant, TenantConfig)]) -> Result<()> {
    let mut seen = HashMap::new();
    for (tenant, tenant_config) in configured {
        let secrets =
            std::iter::once(&tenant_config.jwt_secret).chain(&tenant_config.previous_jwt_secrets);
        for secret in secrets {
            if let Some(other) = seen.insert(secret.as_slice(), tenant.id)
                && other != tenant.id
            {
                anyhow::bail!(
                    "tenants {other} and {} have the same JWT secret; give each its own",
                    tenant.id
                );
            }
        }
    }
    Ok(())
}

/// The domains of `tenant`, the primary one first.
fn domains_of(domains: &[TenantDomain], tenant: TenantId) -> Vec<String> {
    let mut of_tenant = domains
        .iter()
        .filter(|domain| domain.tenant_id == tenant)
        .collect::<Vec<_>>();
    of_tenant.sort_by_key(|domain| !domain.is_primary);
    of_tenant
        .into_iter()
        .map(|domain| domain.domain.clone())
        .collect()
}

/// The public URL a tenant gets from its primary domain: https, except plain http to this
/// machine for local development.
fn public_url(domain: &str) -> String {
    let host = match domain.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => domain.split(':').next().unwrap_or_default(),
    };
    let scheme = if matches!(host, "localhost" | "127.0.0.1" | "::1") {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{domain}")
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::*;

    const SECRET_1: &str = "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK";
    const SECRET_2: &str = "Xw7pR2nLq9TzVb4KsYc1MdHg6JfAe3Uo8NiZ5tQrWmEx";

    /// Settings kept in a map by the variable name `EnvSecrets` would read.
    struct MapSecrets(HashMap<String, String>);

    impl TenantSecrets for MapSecrets {
        async fn get(&self, tenant: TenantId, name: &str) -> Result<Option<String>> {
            Ok(self.0.get(&EnvSecrets::var_name(tenant, name)).cloned())
        }
    }

    fn secrets(pairs: &[(&str, &str)]) -> MapSecrets {
        MapSecrets(
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    fn tenant(id: i32) -> Tenant {
        Tenant {
            id: TenantId(id),
            name: format!("Tenant {id}"),
            created_at: OffsetDateTime::UNIX_EPOCH,
            disabled_at: None,
            delete_at: None,
        }
    }

    fn checked(secret: &str, previous: Option<&str>) -> TenantConfig {
        TenantConfig {
            jwt_secret: secret.as_bytes().to_vec(),
            previous_jwt_secrets: previous
                .map(|secret| vec![secret.as_bytes().to_vec()])
                .unwrap_or_default(),
            public_url: None,
            oidc: None,
            password_login: true,
        }
    }

    #[test]
    fn tenant_one_has_the_unsuffixed_names() {
        assert_eq!(
            EnvSecrets::var_name(TenantId::DEFAULT, "HOMESPACE_JWT_SECRET"),
            "HOMESPACE_JWT_SECRET"
        );
        assert_eq!(
            EnvSecrets::var_name(TenantId(2), "HOMESPACE_JWT_SECRET"),
            "HOMESPACE_JWT_SECRET_2"
        );
        assert_eq!(
            EnvSecrets::var_name(TenantId(12), "HOMESPACE_JWT_SECRET_FILE"),
            "HOMESPACE_JWT_SECRET_12_FILE"
        );
    }

    #[tokio::test]
    async fn another_tenant_does_not_fall_back_to_tenant_ones_secret() {
        let secrets = secrets(&[("HOMESPACE_JWT_SECRET", SECRET_1)]);
        let settings = tenant_settings(&secrets, TenantId(2)).await.expect("read");
        let err = settings.validate().expect_err("no secret of its own");
        assert!(err.to_string().contains("HOMESPACE_JWT_SECRET_2"), "{err}");
    }

    #[tokio::test]
    async fn a_tenants_own_settings_are_read_and_checked() {
        let secrets = secrets(&[
            ("HOMESPACE_JWT_SECRET_2", SECRET_2),
            ("HOMESPACE_OIDC_ISSUER_2", "https://idp.other.org"),
            ("HOMESPACE_OIDC_CLIENT_ID_2", "homespace"),
            ("HOMESPACE_PUBLIC_URL_2", "https://cloud.other.org"),
            ("HOMESPACE_PASSWORD_LOGIN_2", "false"),
        ]);
        let settings = tenant_settings(&secrets, TenantId(2)).await.expect("read");
        let checked = settings.validate().expect("valid");
        assert_eq!(checked.jwt_secret, SECRET_2.as_bytes());
        assert!(!checked.password_login);
        let oidc = checked.oidc.expect("single sign-on");
        assert_eq!(oidc.issuer, "https://idp.other.org");
        assert_eq!(oidc.public_url, "https://cloud.other.org");
        assert_eq!(oidc.display_name, DEFAULT_OIDC_NAME);
    }

    #[tokio::test]
    async fn a_weak_secret_is_refused_for_any_tenant() {
        let secrets = secrets(&[(
            "HOMESPACE_JWT_SECRET_3",
            "change-me-change-me-change-me-1234",
        )]);
        let settings = tenant_settings(&secrets, TenantId(3)).await.expect("read");
        assert!(settings.validate().is_err());
    }

    #[tokio::test]
    async fn password_login_must_be_true_or_false() {
        let secrets = secrets(&[("HOMESPACE_PASSWORD_LOGIN_2", "no")]);
        assert!(tenant_settings(&secrets, TenantId(2)).await.is_err());
    }

    #[test]
    fn two_tenants_may_not_share_a_secret() {
        let one = tenant(1);
        let two = tenant(2);
        assert!(
            ensure_distinct_secrets(&[
                (&one, checked(SECRET_1, None)),
                (&two, checked(SECRET_2, None)),
            ])
            .is_ok()
        );
        assert!(
            ensure_distinct_secrets(&[
                (&one, checked(SECRET_1, None)),
                (&two, checked(SECRET_2, Some(SECRET_1))),
            ])
            .is_err()
        );
        assert!(
            ensure_distinct_secrets(&[(&one, checked(SECRET_1, Some(SECRET_1)))]).is_ok(),
            "a tenant may repeat its own secret"
        );
    }

    #[test]
    fn single_mode_needs_tenant_one_alone_and_active() {
        assert!(single_tenant(&[tenant(1)]).is_ok());
        assert!(single_tenant(&[]).is_err());
        assert!(single_tenant(&[tenant(2)]).is_err());
        assert!(single_tenant(&[tenant(1), tenant(2)]).is_err());
        let mut disabled = tenant(1);
        disabled.disabled_at = Some(OffsetDateTime::UNIX_EPOCH);
        assert!(single_tenant(&[disabled]).is_err());
    }

    #[test]
    fn the_public_url_comes_from_the_primary_domain() {
        assert_eq!(public_url("files.example.com"), "https://files.example.com");
        assert_eq!(public_url("localhost:8080"), "http://localhost:8080");
        assert_eq!(public_url("[::1]:8080"), "http://[::1]:8080");
        assert_eq!(
            public_url("files.example.com:8443"),
            "https://files.example.com:8443"
        );
    }

    #[test]
    fn domains_are_listed_primary_first() {
        let domain = |name: &str, primary| TenantDomain {
            domain: name.to_owned(),
            tenant_id: TenantId(2),
            is_primary: primary,
        };
        let domains = [
            domain("old.example.com", false),
            domain("files.example.com", true),
        ];
        assert_eq!(
            domains_of(&domains, TenantId(2)),
            ["files.example.com", "old.example.com"]
        );
        assert!(domains_of(&domains, TenantId(1)).is_empty());
    }
}
