//! The settings that may differ per tenant.

use std::path::PathBuf;

use anyhow::{Context, Result};

use super::checks::{check_secret, is_https_or_loopback, non_empty};

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
