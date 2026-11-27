//! Counting wrong passwords, so that guessing one takes ever longer.
//!
//! An attempt is admitted against one or more counters, then settled with how the password
//! check went: a success starts the count again, a wrong password stays counted, and a check
//! that could not be made (the server was busy) is taken back.
//!
//! Started sign-ins are counted the same way, every one of them, since each leaves a row
//! behind until it expires.

use std::net::{IpAddr, Ipv6Addr};

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use db::{Db, TenantId};

use crate::AuthError;

/// How long failed sign-ins of a username that is not blocked are remembered.
pub const LOGIN_ATTEMPTS_MEMORY: Duration = Duration::days(1);
/// Wrong passwords a username may get before attempts for it have to wait.
pub(crate) const FREE_LOGIN_FAILURES: u32 = 3;
/// The first wait, which doubles with every further wrong password.
pub(crate) const FIRST_LOGIN_BLOCK: Duration = Duration::seconds(30);
/// The longest wait, so that someone guessing cannot lock the owner out for good.
pub(crate) const MAX_LOGIN_BLOCK: Duration = Duration::hours(24);
/// The username that wrong passwords from one address are counted under whatever name they
/// were for. No user has it, and a sign-in with it is refused before anything is counted.
const ANY_USERNAME: &str = "";
/// Wrong passwords one address may send, over all usernames, before it has to wait. Well
/// above what the people of one household mistype, since they may share an address.
pub(crate) const FREE_ADDRESS_FAILURES: u32 = 20;
/// The longest wait of an address. Short, because it holds up everyone who shares it.
pub(crate) const MAX_ADDRESS_BLOCK: Duration = Duration::minutes(15);
/// How long after its last wrong password an address starts from nothing again.
const ADDRESS_ATTEMPTS_MEMORY: Duration = Duration::hours(1);
/// The source that wrong guesses at one secret are counted under from every source together.
/// No address is written like it, so it never mixes with the count of one.
const ALL_SOURCES: &str = "*";
/// Wrong guesses one secret, such as a folder's password, may get from all sources together
/// before every attempt at it waits. Well above what its owner mistypes on their devices, since
/// only the owner's own sessions can try it.
pub(crate) const FREE_SECRET_FAILURES: u32 = 10;
/// The longest wait of a secret over all sources. Short, because it holds up the owner too,
/// and it is what keeps someone guessing from many addresses at one try an hour.
pub(crate) const MAX_SECRET_BLOCK: Duration = Duration::hours(1);
/// The key started sign-ins from one address are counted under. No user has it: it is only
/// ever counted under [`TenantId::SHARED`], which no user belongs to.
const SIGN_IN_STARTS: &str = "sign-in-starts";
/// Sign-ins one address may start before it has to wait: far more than a household signing
/// in its apps, each start being one click on "Sign in".
pub(crate) const FREE_SIGN_IN_STARTS: u32 = 30;
/// How long after its last start an address starts from nothing again: as long as a started
/// sign-in waits for its answer.
const SIGN_IN_STARTS_MEMORY: Duration = Duration::minutes(10);
/// The longest wait of an address that starts too many sign-ins.
const MAX_SIGN_IN_STARTS_BLOCK: Duration = Duration::minutes(15);
/// Leading bits of an IPv6 address that name a network; one subscriber usually owns them all.
const IPV6_NETWORK_BITS: u32 = 64;

/// One count of wrong passwords.
#[derive(Clone, Debug)]
pub(crate) struct Counter {
    tenant: TenantId,
    key: String,
    source: String,
    scope: Scope,
}

#[derive(Clone, Copy, Debug)]
enum Scope {
    /// One username, or another secret such as a share link's password, from one source.
    Key,
    /// One address, over every username of every tenant.
    Address,
    /// One secret of one tenant, from every source together.
    AllSources,
    /// Sign-ins started from one address, in the browser or with single sign-on.
    Starts,
}

impl Counter {
    /// Wrong passwords for `key` of `tenant` from `source`; the right one starts it again.
    pub(crate) fn key(tenant: TenantId, key: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            tenant,
            key: key.into(),
            source: source.into(),
            scope: Scope::Key,
        }
    }

    /// Wrong passwords from `source`, whatever they were for. The right password only takes
    /// its own attempt back, so an address guessing many names keeps waiting.
    pub(crate) fn address(source: impl Into<String>) -> Self {
        Self {
            tenant: TenantId::SHARED,
            key: ANY_USERNAME.to_owned(),
            source: source.into(),
            scope: Scope::Address,
        }
    }

    /// Wrong guesses at `key` of `tenant` from every source together, so that spreading them
    /// over many addresses gains nothing. The right one only takes its own attempt back, so
    /// the owner getting it right does not start a guesser afresh.
    pub(crate) fn all_sources(tenant: TenantId, key: impl Into<String>) -> Self {
        Self {
            tenant,
            key: key.into(),
            source: ALL_SOURCES.to_owned(),
            scope: Scope::AllSources,
        }
    }

    /// Sign-ins started from `source`, over every tenant. Each start stays counted, whether
    /// or not it is ever finished.
    pub(crate) fn sign_in_starts(source: impl Into<String>) -> Self {
        Self {
            tenant: TenantId::SHARED,
            key: SIGN_IN_STARTS.to_owned(),
            source: source.into(),
            scope: Scope::Starts,
        }
    }

    fn memory(&self) -> Duration {
        match self.scope {
            Scope::Key | Scope::AllSources => LOGIN_ATTEMPTS_MEMORY,
            Scope::Address => ADDRESS_ATTEMPTS_MEMORY,
            Scope::Starts => SIGN_IN_STARTS_MEMORY,
        }
    }

    fn block(&self) -> fn(u32) -> Option<Duration> {
        match self.scope {
            Scope::Key => login_block,
            Scope::Address => address_block,
            Scope::AllSources => secret_block,
            Scope::Starts => sign_in_starts_block,
        }
    }
}

/// Admits password attempts while their counters allow.
#[derive(Clone, Debug)]
pub(crate) struct LoginThrottle {
    db: Db,
}

impl LoginThrottle {
    pub(crate) fn new(db: Db) -> Self {
        Self { db }
    }

    /// Counts an attempt against each of `counters` in order. When one makes it wait, the
    /// attempt is taken back from those before it and refused with the wait.
    pub(crate) async fn admit(&self, counters: Vec<Counter>) -> Result<Attempt, AuthError> {
        let mut admitted = Attempt {
            db: self.db.clone(),
            counters: Vec::with_capacity(counters.len()),
        };
        for counter in counters {
            let begun = self
                .db
                .begin_login_attempt(
                    counter.tenant,
                    &counter.key,
                    &counter.source,
                    counter.memory(),
                    counter.block(),
                )
                .await?;
            match begun {
                Ok(()) => admitted.counters.push(counter),
                Err(until) => {
                    admitted.take_back().await?;
                    return Err(too_many_attempts(until));
                }
            }
        }
        Ok(admitted)
    }
}

/// An attempt the throttle let through; [`settle`](Self::settle) it with the outcome.
#[derive(Debug)]
#[must_use = "an admitted attempt is settled with how the password check went"]
pub(crate) struct Attempt {
    db: Db,
    counters: Vec<Counter>,
}

impl Attempt {
    /// Settles the attempt with how the check went, and passes the outcome on: a success
    /// starts the counts again, an error `is_wrong` calls a wrong password stays counted, and
    /// any other error means the password was not checked, so the attempt is taken back.
    pub(crate) async fn settle<T>(
        self,
        outcome: Result<T, AuthError>,
        is_wrong: impl Fn(&AuthError) -> bool,
    ) -> Result<T, AuthError> {
        match &outcome {
            Ok(_) => self.succeeded().await?,
            Err(err) if is_wrong(err) => {}
            Err(_) => self.take_back().await?,
        }
        outcome
    }

    /// Leaves the attempt counted whatever comes of it, as a started sign-in is.
    pub(crate) fn keep(self) {}

    async fn succeeded(&self) -> Result<(), AuthError> {
        for counter in &self.counters {
            match counter.scope {
                Scope::Key => {
                    self.db
                        .clear_login_attempts(counter.tenant, &counter.key, &counter.source)
                        .await?;
                }
                Scope::Address | Scope::AllSources => self.take_back_one(counter).await?,
                Scope::Starts => {}
            }
        }
        Ok(())
    }

    async fn take_back(&self) -> Result<(), AuthError> {
        for counter in &self.counters {
            self.take_back_one(counter).await?;
        }
        Ok(())
    }

    async fn take_back_one(&self, counter: &Counter) -> Result<(), AuthError> {
        self.db
            .take_back_login_attempt(
                counter.tenant,
                &counter.key,
                &counter.source,
                counter.block(),
            )
            .await?;
        Ok(())
    }
}

/// The refusal of an attempt that must wait until `until`.
fn too_many_attempts(until: OffsetDateTime) -> AuthError {
    let wait = until - OffsetDateTime::now_utc();
    AuthError::TooManyAttempts {
        retry_after: wait.whole_seconds().max(0).unsigned_abs() + 1,
    }
}

/// How long the next sign-in attempt must wait after `failures` wrong passwords in a row:
/// not at all for the first [`FREE_LOGIN_FAILURES`], then 30 seconds, 1 minute, 2 minutes and
/// so on, up to [`MAX_LOGIN_BLOCK`].
pub(crate) fn login_block(failures: u32) -> Option<Duration> {
    doubling_block(failures, FREE_LOGIN_FAILURES, MAX_LOGIN_BLOCK)
}

/// How long an address must wait after `failures` wrong passwords, whatever the usernames:
/// not at all for the first [`FREE_ADDRESS_FAILURES`], then in the same steps as
/// [`login_block`], up to [`MAX_ADDRESS_BLOCK`].
pub(crate) fn address_block(failures: u32) -> Option<Duration> {
    doubling_block(failures, FREE_ADDRESS_FAILURES, MAX_ADDRESS_BLOCK)
}

/// How long every attempt at a secret must wait after `failures` wrong guesses from all
/// sources together: not at all for the first [`FREE_SECRET_FAILURES`], then in the same steps
/// as [`login_block`], up to [`MAX_SECRET_BLOCK`].
pub(crate) fn secret_block(failures: u32) -> Option<Duration> {
    doubling_block(failures, FREE_SECRET_FAILURES, MAX_SECRET_BLOCK)
}

/// How long an address must wait after starting `starts` sign-ins: not at all for the first
/// [`FREE_SIGN_IN_STARTS`], then in the same steps as [`login_block`], up to 15 minutes.
pub(crate) fn sign_in_starts_block(starts: u32) -> Option<Duration> {
    doubling_block(starts, FREE_SIGN_IN_STARTS, MAX_SIGN_IN_STARTS_BLOCK)
}

/// No wait for the first `free` failures, then [`FIRST_LOGIN_BLOCK`], doubled with every
/// further one, up to `longest`.
fn doubling_block(failures: u32, free: u32, longest: Duration) -> Option<Duration> {
    let doublings = failures.checked_sub(free)?;
    let factor = 1i32.checked_shl(doublings).filter(|factor| *factor > 0);
    let wait = factor
        .and_then(|factor| FIRST_LOGIN_BLOCK.checked_mul(factor))
        .unwrap_or(longest);
    Some(wait.min(longest))
}

/// The source of attempts made through a session, such as confirming it is the user.
pub(crate) fn session_source(session_id: Uuid) -> String {
    format!("session:{session_id}")
}

/// The source of attempts from a device the user signed in from before.
pub(crate) fn device_source(device_id: Uuid) -> String {
    format!("device:{device_id}")
}

/// What failed sign-ins from `address` are counted under: the address itself for IPv4 and
/// its network for IPv6, where a single client can pick among more addresses than could
/// ever be blocked one by one.
pub(crate) fn attempt_source(address: IpAddr) -> String {
    match address.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let network = u128::from(v6) & (u128::MAX << (u128::BITS - IPV6_NETWORK_BITS));
            format!("{}/{IPV6_NETWORK_BITS}", Ipv6Addr::from(network))
        }
    }
}
