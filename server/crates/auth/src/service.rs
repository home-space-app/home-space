//! The authentication of one tenant, as the API uses it. The work is done by the parts it
//! is made of: [`PasswordHasher`], [`LoginThrottle`], [`Sessions`], [`Tickets`] and
//! [`OidcLogin`].

use std::net::IpAddr;

use time::Duration;
use tokio::sync::broadcast;
use uuid::Uuid;

use db::{Db, DbError, TenantId, User, UserId};
use protocol::{
    DeviceInfo, OidcProvider, Providers, TicketRequest, TicketResponse, TokenPair,
    rules::MAX_USERNAME_LEN,
};

use crate::{
    AuthError, Claims, OidcService, SharePasswords,
    hasher::PasswordHasher,
    oidc_login::OidcLogin,
    password::MAX_PASSWORD_LEN,
    sessions::Sessions,
    throttle::{Counter, LoginThrottle, attempt_source},
    tickets::Tickets,
    token::Keys,
};

/// Token settings.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// The tenant this service signs users of that tenant in to.
    pub tenant: TenantId,
    pub jwt_secret: Vec<u8>,
    /// Earlier secrets that still verify tokens, for rotating `jwt_secret`.
    pub previous_jwt_secrets: Vec<Vec<u8>>,
    pub access_ttl: Duration,
    pub refresh_ttl: Duration,
}

impl AuthConfig {
    fn keys(&self) -> Keys {
        Keys::new(
            self.tenant,
            self.jwt_secret.clone(),
            self.previous_jwt_secrets.clone(),
        )
    }

    fn ttls(&self) -> (Duration, Duration) {
        (self.access_ttl, self.refresh_ttl)
    }
}

/// A valid access token: its claims and the device of its session.
#[derive(Clone, Copy, Debug)]
pub struct Access {
    pub claims: Claims,
    /// `None` when the client did not name its device at sign-in.
    pub device_id: Option<Uuid>,
}

/// Whom a ticket or token was issued to: the user, their live session and its device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub user_id: UserId,
    pub session_id: Uuid,
    /// `None` when the session names no device.
    pub device_id: Option<Uuid>,
}

/// Passwords, sign-in, sessions and tickets of one tenant.
#[derive(Clone, Debug)]
pub struct AuthService {
    db: Db,
    tenant: TenantId,
    keys: Keys,
    hasher: PasswordHasher,
    throttle: LoginThrottle,
    sessions: Sessions,
    tickets: Tickets,
    oidc: Option<OidcLogin>,
    password_login: bool,
}

impl AuthService {
    /// Creates the service; fails only if the password hasher cannot run.
    pub fn new(db: Db, config: AuthConfig) -> Result<Self, AuthError> {
        let keys = config.keys();
        Ok(Self {
            sessions: Sessions::new(db.clone(), keys.clone(), config.tenant, config.ttls()),
            tickets: Tickets::new(keys.clone()),
            hasher: PasswordHasher::new()?,
            throttle: LoginThrottle::new(db.clone()),
            tenant: config.tenant,
            keys,
            db,
            oidc: None,
            password_login: true,
        })
    }

    /// A service for another tenant, with that tenant's keys and no identity provider yet.
    /// It shares with this one what belongs to the process rather than to a tenant: the
    /// database, the password hashing turns, the used tickets (their ids are unique) and the
    /// notices of ended sessions (user ids are unique).
    pub fn for_tenant(&self, config: AuthConfig) -> Self {
        let keys = config.keys();
        Self {
            sessions: self
                .sessions
                .for_tenant(keys.clone(), config.tenant, config.ttls()),
            tickets: self.tickets.for_tenant(keys.clone()),
            hasher: self.hasher.clone(),
            throttle: self.throttle.clone(),
            tenant: config.tenant,
            keys,
            db: self.db.clone(),
            oidc: None,
            password_login: true,
        }
    }

    /// Enables sign-in through an identity provider. With `password_login` off, only that
    /// provider can be used to sign in.
    pub fn with_oidc(mut self, oidc: OidcService, password_login: bool) -> Self {
        self.oidc = Some(OidcLogin::new(self.db.clone(), self.tenant, oidc));
        self.password_login = password_login;
        self
    }

    /// The tenant whose users this service signs in.
    pub fn tenant(&self) -> TenantId {
        self.tenant
    }

    /// The ways users can sign in on this server.
    pub fn providers(&self) -> Providers {
        Providers {
            password: self.password_login,
            oidc: self.oidc.as_ref().map(|oidc| OidcProvider {
                name: oidc.provider.display_name().to_owned(),
            }),
        }
    }

    /// What the tenant's share links need of it: hashing their passwords, and signing the
    /// proof that one was given.
    pub fn share_passwords(&self) -> SharePasswords {
        SharePasswords {
            tenant: self.tenant,
            keys: self.keys.clone(),
            hasher: self.hasher.clone(),
            throttle: self.throttle.clone(),
        }
    }

    /// Creates a user after validating username and password.
    pub async fn create_user(&self, username: &str, password: &str) -> Result<User, AuthError> {
        if let Some(problem) = protocol::rules::username_problem(username) {
            return Err(AuthError::InvalidInput(problem));
        }
        let hashed = self.hasher.hash_new(password).await?;
        self.db
            .create_user(self.tenant, username, &hashed)
            .await
            .map_err(|err| match err {
                DbError::Conflict => AuthError::UsernameTaken,
                other => other.into(),
            })
    }

    /// Changes a user's password and revokes their sessions.
    pub async fn set_password(&self, user_id: UserId, password: &str) -> Result<(), AuthError> {
        let hashed = self.hasher.hash_new(password).await?;
        self.db.set_password_hash(user_id, &hashed).await?;
        self.sessions_ended(user_id);
        Ok(())
    }

    /// Replaces the password of a signed-in user after checking the current one, and ends
    /// their other sessions; `session_id` is the session asking, which stays signed in.
    ///
    /// Wrong current passwords are throttled like wrong sign-ins, counted for the session
    /// asking, so a stolen session cannot be used to guess the password. Sign-in is not
    /// affected: whoever holds a session must not be able to keep its owner from signing in.
    pub async fn change_password(
        &self,
        user_id: UserId,
        session_id: Uuid,
        current: &str,
        new: &str,
    ) -> Result<(), AuthError> {
        let user = self
            .db
            .find_user(user_id)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        let hash = user.password_hash.ok_or(AuthError::NoPassword)?;
        crate::password::check_new(new)?;
        // No password is that long, so it is not worth a count or the time to hash it.
        if current.len() > MAX_PASSWORD_LEN {
            return Err(AuthError::WrongPassword);
        }
        let source = format!("session:{session_id}");
        let attempt = self
            .throttle
            .admit(vec![Counter::key(user.tenant_id, user.username, source)])
            .await?;
        let checked = match self.hasher.verify(current, hash).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(AuthError::WrongPassword),
            Err(err) => Err(err),
        };
        attempt
            .settle(checked, |err| matches!(err, AuthError::WrongPassword))
            .await?;
        let hashed = self.hasher.hash(new).await?;
        self.db
            .change_password_hash(user_id, &hashed, session_id)
            .await?;
        self.sessions_ended(user_id);
        Ok(())
    }

    /// Verifies credentials and starts a session on `device`, when the client names one.
    ///
    /// Counts wrong passwords as coming from an unknown address; see
    /// [`login_from`](Self::login_from).
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        device: Option<&DeviceInfo>,
    ) -> Result<TokenPair, AuthError> {
        self.login_from(username, password, device, None).await
    }

    /// Verifies credentials sent from `address` and starts a session on `device`, when the
    /// client names one.
    ///
    /// After a few wrong passwords for a username from one address, each further attempt from
    /// there waits twice as long as the one before; the correct password starts the count
    /// again. Other addresses are not held up, so guessing at a username does not lock its
    /// owner out. Usernames that do not exist are counted the same way, so that a block does
    /// not tell which ones do.
    ///
    /// A device the user has signed in from before is counted on its own instead of with its
    /// address. Its id is known only to that install and the server, so the owner can still
    /// sign in from their phone when someone on the same network, or their own browser, has
    /// used up the address's attempts. A device id the user never signed in from earns
    /// nothing: it is counted with the address.
    ///
    /// An address is also counted over all usernames, so that trying a new name every time
    /// does not get around the limit, and nothing is hashed or stored for it while it waits.
    /// A device the user has signed in from before is not held up by this either.
    pub async fn login_from(
        &self,
        username: &str,
        password: &str,
        device: Option<&DeviceInfo>,
        address: Option<IpAddr>,
    ) -> Result<TokenPair, AuthError> {
        // No user has such a name, so it is refused before anything is stored for it.
        // Nor has anyone a password that long, and hashing it would take its time.
        if username.is_empty()
            || username.chars().count() > MAX_USERNAME_LEN
            || password.len() > MAX_PASSWORD_LEN
        {
            return Err(AuthError::InvalidCredentials);
        }
        let counters = self.login_counters(username, device, address).await?;
        let attempt = self.throttle.admit(counters).await?;
        let checked = self.check_password(username, password).await;
        let user = attempt
            .settle(checked, |err| matches!(err, AuthError::InvalidCredentials))
            .await?;
        self.sessions.start(user.id, device).await
    }

    /// What a sign-in counts against: the username, from a device the user signed in from
    /// before or else from the address; and the address as a whole unless it is such a device.
    async fn login_counters(
        &self,
        username: &str,
        device: Option<&DeviceInfo>,
        address: Option<IpAddr>,
    ) -> Result<Vec<Counter>, AuthError> {
        let known_device = match device {
            Some(device) => {
                self.db
                    .is_device_of_username(self.tenant, username, device.id)
                    .await?
            }
            None => false,
        };
        let mut counters = Vec::with_capacity(2);
        let source = match (device, address) {
            (Some(device), _) if known_device => format!("device:{}", device.id),
            (_, Some(address)) => {
                let source = attempt_source(address);
                counters.push(Counter::address(source.clone()));
                source
            }
            (_, None) => String::new(),
        };
        counters.push(Counter::key(self.tenant, username, source));
        Ok(counters)
    }

    /// The user called `username`, if `password` is theirs and they may sign in with it.
    async fn check_password(&self, username: &str, password: &str) -> Result<User, AuthError> {
        let user = self.db.find_user_by_username(self.tenant, username).await?;
        // Always verify against some hash so the timing does not reveal whether the user exists,
        // has a password, or may use password login at all.
        let hash = user
            .as_ref()
            .and_then(|user| user.password_hash.clone())
            .filter(|_| self.password_login);
        let verified = self.hasher.verify_or_pretend(password, hash).await?;
        user.filter(|_| verified)
            .ok_or(AuthError::InvalidCredentials)
    }

    /// Exchanges a refresh token for a new pair, revoking the old token.
    ///
    /// Presenting an already-used token ends the whole session.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, AuthError> {
        self.sessions.refresh(refresh_token, false).await
    }

    /// Like [`refresh`](Self::refresh), for a token kept in a browser's cookie, which all of
    /// its tabs share. Two tabs may send the same token at the same moment, so the one that
    /// comes second, within seconds, gets an access token for the session and no refresh
    /// token: the cookie already holds the one the first tab was given.
    pub async fn refresh_shared(&self, refresh_token: &str) -> Result<TokenPair, AuthError> {
        self.sessions.refresh(refresh_token, true).await
    }

    /// Starts a session labelled `label` for an extension of the app on the device of the
    /// app's own session `parent`; see [`Sessions::start_for_extension`].
    pub async fn start_device_session(
        &self,
        user_id: UserId,
        parent: Uuid,
        label: &str,
    ) -> Result<TokenPair, AuthError> {
        self.sessions
            .start_for_extension(user_id, parent, label)
            .await
    }

    /// Ends the session of `refresh_token`, and with the app's own session the sessions of
    /// extensions it started; unknown tokens are ignored.
    pub async fn logout(&self, refresh_token: &str) -> Result<(), AuthError> {
        self.sessions.logout(refresh_token).await
    }

    /// Tells open websockets that sessions of `user_id` ended, so each checks whether its own
    /// still lives. Call it after ending sessions anywhere else, such as signing out a device.
    pub fn sessions_ended(&self, user_id: UserId) {
        self.sessions.ended(user_id);
    }

    /// Notices from [`sessions_ended`](Self::sessions_ended), for every user.
    pub fn ended_sessions(&self) -> broadcast::Receiver<UserId> {
        self.sessions.ended_sessions()
    }

    /// Whether `grant`'s session still lives.
    pub async fn is_live(&self, grant: &Grant) -> Result<bool, AuthError> {
        self.sessions.is_live(grant).await
    }

    /// Validates an access token: a good signature is not enough, its session must still exist.
    pub async fn verify_access(&self, token: &str) -> Result<Access, AuthError> {
        self.sessions.verify_access(token).await
    }

    /// Lifetime of access tokens.
    pub fn access_ttl(&self) -> Duration {
        self.sessions.access_ttl()
    }

    /// Lifetime of refresh tokens: how long a session lasts without being refreshed.
    pub fn refresh_ttl(&self) -> Duration {
        self.sessions.refresh_ttl()
    }

    /// Issues a short-lived ticket limited to `request`'s scope.
    pub fn issue_ticket(
        &self,
        user_id: UserId,
        session_id: Uuid,
        request: &TicketRequest,
    ) -> Result<TicketResponse, AuthError> {
        self.tickets.issue(user_id, session_id, request)
    }

    /// Validates a ticket for `request`'s scope, once, and returns whom it was issued to.
    pub async fn verify_ticket(
        &self,
        ticket: &str,
        request: &TicketRequest,
    ) -> Result<Grant, AuthError> {
        let ticketed = self.tickets.redeem(ticket, request)?;
        let session = self
            .sessions
            .require(ticketed.session_id, ticketed.user_id)
            .await?;
        Ok(Grant {
            user_id: ticketed.user_id,
            session_id: ticketed.session_id,
            device_id: session.device_id,
        })
    }

    /// The device of `grant`'s session. A session that names none takes `device`, which is
    /// registered to the user first; one that already names a device keeps it.
    pub async fn session_device(
        &self,
        grant: &Grant,
        device: &DeviceInfo,
    ) -> Result<Uuid, AuthError> {
        self.sessions.device(grant, device).await
    }

    /// Starts a single sign-on: returns the provider URL to send the user to.
    pub async fn oidc_start(
        &self,
        redirect_uri: &str,
        code_challenge: &str,
    ) -> Result<String, AuthError> {
        self.oidc()?.start(redirect_uri, code_challenge).await
    }

    /// Finishes a single sign-on after the provider sent the user back, returning the client
    /// URL to redirect to; see [`OidcLogin::callback`].
    pub async fn oidc_callback(
        &self,
        state: Option<&str>,
        code: Option<&str>,
    ) -> Result<String, AuthError> {
        self.oidc()?.callback(&self.sessions, state, code).await
    }

    /// Exchanges the one-time code from the redirect for a session on `device`, when the
    /// client names one.
    pub async fn oidc_exchange(
        &self,
        code: &str,
        code_verifier: &str,
        device: Option<&DeviceInfo>,
    ) -> Result<TokenPair, AuthError> {
        let user_id = self.oidc()?.exchange(code, code_verifier).await?;
        self.sessions.start(user_id, device).await
    }

    fn oidc(&self) -> Result<&OidcLogin, AuthError> {
        self.oidc.as_ref().ok_or(AuthError::OidcDisabled)
    }
}

#[cfg(test)]
mod tests;
