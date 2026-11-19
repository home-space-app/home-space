//! Sessions: starting them, handing out the next refresh token, and ending them.

use time::{Duration, OffsetDateTime};
use tokio::sync::broadcast;
use uuid::Uuid;

use db::{Db, LiveSession, NewDevice, TenantId, UserId};
use protocol::{DEVICE_SESSION_LABELS, DeviceInfo, TokenPair, rules::device_name};

use crate::{
    Access, AuthError, Grant,
    token::{Keys, decode_access, encode_access, hash_refresh_token, new_refresh_token},
};

/// How long after a refresh token kept in a cookie was used it may be presented once more
/// without ending the session: long enough for two tabs that refreshed at the same moment.
const SHARED_REFRESH_GRACE: Duration = Duration::seconds(10);
/// Room for session-end notices a slow websocket has not read yet; one that falls further
/// behind re-checks its session anyway.
const ENDED_CAPACITY: usize = 64;
const MAX_PLATFORM_CHARS: usize = 32;

/// The sessions of one tenant's users.
#[derive(Clone, Debug)]
pub(crate) struct Sessions {
    db: Db,
    tenant: TenantId,
    keys: Keys,
    access_ttl: Duration,
    refresh_ttl: Duration,
    /// Users some of whose sessions just ended, so open websockets re-check theirs.
    ended: broadcast::Sender<UserId>,
}

impl Sessions {
    pub(crate) fn new(
        db: Db,
        keys: Keys,
        tenant: TenantId,
        (access_ttl, refresh_ttl): (Duration, Duration),
    ) -> Self {
        Self {
            db,
            tenant,
            keys,
            access_ttl,
            refresh_ttl,
            ended: broadcast::channel(ENDED_CAPACITY).0,
        }
    }

    /// The sessions of another tenant, which hear of the same ended sessions: user ids are
    /// unique over all tenants.
    pub(crate) fn for_tenant(
        &self,
        keys: Keys,
        tenant: TenantId,
        ttls: (Duration, Duration),
    ) -> Self {
        Self {
            ended: self.ended.clone(),
            ..Self::new(self.db.clone(), keys, tenant, ttls)
        }
    }

    pub(crate) fn access_ttl(&self) -> Duration {
        self.access_ttl
    }

    pub(crate) fn refresh_ttl(&self) -> Duration {
        self.refresh_ttl
    }

    /// Starts a session, which exists only from here: after a verified password or provider
    /// login. Tokens cannot create one. A named device is registered to the user first.
    pub(crate) async fn start(
        &self,
        user_id: UserId,
        device: Option<&DeviceInfo>,
    ) -> Result<TokenPair, AuthError> {
        self.require_may_sign_in(user_id).await?;
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

    /// Exchanges a refresh token for a new pair, revoking the old one. Presenting a token
    /// that was already used ends the whole session, except, for a token `shared` by a
    /// browser's tabs, within [`SHARED_REFRESH_GRACE`] of its use: that answers an access
    /// token for the session and no refresh token, since the cookie already holds the next.
    pub(crate) async fn refresh(
        &self,
        refresh_token: &str,
        shared: bool,
    ) -> Result<TokenPair, AuthError> {
        let stored = self
            .db
            .find_refresh_token(self.tenant, &hash_refresh_token(refresh_token))
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        let now = OffsetDateTime::now_utc();
        if let Some(revoked_at) = stored.revoked_at {
            if shared && now - revoked_at <= SHARED_REFRESH_GRACE {
                return self.rejoin(stored.user_id, stored.family_id).await;
            }
            self.end(stored.user_id, stored.family_id).await?;
            Err(AuthError::InvalidCredentials)
        } else if stored.expires_at <= now {
            Err(AuthError::InvalidCredentials)
        } else if self.db.revoke_refresh_token(stored.id).await? {
            self.continue_session(stored.user_id, stored.family_id)
                .await
        } else if shared {
            // Another tab revoked it between the two statements above.
            self.rejoin(stored.user_id, stored.family_id).await
        } else {
            self.end(stored.user_id, stored.family_id).await?;
            Err(AuthError::InvalidCredentials)
        }
    }

    /// An access token for a session that still lives, without a new refresh token.
    async fn rejoin(&self, user_id: UserId, session_id: Uuid) -> Result<TokenPair, AuthError> {
        self.require(session_id, user_id).await?;
        self.pair(user_id, session_id, String::new())
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

    /// Starts a session labelled `label` for an extension of the app on the device of the
    /// app's own session `parent`, ending any earlier one with that label `parent` started.
    /// The pair's refresh token belongs to the extension; the app's own session carries on.
    pub(crate) async fn start_for_extension(
        &self,
        user_id: UserId,
        parent: Uuid,
        label: &str,
    ) -> Result<TokenPair, AuthError> {
        if !DEVICE_SESSION_LABELS.contains(&label) {
            return Err(AuthError::InvalidInput("unknown device session label"));
        }
        self.require_may_sign_in(user_id).await?;
        let session_id = Uuid::now_v7();
        let refresh_token = new_refresh_token();
        let started = self
            .db
            .start_device_session(
                parent,
                user_id,
                label,
                session_id,
                &hash_refresh_token(&refresh_token),
                self.refresh_expiry(),
            )
            .await?;
        if started.is_none() {
            return Err(AuthError::InvalidInput(
                "only an app session that names its device can start another",
            ));
        }
        // The earlier session with this label may have had a websocket open.
        self.ended(user_id);
        self.pair(user_id, session_id, refresh_token)
    }

    /// Ends the session of `refresh_token`, and with the app's own session the sessions of
    /// extensions it started; unknown tokens are ignored.
    pub(crate) async fn logout(&self, refresh_token: &str) -> Result<(), AuthError> {
        if let Some(stored) = self
            .db
            .find_refresh_token(self.tenant, &hash_refresh_token(refresh_token))
            .await?
        {
            self.end(stored.user_id, stored.family_id).await?;
        }
        Ok(())
    }

    async fn end(&self, user_id: UserId, session_id: Uuid) -> Result<(), AuthError> {
        self.db.end_session(session_id).await?;
        self.ended(user_id);
        Ok(())
    }

    /// Tells open websockets that sessions of `user_id` ended, so each checks whether its own
    /// still lives.
    pub(crate) fn ended(&self, user_id: UserId) {
        // An error only means no websocket is open right now.
        let _ = self.ended.send(user_id);
    }

    /// Notices from [`ended`](Self::ended), for every user.
    pub(crate) fn ended_sessions(&self) -> broadcast::Receiver<UserId> {
        self.ended.subscribe()
    }

    /// Whether `grant`'s session still lives.
    pub(crate) async fn is_live(&self, grant: &Grant) -> Result<bool, AuthError> {
        Ok(self
            .db
            .live_session(self.tenant, grant.session_id, grant.user_id)
            .await?
            .is_some())
    }

    /// Validates an access token: a good signature is not enough, its session must still exist.
    pub(crate) async fn verify_access(&self, token: &str) -> Result<Access, AuthError> {
        let claims = decode_access(&self.keys, token)?;
        let session = self.require(claims.sid, claims.sub).await?;
        Ok(Access {
            claims,
            device_id: session.device_id,
        })
    }

    /// The live session `session_id` of `user_id`.
    pub(crate) async fn require(
        &self,
        session_id: Uuid,
        user_id: UserId,
    ) -> Result<LiveSession, AuthError> {
        self.db
            .live_session(self.tenant, session_id, user_id)
            .await?
            .ok_or(AuthError::InvalidCredentials)
    }

    /// The device of `grant`'s session. A session that names none takes `device`, which is
    /// registered to the user first; one that already names a device keeps it.
    pub(crate) async fn device(
        &self,
        grant: &Grant,
        device: &DeviceInfo,
    ) -> Result<Uuid, AuthError> {
        if let Some(current) = grant.device_id {
            return Ok(current);
        }
        let device = new_device(device)?;
        self.db.register_device(grant.user_id, device).await?;
        self.db
            .attach_device(grant.session_id, grant.user_id, device.id)
            .await?;
        Ok(device.id)
    }

    /// Refuses a user who is disabled or marked for deletion, or who is not of this tenant.
    pub(crate) async fn require_may_sign_in(&self, user_id: UserId) -> Result<(), AuthError> {
        let user = self
            .db
            .find_user(user_id)
            .await?
            .filter(|user| user.tenant_id == self.tenant)
            .ok_or(AuthError::InvalidCredentials)?;
        if user.may_sign_in() {
            Ok(())
        } else {
            Err(AuthError::AccountDisabled)
        }
    }

    fn refresh_expiry(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc() + self.refresh_ttl
    }

    fn pair(
        &self,
        user_id: UserId,
        session_id: Uuid,
        refresh_token: String,
    ) -> Result<TokenPair, AuthError> {
        Ok(TokenPair {
            access_token: encode_access(&self.keys, user_id, session_id, self.access_ttl)?,
            refresh_token,
            expires_in: self.access_ttl.whole_seconds(),
        })
    }
}

/// `device` checked against what the server stores: a name of 1-100 characters, kept without
/// surrounding blanks, and a platform of 1-32.
fn new_device(device: &DeviceInfo) -> Result<NewDevice<'_>, AuthError> {
    let name = device_name(&device.name).ok_or(AuthError::InvalidInput(
        "device name must be 1-100 characters",
    ))?;
    let platform = device.platform.trim();
    if (1..=MAX_PLATFORM_CHARS).contains(&platform.chars().count()) {
        Ok(NewDevice {
            id: device.id,
            name,
            platform,
        })
    } else {
        Err(AuthError::InvalidInput(
            "device platform must be 1-32 characters",
        ))
    }
}
