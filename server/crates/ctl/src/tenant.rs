//! Finding the tenant a command is about from `--tenant`.

use anyhow::{Context, Result};

use db::{Db, TenantDomain, TenantId};

/// The tenant `arg` names, by id or domain. Without `arg`, tenant 1 while it is the only
/// tenant; once there are several, `--tenant` is required so that a script cannot change the
/// wrong one.
pub async fn resolve(db: &Db, arg: Option<&str>) -> Result<TenantId> {
    let tenants: Vec<TenantId> = db
        .list_tenants()
        .await
        .context("listing tenants")?
        .into_iter()
        .map(|tenant| tenant.id)
        .collect();
    let domains = db
        .list_tenant_domains()
        .await
        .context("listing tenant domains")?;
    pick(arg, &tenants, &domains)
}

fn pick(arg: Option<&str>, tenants: &[TenantId], domains: &[TenantDomain]) -> Result<TenantId> {
    match arg {
        None => match tenants {
            [] | [TenantId::DEFAULT] => Ok(TenantId::DEFAULT),
            _ => anyhow::bail!("there are {} tenants: pass --tenant", tenants.len()),
        },
        Some(arg) => by_id(arg, tenants)
            .or_else(|| by_domain(arg, domains))
            .with_context(|| format!("no such tenant: {arg}")),
    }
}

fn by_id(arg: &str, tenants: &[TenantId]) -> Option<TenantId> {
    arg.parse()
        .ok()
        .map(TenantId)
        .filter(|id| tenants.contains(id))
}

fn by_domain(arg: &str, domains: &[TenantDomain]) -> Option<TenantId> {
    let wanted = arg.to_ascii_lowercase();
    domains
        .iter()
        .find(|domain| domain.domain == wanted)
        .map(|domain| domain.tenant_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTHER: TenantId = TenantId(2);

    fn domains() -> Vec<TenantDomain> {
        vec![
            TenantDomain {
                domain: "files.example.com".to_owned(),
                tenant_id: TenantId::DEFAULT,
                is_primary: true,
            },
            TenantDomain {
                domain: "cloud.other.org".to_owned(),
                tenant_id: OTHER,
                is_primary: true,
            },
        ]
    }

    #[test]
    fn defaults_to_tenant_one_when_it_is_the_only_tenant() {
        let picked = pick(None, &[TenantId::DEFAULT], &[]).unwrap();
        assert_eq!(picked, TenantId::DEFAULT);
    }

    #[test]
    fn requires_a_tenant_once_there_are_several() {
        let err = pick(None, &[TenantId::DEFAULT, OTHER], &domains()).unwrap_err();
        assert!(err.to_string().contains("--tenant"));
    }

    #[test]
    fn finds_a_tenant_by_id() {
        let picked = pick(Some("2"), &[TenantId::DEFAULT, OTHER], &domains()).unwrap();
        assert_eq!(picked, OTHER);
    }

    #[test]
    fn finds_a_tenant_by_domain_whatever_its_case() {
        let picked = pick(
            Some("Cloud.Other.ORG"),
            &[TenantId::DEFAULT, OTHER],
            &domains(),
        );
        assert_eq!(picked.unwrap(), OTHER);
    }

    #[test]
    fn refuses_an_unknown_id() {
        assert!(pick(Some("3"), &[TenantId::DEFAULT, OTHER], &domains()).is_err());
    }

    #[test]
    fn refuses_an_unknown_domain() {
        assert!(pick(Some("nope.example.com"), &[TenantId::DEFAULT], &domains()).is_err());
    }
}
