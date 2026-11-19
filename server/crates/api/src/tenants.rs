//! The tenants this server serves and how a request finds its own.

use std::{collections::HashMap, fmt, str::FromStr, sync::Arc};

use accounts::{AccountService, DeviceService};
use auth::AuthService;
use db::{Db, TenantDomain, TenantId};
use events::Events;
use files::{NodeService, UploadService};
use notifications::Hub;
use shares::ShareService;

/// How requests find their tenant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TenantMode {
    /// Every request is tenant 1, whatever host it was sent to: the server as it was before
    /// tenants, reachable by IP address, `localhost` and any name at once.
    #[default]
    Single,
    /// Each request is the tenant whose domain is the request's host.
    Host,
}

impl FromStr for TenantMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "single" => Ok(Self::Single),
            "host" => Ok(Self::Host),
            other => Err(format!("unknown tenant mode {other:?}; use single or host")),
        }
    }
}

impl fmt::Display for TenantMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Single => "single",
            Self::Host => "host",
        })
    }
}

/// A tenant ready to serve requests.
#[derive(Clone, Debug)]
pub struct TenantContext {
    pub id: TenantId,
    pub name: String,
    /// The domains it answers on, the primary one first; empty in single mode when none is
    /// recorded.
    pub domains: Vec<String>,
    /// Where it is reached from outside, e.g. `https://files.example.com`, when known.
    pub public_url: Option<String>,
    /// Signs its users in with its own keys and identity provider.
    pub auth: AuthService,
    pub accounts: AccountService,
    pub devices: DeviceService,
    pub shares: ShareService,
}

/// The services of the whole server that each tenant's own services are built on.
#[derive(Clone, Debug)]
pub struct SharedServices {
    pub db: Db,
    pub nodes: NodeService,
    pub uploads: UploadService,
    pub hub: Hub,
    pub events: Events,
    /// How long after an administrator deletes an account it is deleted.
    pub deletion_delay: time::Duration,
}

impl TenantContext {
    /// A tenant whose users `auth` signs in, with its services built on `shared`.
    pub fn new(
        id: TenantId,
        name: String,
        domains: Vec<String>,
        public_url: Option<String>,
        auth: AuthService,
        shared: &SharedServices,
    ) -> Self {
        let SharedServices {
            db,
            nodes,
            uploads,
            hub,
            events,
            deletion_delay,
        } = shared.clone();
        Self {
            id,
            name,
            domains,
            public_url,
            accounts: AccountService::new(db.clone(), auth.clone(), deletion_delay),
            devices: DeviceService::new(db.clone(), auth.clone(), uploads, hub, events),
            shares: ShareService::new(db, id, auth.share_passwords(), nodes),
            auth,
        }
    }
}

/// What a request's host leads to.
#[derive(Clone, Debug)]
pub enum Resolved {
    Ready(Arc<TenantContext>),
    /// A tenant that is disabled, being deleted, or whose settings are missing or invalid.
    Unavailable(TenantId),
    /// No tenant answers on this host.
    Unknown,
}

/// The tenants of this server, loaded at start-up; cheap to clone.
#[derive(Clone, Debug)]
pub struct Tenants(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    mode: TenantMode,
    ready: HashMap<TenantId, Arc<TenantContext>>,
    by_domain: HashMap<String, TenantId>,
}

impl Tenants {
    /// The registry of a server in `mode` serving the `ready` tenants. The domains of any
    /// other tenant lead to [`Resolved::Unavailable`].
    pub fn new(mode: TenantMode, ready: Vec<TenantContext>, domains: &[TenantDomain]) -> Self {
        Self(Arc::new(Inner {
            mode,
            ready: ready
                .into_iter()
                .map(|tenant| (tenant.id, Arc::new(tenant)))
                .collect(),
            by_domain: domains
                .iter()
                .map(|domain| (domain.domain.clone(), domain.tenant_id))
                .collect(),
        }))
    }

    /// A single-tenant server: every request is `tenant`.
    pub fn single(tenant: TenantContext) -> Self {
        Self::new(TenantMode::Single, vec![tenant], &[])
    }

    pub fn mode(&self) -> TenantMode {
        self.0.mode
    }

    /// A tenant that is ready to serve, by id.
    pub fn get(&self, id: TenantId) -> Option<Arc<TenantContext>> {
        self.0.ready.get(&id).cloned()
    }

    /// The tenants ready to serve, by id.
    pub fn ready(&self) -> impl Iterator<Item = &Arc<TenantContext>> {
        let mut ready = self.0.ready.values().collect::<Vec<_>>();
        ready.sort_by_key(|tenant| tenant.id);
        ready.into_iter()
    }

    /// The tenant a request sent to `host` (its `Host` header) is for.
    pub fn resolve(&self, host: Option<&str>) -> Resolved {
        let id = match self.0.mode {
            TenantMode::Single => TenantId::DEFAULT,
            TenantMode::Host => {
                let Some(id) = host
                    .and_then(normalize_host)
                    .and_then(|host| self.0.by_domain.get(&host).copied())
                else {
                    return Resolved::Unknown;
                };
                id
            }
        };
        match self.get(id) {
            Some(tenant) => Resolved::Ready(tenant),
            None => Resolved::Unavailable(id),
        }
    }
}

/// `host` as tenant domains are stored: lower-case, without a trailing dot and without the
/// default ports 80 and 443. `None` for what cannot be a host name or address; names outside
/// ASCII have to come in their punycode form, as browsers send them.
pub fn normalize_host(host: &str) -> Option<String> {
    let host = host.trim().to_ascii_lowercase();
    let (name, port) = if host.starts_with('[') {
        let end = host.find(']')?;
        let rest = &host[end + 1..];
        let port = match rest {
            "" => None,
            rest => Some(rest.strip_prefix(':')?),
        };
        (&host[..=end], port)
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (host.as_str(), None),
        }
    };
    let name = name.strip_suffix('.').unwrap_or(name);
    let valid_name = match name
        .strip_prefix('[')
        .and_then(|name| name.strip_suffix(']'))
    {
        Some(address) => {
            !address.is_empty()
                && address
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'))
        }
        None => {
            !name.is_empty()
                && name.len() <= 253
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        }
    };
    if !valid_name {
        return None;
    }
    match port {
        None | Some("80" | "443") => Some(name.to_owned()),
        Some(port) if !port.is_empty() && port.len() <= 5 && port.parse::<u16>().is_ok() => {
            Some(format!("{name}:{port}"))
        }
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_lower_cased_without_default_ports() {
        for (host, normalized) in [
            ("Files.Example.com", "files.example.com"),
            ("files.example.com:443", "files.example.com"),
            ("files.example.com:80", "files.example.com"),
            ("files.example.com.", "files.example.com"),
            ("files.example.com:8443", "files.example.com:8443"),
            ("localhost:8080", "localhost:8080"),
            ("192.0.2.1", "192.0.2.1"),
            ("[::1]:8080", "[::1]:8080"),
            ("[::1]", "[::1]"),
            ("xn--bcher-kva.example", "xn--bcher-kva.example"),
        ] {
            assert_eq!(normalize_host(host).as_deref(), Some(normalized), "{host}");
        }
    }

    #[test]
    fn what_is_not_a_host_is_refused() {
        for host in [
            "",
            ":443",
            "files.example.com:",
            "files.example.com:http",
            "files.example.com:99999",
            "user@files.example.com",
            "files.example.com/path",
            "bücher.example",
            "[::1",
            "[::1]x",
            "::1",
            "a:b:80",
            "[]",
        ] {
            assert_eq!(normalize_host(host), None, "{host}");
        }
    }

    fn domain(domain: &str, tenant: i32) -> TenantDomain {
        TenantDomain {
            domain: domain.to_owned(),
            tenant_id: TenantId(tenant),
            is_primary: true,
        }
    }

    #[test]
    fn host_mode_finds_the_tenant_by_domain() {
        let tenants = Tenants::new(
            TenantMode::Host,
            Vec::new(),
            &[domain("files.example.com", 1), domain("cloud.other.org", 2)],
        );
        assert!(matches!(
            tenants.resolve(Some("Cloud.Other.org:443")),
            Resolved::Unavailable(TenantId(2))
        ));
        assert!(matches!(
            tenants.resolve(Some("unknown.example")),
            Resolved::Unknown
        ));
        assert!(matches!(tenants.resolve(None), Resolved::Unknown));
    }

    #[test]
    fn single_mode_ignores_the_host() {
        let tenants = Tenants::new(
            TenantMode::Single,
            Vec::new(),
            &[domain("cloud.other.org", 2)],
        );
        for host in [Some("cloud.other.org"), Some("192.0.2.1"), None] {
            assert!(
                matches!(
                    tenants.resolve(host),
                    Resolved::Unavailable(TenantId::DEFAULT)
                ),
                "{host:?}"
            );
        }
    }

    #[test]
    fn tenant_modes_parse() {
        assert_eq!("single".parse(), Ok(TenantMode::Single));
        assert_eq!("host".parse(), Ok(TenantMode::Host));
        assert!("Host".parse::<TenantMode>().is_err());
    }
}
