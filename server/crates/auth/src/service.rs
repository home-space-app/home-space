use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr},
    sync::{Arc, Mutex, PoisonError},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use time::{Duration, OffsetDateTime};
use tokio::sync::broadcast;
use uuid::Uuid;

use db::{Db, DbError, LiveSession, NewDevice, OidcPending, User, UserId};
use protocol::{
    DeviceInfo, NodeId, OidcProvider, Providers, TicketRequest, TicketResponse, TokenPair,
};

use crate::{
    AuthError, Claims, OidcService,
    oidc::{Identity, is_valid_challenge, verifier_matches},
    password,
    token::{
        Keys, decode_access, decode_ticket, encode_access, encode_ticket, hash_refresh_token,
        new_refresh_token,
    },
};

const MAX_USERNAME_LEN: usize = 64;
/// Leading bits of an IPv6 address that name a network; one subscriber usually owns them all.
const IPV6_NETWORK_BITS: u32 = 64;
const MIN_PASSWORD_LEN: usize = 8;
const TICKET_TTL_SECS: i64 = 30;
const SCOPE_EVENTS: &str = "events";
const SCOPE_DOWNLOAD_PREFIX: &str = "download:";
const DUMMY_PASSWORD: &str = "dummy-password-for-timing";
const OIDC_PENDING_TTL: Duration = Duration::minutes(10);
const OIDC_CODE_TTL: Duration = Duration::seconds(60);
const OIDC_TOKEN_BYTES: usize = 32;
const USERNAME_ATTEMPTS: u32 = 20;
const FALLBACK_USERNAME: &str = "user";
const MAX_DEVICE_NAME_CHARS: usize = 100;
const MAX_PLATFORM_CHARS: usize = 32;
/// Wrong passwords a username may get before attempts for it have to wait.
const FREE_LOGIN_FAILURES: u32 = 3;
/// The first wait, which doubles with every further wrong password.
const FIRST_LOGIN_BLOCK: Duration = Duration::seconds(30);
/// The longest wait, so that someone guessing cannot lock the owner out for good.
const MAX_LOGIN_BLOCK: Duration = Duration::hours(24);
/// How long failed sign-ins of a username that is not blocked are remembered.
pub const LOGIN_ATTEMPTS_MEMORY: Duration = Duration::days(1);
/// Room for session-end notices a slow websocket has not read yet; one that falls further
/// behind re-checks its session anyway.
const ENDED_CAPACITY: usize = 64;

/// Token settings.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub jwt_secret: Vec<u8>,
    /// Earlier secrets that still verify tokens, for rotating `jwt_secret`.
    pub previous_jwt_secrets: Vec<Vec<u8>>,
    pub access_ttl: Duration,
    pub refresh_ttl: Duration,
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

/// Login, refresh, logout and user management.
#[derive(Clone, Debug)]
pub struct AuthService {
    db: Db,
    config: AuthConfig,
    keys: Keys,
    dummy_hash: String,
    oidc: Option<OidcService>,
    password_login: bool,
    /// Users some of whose sessions just ended, so open websockets re-check theirs.
    ended: broadcast::Sender<UserId>,
    /// Tickets already used, by id, with the time they expire and can be forgotten. Kept in
    /// memory: after a restart a used ticket would be accepted again, for the rest of its
    /// [`TICKET_TTL_SECS`] at most.
    used_tickets: Arc<Mutex<HashMap<Uuid, i64>>>,
}

impl AuthService {
    /// Creates the service; fails only if the password hasher cannot run.
    pub fn new(db: Db, config: AuthConfig) -> Result<Self, AuthError> {
        let dummy_hash = password::hash(DUMMY_PASSWORD)?;
        let keys = Keys::new(
            config.jwt_secret.clone(),
            config.previous_jwt_secrets.clone(),
        );
        Ok(Self {
            db,
            config,
            keys,
            dummy_hash,
            oidc: None,
            password_login: true,
            ended: broadcast::channel(ENDED_CAPACITY).0,
            used_tickets: Arc::default(),
        })
    }

    /// Enables sign-in through an identity provider. With `password_login` off, only that
    /// provider can be used to sign in.
    pub fn with_oidc(mut self, oidc: OidcService, password_login: bool) -> Self {
        self.oidc = Some(oidc);
        self.password_login = password_login;
        self
    }

    /// The ways users can sign in on this server.
    pub fn providers(&self) -> Providers {
        Providers {
            password: self.password_login,
            oidc: self.oidc.as_ref().map(|oidc| OidcProvider {
                name: oidc.display_name().to_owned(),
            }),
        }
    }

    /// Creates a user after validating username and password.
    pub async fn create_user(&self, username: &str, password: &str) -> Result<User, AuthError> {
        if username.is_empty() || username.len() > MAX_USERNAME_LEN {
            Err(AuthError::InvalidInput("username must be 1-64 characters"))
        } else if password.len() < MIN_PASSWORD_LEN {
            Err(AuthError::InvalidInput(
                "password must be at least 8 characters",
            ))
        } else {
            let hashed = self.hash_blocking(password).await?;
            self.db
                .create_user(username, &hashed)
                .await
                .map_err(|err| match err {
                    DbError::Conflict => AuthError::UsernameTaken,
                    other => other.into(),
                })
        }
    }

    /// Changes a user's password and revokes their sessions.
    pub async fn set_password(&self, user_id: UserId, password: &str) -> Result<(), AuthError> {
        if password.len() < MIN_PASSWORD_LEN {
            Err(AuthError::InvalidInput(
                "password must be at least 8 characters",
            ))
        } else {
            let hashed = self.hash_blocking(password).await?;
            self.db.set_password_hash(user_id, &hashed).await?;
            self.sessions_ended(user_id);
            Ok(())
        }
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
        if new.len() < MIN_PASSWORD_LEN {
            return Err(AuthError::InvalidInput(
                "password must be at least 8 characters",
            ));
        }
        let source = format!("session:{session_id}");
        if let Err(until) = self
            .db
            .begin_login_attempt(&user.username, &source, login_block)
            .await?
        {
            let wait = until - OffsetDateTime::now_utc();
            return Err(AuthError::TooManyAttempts {
                retry_after: wait.whole_seconds().max(0).unsigned_abs() + 1,
            });
        }
        let candidate = current.to_owned();
        let verified =
            tokio::task::spawn_blocking(move || password::verify(&candidate, &hash)).await?;
        if !verified {
            return Err(AuthError::WrongPassword);
        }
        self.db
            .clear_login_attempts(&user.username, &source)
            .await?;
        let hashed = self.hash_blocking(new).await?;
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
    /// After [`FREE_LOGIN_FAILURES`] wrong passwords for a username from one address, each
    /// further attempt from there waits twice as long as the one before, starting at
    /// [`FIRST_LOGIN_BLOCK`]; the correct password starts the count again. Other addresses are
    /// not held up, so guessing at a username does not lock its owner out. Usernames that do
    /// not exist are counted the same way, so that a block does not tell which ones do.
    ///
    /// A device the user has signed in from before is counted on its own instead of with its
    /// address. Its id is known only to that install and the server, so the owner can still
    /// sign in from their phone when someone on the same network, or their own browser, has
    /// used up the address's attempts. A device id the user never signed in from earns
    /// nothing: it is counted with the address.
    pub async fn login_from(
        &self,
        username: &str,
        password: &str,
        device: Option<&DeviceInfo>,
        address: Option<IpAddr>,
    ) -> Result<TokenPair, AuthError> {
        // No user has such a name, so it is refused before anything is stored for it.
        if username.chars().count() > MAX_USERNAME_LEN {
            return Err(AuthError::InvalidCredentials);
        }
        let known_device = match device {
            Some(device) => self.db.is_device_of_username(username, device.id).await?,
            None => false,
        };
        let source = match device {
            Some(device) if known_device => format!("device:{}", device.id),
            _ => address.map(attempt_source).unwrap_or_default(),
        };
        if let Err(until) = self
            .db
            .begin_login_attempt(username, &source, login_block)
            .await?
        {
            let wait = until - OffsetDateTime::now_utc();
            return Err(AuthError::TooManyAttempts {
                retry_after: wait.whole_seconds().max(0).unsigned_abs() + 1,
            });
        }
        let user = self.db.find_user_by_username(username).await?;
        // Always verify against some hash so the timing does not reveal whether the user exists,
        // has a password, or may use password login at all.
        let hash = user
            .as_ref()
            .and_then(|user| user.password_hash.clone())
            .filter(|_| self.password_login);
        let allowed = hash.is_some();
        let hash = hash.unwrap_or_else(|| self.dummy_hash.clone());
        let candidate = password.to_owned();
        let verified =
            tokio::task::spawn_blocking(move || password::verify(&candidate, &hash)).await?;
        match user {
            Some(user) if verified && allowed => {
                self.db.clear_login_attempts(username, &source).await?;
                self.start_session(user.id, device).await
            }
            _ => Err(AuthError::InvalidCredentials),
        }
    }

    /// Exchanges a refresh token for a new pair, revoking the old token.
    ///
    /// Presenting an already-used token ends the whole session.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, AuthError> {
        let stored = self
            .db
            .find_refresh_token(&hash_refresh_token(refresh_token))
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        if stored.revoked_at.is_some() {
            self.end_session(stored.user_id, stored.family_id).await?;
            Err(AuthError::InvalidCredentials)
        } else if stored.expires_at <= OffsetDateTime::now_utc() {
            Err(AuthError::InvalidCredentials)
        } else if self.db.revoke_refresh_token(stored.id).await? {
            self.continue_session(stored.user_id, stored.family_id)
                .await
        } else {
            self.end_session(stored.user_id, stored.family_id).await?;
            Err(AuthError::InvalidCredentials)
        }
    }

    /// Ends the session of `refresh_token`; unknown tokens are ignored.
    pub async fn logout(&self, refresh_token: &str) -> Result<(), AuthError> {
        if let Some(stored) = self
            .db
            .find_refresh_token(&hash_refresh_token(refresh_token))
            .await?
        {
            self.end_session(stored.user_id, stored.family_id).await?;
        }
        Ok(())
    }

    async fn end_session(&self, user_id: UserId, session_id: Uuid) -> Result<(), AuthError> {
        self.db.end_session(session_id).await?;
        self.sessions_ended(user_id);
        Ok(())
    }

    /// Tells open websockets that sessions of `user_id` ended, so each checks whether its own
    /// still lives. Call it after ending sessions anywhere else, such as signing out a device.
    pub fn sessions_ended(&self, user_id: UserId) {
        // An error only means no websocket is open right now.
        let _ = self.ended.send(user_id);
    }

    /// Notices from [`sessions_ended`](Self::sessions_ended), for every user.
    pub fn ended_sessions(&self) -> broadcast::Receiver<UserId> {
        self.ended.subscribe()
    }

    /// Whether `grant`'s session still lives.
    pub async fn is_live(&self, grant: &Grant) -> Result<bool, AuthError> {
        Ok(self
            .db
            .live_session(grant.session_id, grant.user_id)
            .await?
            .is_some())
    }

    /// Validates an access token: a good signature is not enough, its session must still exist.
    pub async fn verify_access(&self, token: &str) -> Result<Access, AuthError> {
        let claims = decode_access(&self.keys, token)?;
        let session = self.require_session(claims.sid, claims.sub).await?;
        Ok(Access {
            claims,
            device_id: session.device_id,
        })
    }

    /// Lifetime of access tokens.
    pub fn access_ttl(&self) -> Duration {
        self.config.access_ttl
    }

    /// Lifetime of refresh tokens: how long a session lasts without being refreshed.
    pub fn refresh_ttl(&self) -> Duration {
        self.config.refresh_ttl
    }

    /// Issues a short-lived ticket limited to `request`'s scope.
    pub fn issue_ticket(
        &self,
        user_id: UserId,
        session_id: Uuid,
        request: &TicketRequest,
    ) -> Result<TicketResponse, AuthError> {
        let ttl = Duration::seconds(TICKET_TTL_SECS);
        Ok(TicketResponse {
            ticket: encode_ticket(&self.keys, user_id, session_id, &scope_of(request), ttl)?,
            expires_in: TICKET_TTL_SECS,
        })
    }

    /// Validates a ticket for `request`'s scope and returns whom it was issued to.
    ///
    /// A ticket works once. It reaches the server in a URL, so a copy of it, from a proxy's
    /// log or the browser's history, must be worth nothing after the request it was made for.
    pub async fn verify_ticket(
        &self,
        ticket: &str,
        request: &TicketRequest,
    ) -> Result<Grant, AuthError> {
        let grant = decode_ticket(&self.keys, ticket, &scope_of(request))?;
        if !self.use_ticket(grant.id, grant.expires_at) {
            return Err(AuthError::InvalidCredentials);
        }
        let session = self
            .require_session(grant.session_id, grant.user_id)
            .await?;
        Ok(Grant {
            user_id: grant.user_id,
            session_id: grant.session_id,
            device_id: session.device_id,
        })
    }

    /// Marks a ticket used; `false` when it already was. Only tickets that passed validation
    /// get here, and each is forgotten once it has expired, so the list stays as short as the
    /// tickets used within one lifetime.
    fn use_ticket(&self, id: Uuid, expires_at: i64) -> bool {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let mut used = self
            .used_tickets
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        used.retain(|_, expires_at| *expires_at > now);
        used.insert(id, expires_at).is_none()
    }

    /// The device of `grant`'s session. A session that names none takes `device`, which is
    /// registered to the user first; one that already names a device keeps it.
    pub async fn session_device(
        &self,
        grant: &Grant,
        device: &DeviceInfo,
    ) -> Result<Uuid, AuthError> {
        if let Some(current) = grant.device_id {
            Ok(current)
        } else {
            let device = new_device(device)?;
            self.db.register_device(grant.user_id, device).await?;
            self.db
                .attach_device(grant.session_id, grant.user_id, device.id)
                .await?;
            Ok(device.id)
        }
    }

    async fn require_session(
        &self,
        session_id: Uuid,
        user_id: UserId,
    ) -> Result<LiveSession, AuthError> {
        self.db
            .live_session(session_id, user_id)
            .await?
            .ok_or(AuthError::InvalidCredentials)
    }

    /// Starts a single sign-on: returns the provider URL to send the user to.
    ///
    /// `redirect_uri` is where the client wants to land afterwards and `code_challenge` is the
    /// S256 PKCE challenge its later [`oidc_exchange`](Self::oidc_exchange) must answer.
    pub async fn oidc_start(
        &self,
        redirect_uri: &str,
        code_challenge: &str,
    ) -> Result<String, AuthError> {
        let oidc = self.oidc()?;
        if !oidc.allows_redirect(redirect_uri) {
            return Err(AuthError::InvalidInput("redirect_uri is not allowed"));
        }
        if !is_valid_challenge(code_challenge) {
            return Err(AuthError::InvalidInput(
                "code_challenge must be an S256 challenge",
            ));
        }
        let state = random_token();
        let authorization = oidc.authorize(&state);
        let pending = OidcPending {
            nonce: authorization.nonce,
            idp_pkce_verifier: authorization.pkce_verifier,
            client_redirect: redirect_uri.to_owned(),
            client_challenge: code_challenge.to_owned(),
        };
        self.db
            .insert_oidc_pending(
                &hash_refresh_token(&state),
                &pending,
                OffsetDateTime::now_utc() + OIDC_PENDING_TTL,
            )
            .await?;
        Ok(authorization.url)
    }

    /// Finishes a single sign-on after the provider sent the user back, returning the client URL
    /// to redirect to: it carries a one-time `code` or, if the sign-in failed, an `error`.
    ///
    /// When `state` is missing, unknown, expired or already used there is no client to send the
    /// user to, so the browser goes to the web client with `error=login_expired`. This happens
    /// when the provider resumes a sign-in that already came back once, for example after a
    /// wrong password and the browser's back button, and the user should get a page that lets
    /// them start again rather than a bare API error.
    pub async fn oidc_callback(
        &self,
        state: Option<&str>,
        code: Option<&str>,
    ) -> Result<String, AuthError> {
        let oidc = self.oidc()?;
        let pending = match state {
            Some(state) => {
                self.db
                    .take_oidc_pending(&hash_refresh_token(state))
                    .await?
            }
            None => None,
        };
        let Some(pending) = pending else {
            tracing::info!("single sign-on came back with an unknown or used state");
            return Ok(append_query(
                &oidc.web_redirect(),
                ("error", "login_expired".to_owned()),
            ));
        };
        let outcome = match code {
            Some(code) => self.finish_oidc_login(oidc, code, &pending).await,
            None => Err(AuthError::OidcDenied),
        };
        let params = match outcome {
            Ok(code) => ("code", code),
            Err(AuthError::OidcDenied) => ("error", "access_denied".to_owned()),
            Err(err) => {
                tracing::warn!(error = %err, "single sign-on failed");
                ("error", "login_failed".to_owned())
            }
        };
        Ok(append_query(&pending.client_redirect, params))
    }

    /// Exchanges the one-time code from the redirect for a session on `device`, when the
    /// client names one.
    pub async fn oidc_exchange(
        &self,
        code: &str,
        code_verifier: &str,
        device: Option<&DeviceInfo>,
    ) -> Result<TokenPair, AuthError> {
        self.oidc()?;
        let stored = self
            .db
            .take_oidc_code(&hash_refresh_token(code))
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        if verifier_matches(code_verifier, &stored.client_challenge) {
            self.start_session(stored.user_id, device).await
        } else {
            Err(AuthError::InvalidCredentials)
        }
    }

    fn oidc(&self) -> Result<&OidcService, AuthError> {
        self.oidc.as_ref().ok_or(AuthError::OidcDisabled)
    }

    /// Verifies the provider's answer, provisions the user and stores a login code for them.
    async fn finish_oidc_login(
        &self,
        oidc: &OidcService,
        code: &str,
        pending: &OidcPending,
    ) -> Result<String, AuthError> {
        let identity = oidc
            .complete(code, &pending.nonce, &pending.idp_pkce_verifier)
            .await?;
        let user_id = self.user_for_identity(&identity).await?;
        let login_code = random_token();
        self.db
            .insert_oidc_code(
                &hash_refresh_token(&login_code),
                user_id,
                &pending.client_challenge,
                OffsetDateTime::now_utc() + OIDC_CODE_TTL,
            )
            .await?;
        Ok(login_code)
    }

    /// The local user for a provider account, created on first sign-in.
    async fn user_for_identity(&self, identity: &Identity) -> Result<UserId, AuthError> {
        let find = || {
            self.db
                .find_user_by_identity(&identity.issuer, &identity.subject)
        };
        if let Some(user) = find().await? {
            return Ok(user.id);
        }
        let base = username_base(identity);
        for attempt in 0..USERNAME_ATTEMPTS {
            let username = username_candidate(&base, attempt);
            match self
                .db
                .create_user_with_identity(
                    &username,
                    &identity.issuer,
                    &identity.subject,
                    identity.email.as_deref(),
                )
                .await
            {
                Ok(user) => return Ok(user.id),
                // Either the name is taken or a concurrent sign-in created this very identity.
                Err(DbError::Conflict) => {
                    if let Some(user) = find().await? {
                        return Ok(user.id);
                    }
                }
                Err(err) => return Err(err.into()),
            }
        }
        Err(AuthError::InvalidInput("no free username for this account"))
    }

    /// Starts a session, which exists only from here: after a verified password or provider
    /// login. Tokens cannot create one. A named device is registered to the user first.
    async fn start_session(
        &self,
        user_id: UserId,
        device: Option<&DeviceInfo>,
    ) -> Result<TokenPair, AuthError> {
        let device = device.map(new_device).transpose()?;
        if let Some(device) = device {
            self.db.register_device(user_id, device).await?;
        }
        let session_id = Uuid::now_v7();
        let refresh_token = new_refresh_token();
        self.db
            .start_session(
                session_id,
                user_id,
                device.map(|device| device.id),
                &hash_refresh_token(&refresh_token),
                self.refresh_expiry(),
            )
            .await?;
        self.pair(user_id, session_id, refresh_token)
    }

    /// Hands the next refresh token to a session; fails if the session has ended.
    async fn continue_session(
        &self,
        user_id: UserId,
        session_id: Uuid,
    ) -> Result<TokenPair, AuthError> {
        let refresh_token = new_refresh_token();
        let continued = self
            .db
            .continue_session(
                session_id,
                user_id,
                &hash_refresh_token(&refresh_token),
                self.refresh_expiry(),
            )
            .await?;
        if continued {
            self.pair(user_id, session_id, refresh_token)
        } else {
            Err(AuthError::InvalidCredentials)
        }
    }

    fn refresh_expiry(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc() + self.config.refresh_ttl
    }

    fn pair(
        &self,
        user_id: UserId,
        session_id: Uuid,
        refresh_token: String,
    ) -> Result<TokenPair, AuthError> {
        Ok(TokenPair {
            access_token: encode_access(&self.keys, user_id, session_id, self.config.access_ttl)?,
            refresh_token,
            expires_in: self.config.access_ttl.whole_seconds(),
        })
    }

    async fn hash_blocking(&self, password: &str) -> Result<String, AuthError> {
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || password::hash(&password)).await?
    }
}

/// How long the next sign-in attempt must wait after `failures` wrong passwords in a row:
/// not at all for the first [`FREE_LOGIN_FAILURES`], then 30 seconds, 1 minute, 2 minutes and
/// so on, up to [`MAX_LOGIN_BLOCK`].
fn login_block(failures: u32) -> Option<Duration> {
    let doublings = failures.checked_sub(FREE_LOGIN_FAILURES)?;
    let factor = 1i32.checked_shl(doublings).filter(|factor| *factor > 0);
    let wait = factor
        .and_then(|factor| FIRST_LOGIN_BLOCK.checked_mul(factor))
        .unwrap_or(MAX_LOGIN_BLOCK);
    Some(wait.min(MAX_LOGIN_BLOCK))
}

/// What failed sign-ins from `address` are counted under: the address itself for IPv4 and
/// its network for IPv6, where a single client can pick among more addresses than could
/// ever be blocked one by one.
fn attempt_source(address: IpAddr) -> String {
    match address.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let network = u128::from(v6) & (u128::MAX << (u128::BITS - IPV6_NETWORK_BITS));
            format!("{}/{IPV6_NETWORK_BITS}", Ipv6Addr::from(network))
        }
    }
}

/// `device` checked against what the server stores: a name of 1-100 characters, kept without
/// surrounding blanks, and a platform of 1-32.
fn new_device(device: &DeviceInfo) -> Result<NewDevice<'_>, AuthError> {
    let name = device_name(&device.name).ok_or(AuthError::InvalidInput(
        "device name must be 1-100 characters",
    ))?;
    let platform = device.platform.trim();
    if !(1..=MAX_PLATFORM_CHARS).contains(&platform.chars().count()) {
        Err(AuthError::InvalidInput(
            "device platform must be 1-32 characters",
        ))
    } else {
        Ok(NewDevice {
            id: device.id,
            name,
            platform,
        })
    }
}

/// `name` as a device name, or `None` when it is blank or too long.
pub fn device_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (1..=MAX_DEVICE_NAME_CHARS)
        .contains(&name.chars().count())
        .then_some(name)
}

fn random_token() -> String {
    let mut bytes = [0u8; OIDC_TOKEN_BYTES];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn append_query(url: &str, (key, value): (&str, impl AsRef<str>)) -> String {
    // Redirects are validated to carry no query or fragment, so a plain `?` is enough.
    let value: String =
        openidconnect::url::form_urlencoded::byte_serialize(value.as_ref().as_bytes()).collect();
    format!("{url}?{key}={value}")
}

/// The preferred name for a new user: the provider's username, else the e-mail's local part,
/// else the subject.
fn username_base(identity: &Identity) -> String {
    let from_email = identity
        .email
        .as_deref()
        .and_then(|email| email.split('@').next());
    let name = identity
        .username_hint
        .as_deref()
        .or(from_email)
        .unwrap_or(&identity.subject);
    let cleaned: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    if cleaned.is_empty() {
        FALLBACK_USERNAME.to_owned()
    } else {
        cleaned
    }
}

/// `base`, or `base-2`, `base-3`, ... for later attempts, cut to fit [`MAX_USERNAME_LEN`] bytes.
fn username_candidate(base: &str, attempt: u32) -> String {
    let suffix = if attempt == 0 {
        String::new()
    } else {
        format!("-{}", attempt + 1)
    };
    let mut name = String::new();
    for c in base.chars() {
        if name.len() + c.len_utf8() + suffix.len() > MAX_USERNAME_LEN {
            break;
        }
        name.push(c);
    }
    name + &suffix
}

fn scope_of(request: &TicketRequest) -> String {
    match request {
        TicketRequest::Events => SCOPE_EVENTS.to_owned(),
        TicketRequest::Download { node_id } => download_scope(*node_id),
        TicketRequest::DownloadVersion { node_id, version } => {
            format!("{}:v{version}", download_scope(*node_id))
        }
        TicketRequest::TransferItem {
            transfer_id,
            item_id,
        } => format!("transfer:{transfer_id}:{item_id}"),
        TicketRequest::TransferArchive { transfer_id } => format!("transfer:{transfer_id}"),
    }
}

fn download_scope(node_id: NodeId) -> String {
    format!("{SCOPE_DOWNLOAD_PREFIX}{node_id}")
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    fn service(pool: PgPool) -> AuthService {
        let config = AuthConfig {
            jwt_secret: b"test-secret".to_vec(),
            previous_jwt_secrets: Vec::new(),
            access_ttl: Duration::minutes(15),
            refresh_ttl: Duration::days(30),
        };
        AuthService::new(Db::from_pool(pool), config).expect("service")
    }

    #[test]
    fn username_candidates_get_a_suffix_and_fit_the_limit() {
        assert_eq!(username_candidate("alice", 0), "alice");
        assert_eq!(username_candidate("alice", 1), "alice-2");
        let long = "x".repeat(100);
        assert_eq!(username_candidate(&long, 0).len(), MAX_USERNAME_LEN);
        assert_eq!(username_candidate(&long, 9).len(), MAX_USERNAME_LEN);
        assert!(username_candidate(&long, 9).ends_with("-10"));
        let wide = "é".repeat(100);
        assert!(username_candidate(&wide, 1).len() <= MAX_USERNAME_LEN);
    }

    #[test]
    fn username_base_prefers_the_username_then_the_email() {
        let mut identity = Identity {
            issuer: "https://idp".to_owned(),
            subject: "sub-1".to_owned(),
            username_hint: Some("alice".to_owned()),
            email: Some("a.b@example.com".to_owned()),
        };
        assert_eq!(username_base(&identity), "alice");
        identity.username_hint = None;
        assert_eq!(username_base(&identity), "a.b");
        identity.email = None;
        assert_eq!(username_base(&identity), "sub-1");
        identity.subject = "  ".to_owned();
        assert_eq!(username_base(&identity), FALLBACK_USERNAME);
    }

    #[test]
    fn append_query_escapes_the_value() {
        assert_eq!(
            append_query("http://127.0.0.1:1/callback", ("code", "a b&c")),
            "http://127.0.0.1:1/callback?code=a+b%26c"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_succeeds_with_correct_password(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let pair = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        assert!(auth.verify_access(&pair.access_token).await.is_ok());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_from_a_device_registers_it_and_tags_the_session(pool: PgPool) {
        let db = Db::from_pool(pool.clone());
        let auth = service(pool);
        let user = auth.create_user("alice", "password-1").await.expect("user");
        let device = DeviceInfo {
            id: Uuid::now_v7(),
            name: "  Laptop ".to_owned(),
            platform: protocol::PLATFORM_LINUX.to_owned(),
        };

        let pair = auth
            .login("alice", "password-1", Some(&device))
            .await
            .expect("login");

        let access = auth
            .verify_access(&pair.access_token)
            .await
            .expect("access");
        assert_eq!(access.device_id, Some(device.id));
        let devices = db.devices(user.id).await.expect("devices");
        assert_eq!(
            devices
                .iter()
                .map(|d| (d.id, d.name.as_str(), d.signed_in))
                .collect::<Vec<_>>(),
            [(device.id, "Laptop", true)]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_blank_device_name_is_refused(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let device = DeviceInfo {
            id: Uuid::now_v7(),
            name: " ".to_owned(),
            platform: protocol::PLATFORM_WEB.to_owned(),
        };
        let result = auth.login("alice", "password-1", Some(&device)).await;
        assert!(matches!(result, Err(AuthError::InvalidInput(_))));
    }

    fn laptop() -> DeviceInfo {
        DeviceInfo {
            id: Uuid::now_v7(),
            name: "Laptop".to_owned(),
            platform: protocol::PLATFORM_LINUX.to_owned(),
        }
    }

    async fn grant_of(auth: &AuthService, pair: &TokenPair) -> Grant {
        let access = auth
            .verify_access(&pair.access_token)
            .await
            .expect("access");
        Grant {
            user_id: access.claims.sub,
            session_id: access.claims.sid,
            device_id: access.device_id,
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_session_without_a_device_registers_the_one_it_names(pool: PgPool) {
        let db = Db::from_pool(pool.clone());
        let auth = service(pool);
        let user = auth.create_user("alice", "password-1").await.expect("user");
        let pair = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        let device = laptop();

        let id = auth
            .session_device(&grant_of(&auth, &pair).await, &device)
            .await
            .expect("device");

        assert_eq!(id, device.id);
        assert_eq!(grant_of(&auth, &pair).await.device_id, Some(device.id));
        assert_eq!(db.devices(user.id).await.expect("devices").len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_session_with_a_device_keeps_it(pool: PgPool) {
        let db = Db::from_pool(pool.clone());
        let auth = service(pool);
        let user = auth.create_user("alice", "password-1").await.expect("user");
        let device = laptop();
        let pair = auth
            .login("alice", "password-1", Some(&device))
            .await
            .expect("login");

        let id = auth
            .session_device(&grant_of(&auth, &pair).await, &laptop())
            .await
            .expect("device");

        assert_eq!(id, device.id);
        assert_eq!(db.devices(user.id).await.expect("devices").len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_blank_device_is_not_registered_for_a_session(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let pair = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        let blank = DeviceInfo {
            name: " ".to_owned(),
            ..laptop()
        };

        let result = auth
            .session_device(&grant_of(&auth, &pair).await, &blank)
            .await;

        assert!(matches!(result, Err(AuthError::InvalidInput(_))));
        assert_eq!(grant_of(&auth, &pair).await.device_id, None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_fails_with_wrong_password(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let result = auth.login("alice", "password-2", None).await;
        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_fails_for_unknown_user(pool: PgPool) {
        let result = service(pool).login("nobody", "password-1", None).await;
        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[test]
    fn login_waits_double_after_three_failures_up_to_a_day() {
        assert_eq!(login_block(1), None);
        assert_eq!(login_block(2), None);
        assert_eq!(login_block(3), Some(Duration::seconds(30)));
        assert_eq!(login_block(4), Some(Duration::minutes(1)));
        assert_eq!(login_block(5), Some(Duration::minutes(2)));
        assert_eq!(login_block(6), Some(Duration::minutes(4)));
        assert_eq!(login_block(20), Some(MAX_LOGIN_BLOCK));
        assert_eq!(login_block(u32::MAX), Some(MAX_LOGIN_BLOCK));
    }

    async fn wrong(auth: &AuthService, username: &str) -> Result<TokenPair, AuthError> {
        auth.login(username, "wrong-password", None).await
    }

    /// Ends the current wait, as if it had passed.
    async fn wait_out(pool: &PgPool) {
        sqlx::query("UPDATE login_attempts SET blocked_until = now() - interval '1 second'")
            .execute(pool)
            .await
            .expect("wait out");
    }

    fn retry_after(result: Result<TokenPair, AuthError>) -> u64 {
        match result {
            Err(AuthError::TooManyAttempts { retry_after }) => retry_after,
            other => panic!("expected a block, got {other:?}"),
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn three_wrong_passwords_block_sign_in_even_with_the_right_one(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        for _ in 0..3 {
            assert!(matches!(
                wrong(&auth, "alice").await,
                Err(AuthError::InvalidCredentials)
            ));
        }
        let wait = retry_after(auth.login("alice", "password-1", None).await);
        assert!((29..=31).contains(&wait), "{wait}");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn each_further_wrong_password_doubles_the_wait(pool: PgPool) {
        let auth = service(pool.clone());
        auth.create_user("alice", "password-1").await.expect("user");
        for _ in 0..3 {
            wrong(&auth, "alice").await.expect_err("wrong");
        }
        for expected in [60, 120, 240] {
            wait_out(&pool).await;
            assert!(matches!(
                wrong(&auth, "alice").await,
                Err(AuthError::InvalidCredentials)
            ));
            let wait = retry_after(wrong(&auth, "alice").await);
            assert!((expected - 1..=expected + 1).contains(&wait), "{wait}");
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_right_password_resets_the_count(pool: PgPool) {
        let auth = service(pool.clone());
        auth.create_user("alice", "password-1").await.expect("user");
        for _ in 0..3 {
            wrong(&auth, "alice").await.expect_err("wrong");
        }
        wait_out(&pool).await;
        auth.login("alice", "password-1", None)
            .await
            .expect("login");
        for _ in 0..3 {
            assert!(matches!(
                wrong(&auth, "alice").await,
                Err(AuthError::InvalidCredentials)
            ));
        }
        retry_after(wrong(&auth, "alice").await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn unknown_users_are_blocked_like_real_ones_and_others_are_not(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        for _ in 0..3 {
            wrong(&auth, "nobody").await.expect_err("wrong");
        }
        retry_after(wrong(&auth, "nobody").await);
        auth.login("alice", "password-1", None)
            .await
            .expect("alice");
    }

    #[test]
    fn attempts_are_counted_per_ipv4_address_and_ipv6_network() {
        let source = |address: &str| attempt_source(address.parse().expect("address"));
        assert_eq!(source("203.0.113.9"), "203.0.113.9");
        assert_eq!(source("::ffff:203.0.113.9"), "203.0.113.9");
        assert_eq!(source("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::/64");
        assert_eq!(source("2001:db8:1:2::1"), source("2001:db8:1:2:ffff::9"));
        assert_ne!(source("2001:db8:1:2::1"), source("2001:db8:1:3::1"));
    }

    fn address(text: &str) -> Option<IpAddr> {
        Some(text.parse().expect("address"))
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn wrong_passwords_from_elsewhere_do_not_lock_the_owner_out(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let attacker = address("198.51.100.7");
        for _ in 0..3 {
            auth.login_from("alice", "wrong-password", None, attacker)
                .await
                .expect_err("wrong");
        }
        // The guesser is held up, even with the right password...
        retry_after(auth.login_from("alice", "password-1", None, attacker).await);
        // ...and the owner, somewhere else, is not.
        auth.login_from("alice", "password-1", None, address("192.0.2.1"))
            .await
            .expect("owner");
        // The owner signing in does not give the guesser a fresh start.
        retry_after(auth.login_from("alice", "password-1", None, attacker).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_known_device_can_sign_in_while_its_address_is_blocked(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let shared = address("198.51.100.7");
        let phone = laptop();
        auth.login_from("alice", "password-1", Some(&phone), shared)
            .await
            .expect("first sign-in registers the phone");
        for _ in 0..3 {
            auth.login_from("alice", "wrong-password", None, shared)
                .await
                .expect_err("wrong");
        }
        retry_after(auth.login_from("alice", "password-1", None, shared).await);

        // A device id alice never signed in from does not get around the block...
        retry_after(
            auth.login_from("alice", "password-1", Some(&laptop()), shared)
                .await,
        );
        // ...and her phone does.
        auth.login_from("alice", "password-1", Some(&phone), shared)
            .await
            .expect("known device");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_known_device_is_throttled_on_its_own(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let home = address("192.0.2.1");
        let phone = laptop();
        auth.login_from("alice", "password-1", Some(&phone), home)
            .await
            .expect("first sign-in registers the phone");
        for _ in 0..3 {
            auth.login_from("alice", "wrong-password", Some(&phone), home)
                .await
                .expect_err("wrong");
        }
        retry_after(
            auth.login_from("alice", "password-1", Some(&phone), home)
                .await,
        );
        auth.login_from("alice", "password-1", None, home)
            .await
            .expect("the address itself is not blocked");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_overlong_username_is_refused_without_being_remembered(pool: PgPool) {
        let auth = service(pool.clone());
        let name = "x".repeat(MAX_USERNAME_LEN + 1);
        let result = auth.login(&name, "password-1", None).await;
        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        let remembered: i64 = sqlx::query_scalar("SELECT count(*) FROM login_attempts")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(remembered, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn guessing_the_current_password_does_not_block_sign_in(pool: PgPool) {
        let auth = service(pool);
        let user = auth.create_user("alice", "password-1").await.expect("user");
        let pair = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        let session = grant_of(&auth, &pair).await.session_id;
        for _ in 0..3 {
            let result = auth
                .change_password(user.id, session, "wrong-password", "password-2")
                .await;
            assert!(matches!(result, Err(AuthError::WrongPassword)));
        }
        let blocked = auth
            .change_password(user.id, session, "password-1", "password-2")
            .await;
        assert!(matches!(blocked, Err(AuthError::TooManyAttempts { .. })));
        auth.login("alice", "password-1", None)
            .await
            .expect("sign-in is not blocked");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_ticket_works_once(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let pair = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        let grant = grant_of(&auth, &pair).await;
        let issue = || {
            auth.issue_ticket(grant.user_id, grant.session_id, &TicketRequest::Events)
                .expect("ticket")
                .ticket
        };
        let (first, second) = (issue(), issue());

        assert!(
            auth.verify_ticket(&first, &TicketRequest::Events)
                .await
                .is_ok()
        );
        assert!(matches!(
            auth.verify_ticket(&first, &TicketRequest::Events).await,
            Err(AuthError::InvalidCredentials)
        ));
        // Another ticket of the same session and scope is its own ticket.
        assert!(
            auth.verify_ticket(&second, &TicketRequest::Events)
                .await
                .is_ok()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_ticket_refused_for_its_scope_is_not_used_up(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let pair = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        let grant = grant_of(&auth, &pair).await;
        let ticket = auth
            .issue_ticket(grant.user_id, grant.session_id, &TicketRequest::Events)
            .expect("ticket")
            .ticket;
        let download = TicketRequest::Download {
            node_id: NodeId::new(),
        };

        assert!(auth.verify_ticket(&ticket, &download).await.is_err());
        assert!(
            auth.verify_ticket(&ticket, &TicketRequest::Events)
                .await
                .is_ok()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn used_tickets_are_forgotten_once_they_expire(pool: PgPool) {
        let auth = service(pool);
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let (expired, live) = (Uuid::now_v7(), Uuid::now_v7());
        assert!(auth.use_ticket(expired, now - 1));
        assert!(auth.use_ticket(live, now + 30));
        assert!(!auth.use_ticket(live, now + 30));
        let used = auth.used_tickets.lock().expect("lock");
        assert_eq!(used.keys().collect::<Vec<_>>(), [&live]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn duplicate_username_is_rejected(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let result = auth.create_user("alice", "password-2").await;
        assert!(matches!(result, Err(AuthError::UsernameTaken)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn refresh_rotates_the_token(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let first = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        let second = auth.refresh(&first.refresh_token).await.expect("refresh");
        assert_ne!(first.refresh_token, second.refresh_token);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reusing_a_refresh_token_revokes_the_family(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let first = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        let second = auth.refresh(&first.refresh_token).await.expect("refresh");
        assert!(auth.refresh(&first.refresh_token).await.is_err());
        assert!(auth.refresh(&second.refresh_token).await.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn logout_revokes_the_session(pool: PgPool) {
        let auth = service(pool);
        auth.create_user("alice", "password-1").await.expect("user");
        let pair = auth
            .login("alice", "password-1", None)
            .await
            .expect("login");
        auth.logout(&pair.refresh_token).await.expect("logout");
        assert!(auth.refresh(&pair.refresh_token).await.is_err());
    }
}
