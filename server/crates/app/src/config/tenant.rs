//! The settings that may differ per tenant.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};

use auth::{
    is_https_or_loopback,
    secrets::{check_secret, value_or_file},
};

use super::checks::non_empty;

/// The settings that may differ per tenant, as given and not yet checked. Tenant 1 has the
/// server's unsuffixed settings; tenant *n* has the same names with `_n` appended.
#[derive(Clone, Debug, Default)]
pub struct TenantSettings {
    /// Appended to the variable names in messages: empty for tenant 1, `_2` for tenant 2.
    pub suffix: String,
    pub jwt_secret: Option<String>,
    pub jwt_secret_file: Option<PathBuf>,
    /// No longer read: refused, so that a rotated-out secret cannot go on working.
    pub jwt_secret_previous: Option<String>,
    pub secrets_key: Option<String>,
    pub secrets_key_file: Option<PathBuf>,
    /// No longer read: refused; `homespace-ctl secrets rotate` seals again what it sealed.
    pub secrets_key_previous: Option<String>,
    pub public_url: Option<String>,
    pub oidc_issuer: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    pub oidc_client_secret_file: Option<PathBuf>,
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
    /// Seals authenticator app secrets and password-folder keys; `None` without
    /// `HOMESPACE_SECRETS_KEY`.
    pub secrets_key: Option<Arc<auth::SecretBox>>,
    pub public_url: Option<String>,
    pub oidc: Option<auth::OidcConfig>,
    pub password_login: bool,
}

impl TenantSettings {
    /// Checks the settings, refusing those that would make the tenant insecure.
    pub fn validate(&self) -> Result<TenantConfig> {
        self.refuse_previous_secrets()?;
        Ok(TenantConfig {
            jwt_secret: self.jwt_secret()?,
            secrets_key: self.secrets_key()?.map(Arc::new),
            public_url: non_empty(&self.public_url).map(str::to_owned),
            oidc: self.oidc()?,
            password_login: self.password_login,
        })
    }

    /// Refuses the `_PREVIOUS` secrets of earlier versions: a secret kept there went on working
    /// after it was rotated out, leaked or not.
    fn refuse_previous_secrets(&self) -> Result<()> {
        if non_empty(&self.jwt_secret_previous).is_some() {
            anyhow::bail!(
                "{} is no longer read: a rotated-out secret must stop working. Remove it; rotate \
                 secrets with `homespace-ctl secrets rotate`",
                self.var("HOMESPACE_JWT_SECRET_PREVIOUS")
            );
        }
        if non_empty(&self.secrets_key_previous).is_some() {
            anyhow::bail!(
                "{} is no longer read. Run `homespace-ctl secrets rotate` with it still set: it \
                 seals again with a new key everything still sealed with it. Then remove it",
                self.var("HOMESPACE_SECRETS_KEY_PREVIOUS")
            );
        }
        Ok(())
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
        let client_secret = value_or_file(
            &self.var("HOMESPACE_OIDC_CLIENT_SECRET"),
            self.oidc_client_secret.as_deref(),
            &self.file_var("HOMESPACE_OIDC_CLIENT_SECRET"),
            self.oidc_client_secret_file.as_deref(),
        )?;
        Ok(Some(auth::OidcConfig {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            client_secret,
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

    /// The key that seals authenticator app secrets; `None` when no key is set. Optional:
    /// without it no authenticator app can be added, and nothing else changes.
    pub fn secrets_key(&self) -> Result<Option<auth::SecretBox>> {
        let Some(current) = value_or_file(
            &self.var("HOMESPACE_SECRETS_KEY"),
            self.secrets_key.as_deref(),
            &self.file_var("HOMESPACE_SECRETS_KEY"),
            self.secrets_key_file.as_deref(),
        )?
        else {
            return Ok(None);
        };
        check_secret(&self.var("HOMESPACE_SECRETS_KEY"), &current)?;
        let reuses_jwt_secret = value_or_file(
            &self.var("HOMESPACE_JWT_SECRET"),
            self.jwt_secret.as_deref(),
            &self.file_var("HOMESPACE_JWT_SECRET"),
            self.jwt_secret_file.as_deref(),
        )
        .ok()
        .flatten()
        .is_some_and(|jwt| jwt == current);
        anyhow::ensure!(
            !reuses_jwt_secret,
            "{} must differ from the JWT secret: a token signing secret must not open sealed \
             secrets",
            self.var("HOMESPACE_SECRETS_KEY")
        );
        Ok(Some(auth::SecretBox::new(&current)))
    }

    /// The token signing secret.
    pub fn jwt_secret(&self) -> Result<Vec<u8>> {
        let current = value_or_file(
            &self.var("HOMESPACE_JWT_SECRET"),
            self.jwt_secret.as_deref(),
            &self.file_var("HOMESPACE_JWT_SECRET"),
            self.jwt_secret_file.as_deref(),
        )?
        .with_context(|| {
            format!(
                "{} or {} must be set",
                self.var("HOMESPACE_JWT_SECRET"),
                self.file_var("HOMESPACE_JWT_SECRET")
            )
        })?;
        let name = if self.suffix.is_empty() {
            "the JWT secret".to_owned()
        } else {
            self.var("HOMESPACE_JWT_SECRET")
        };
        check_secret(&name, &current)?;
        Ok(current.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn oidc_settings() -> TenantSettings {
        TenantSettings {
            public_url: Some("https://files.example.com".to_owned()),
            oidc_issuer: Some("https://idp.example.com".to_owned()),
            oidc_client_id: Some("homespace".to_owned()),
            ..TenantSettings::default()
        }
    }

    #[test]
    fn the_oidc_client_secret_can_come_from_a_file() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        writeln!(file, "from-file").expect("write");
        let settings = TenantSettings {
            oidc_client_secret_file: Some(file.path().to_owned()),
            ..oidc_settings()
        };
        let oidc = settings.oidc().expect("valid").expect("configured");
        assert_eq!(oidc.client_secret.as_deref(), Some("from-file"));
    }

    #[test]
    fn the_oidc_client_secret_and_its_file_are_not_both_accepted() {
        let settings = TenantSettings {
            oidc_client_secret: Some("value".to_owned()),
            oidc_client_secret_file: Some(PathBuf::from("/run/secrets/oidc")),
            ..oidc_settings()
        };
        let err = settings.oidc().unwrap_err().to_string();
        assert!(err.contains("HOMESPACE_OIDC_CLIENT_SECRET_FILE"), "{err}");
    }

    const KEY: &str = "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK";

    #[test]
    fn the_secrets_key_is_optional_but_must_be_strong_and_its_own() {
        assert!(
            TenantSettings::default()
                .secrets_key()
                .expect("valid")
                .is_none()
        );
        let weak = TenantSettings {
            secrets_key: Some("short".to_owned()),
            ..TenantSettings::default()
        };
        assert!(weak.secrets_key().is_err());
        let reused = TenantSettings {
            secrets_key: Some(KEY.to_owned()),
            jwt_secret: Some(KEY.to_owned()),
            ..TenantSettings::default()
        };
        let err = reused.secrets_key().unwrap_err().to_string();
        assert!(err.contains("JWT secret"), "{err}");
        let own = TenantSettings {
            secrets_key: Some(KEY.to_owned()),
            ..TenantSettings::default()
        };
        assert!(own.secrets_key().expect("valid").is_some());
    }

    #[test]
    fn previous_secrets_are_refused() {
        let valid = TenantSettings {
            jwt_secret: Some("Xw7pR2nLq9TzVb4KsYc1MdHg6JfAe3Uo8NiZ5tQrWmEx".to_owned()),
            secrets_key: Some(KEY.to_owned()),
            password_login: true,
            ..TenantSettings::default()
        };
        valid.validate().expect("valid");
        let jwt = TenantSettings {
            jwt_secret_previous: Some("anything".to_owned()),
            ..valid.clone()
        };
        let err = jwt.validate().unwrap_err().to_string();
        assert!(err.contains("HOMESPACE_JWT_SECRET_PREVIOUS"), "{err}");
        let key = TenantSettings {
            secrets_key_previous: Some("anything".to_owned()),
            ..valid
        };
        let err = key.validate().unwrap_err().to_string();
        assert!(err.contains("secrets rotate"), "{err}");
    }
}
