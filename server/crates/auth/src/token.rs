use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use db::{TenantId, UserId};

use crate::AuthError;

const REFRESH_TOKEN_BYTES: usize = 32;
const KEY_ID_BYTES: usize = 6;
/// The `typ` claim of an access token and of a ticket. Both are signed with the same key, so
/// each names what it is and is refused when presented as the other.
const TYPE_ACCESS: &str = "access";
const TYPE_TICKET: &str = "ticket";
/// The `typ` claim of the access token of an unlocked share link.
const TYPE_SHARE: &str = "share";

/// Signing keys of one tenant: tokens are signed with the current secret and verified against
/// the current and any previous secrets, so the secret can be rotated without ending sessions.
///
/// Every token names the tenant in its `tid` claim, and one naming another tenant is refused
/// even when its signature is good, so a secret two tenants share by mistake still keeps
/// their tokens apart.
#[derive(Clone)]
pub struct Keys {
    tenant: TenantId,
    current: Vec<u8>,
    previous: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys")
            .field("tenant", &self.tenant)
            .field("previous", &self.previous.len())
            .finish_non_exhaustive()
    }
}

impl Keys {
    pub fn new(tenant: TenantId, current: Vec<u8>, previous: Vec<Vec<u8>>) -> Self {
        Self {
            tenant,
            current,
            previous,
        }
    }

    /// Whether a token's `tid` claim names the tenant of these keys.
    fn is_own(&self, tid: i32) -> bool {
        tid == self.tenant.0
    }

    /// Identifier stored in the token header: a truncated hash, never the secret itself.
    fn key_id(secret: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(&Sha256::digest(secret)[..KEY_ID_BYTES])
    }

    fn header(&self) -> Header {
        Header {
            kid: Some(Self::key_id(&self.current)),
            ..Header::default()
        }
    }

    /// The secret a token was signed with. A token without a key id predates rotation support
    /// and is checked against the current secret only; an unknown key id is rejected.
    fn secret_for(&self, token: &str) -> Result<&[u8], AuthError> {
        let kid = decode_header(token)
            .map_err(|_| AuthError::InvalidCredentials)?
            .kid;
        match kid {
            None => Ok(&self.current),
            Some(kid) => std::iter::once(&self.current)
                .chain(&self.previous)
                .find(|secret| Self::key_id(secret) == kid)
                .map(Vec::as_slice)
                .ok_or(AuthError::InvalidCredentials),
        }
    }
}

/// Whom an access token was issued to.
#[derive(Clone, Copy, Debug)]
pub struct Claims {
    pub sub: UserId,
    /// The session this token belongs to; it is only valid while that session exists.
    pub sid: Uuid,
    pub iat: i64,
    pub exp: i64,
}

/// Claims carried by an access token.
///
/// Unknown claims are rejected as well as a wrong `typ`, so a ticket has two reasons not to
/// pass as an access token.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessClaims {
    typ: String,
    tid: i32,
    sub: UserId,
    sid: Uuid,
    iat: i64,
    exp: i64,
}

/// Signs an access token for `user_id` valid for `ttl`.
pub fn encode_access(
    keys: &Keys,
    user_id: UserId,
    session_id: Uuid,
    ttl: Duration,
) -> Result<String, AuthError> {
    let now = OffsetDateTime::now_utc();
    let claims = AccessClaims {
        typ: TYPE_ACCESS.to_owned(),
        tid: keys.tenant.0,
        sub: user_id,
        sid: session_id,
        iat: now.unix_timestamp(),
        exp: (now + ttl).unix_timestamp(),
    };
    Ok(encode(
        &keys.header(),
        &claims,
        &EncodingKey::from_secret(&keys.current)?,
    )?)
}

/// Validates signature and expiry of an access token.
pub fn decode_access(keys: &Keys, token: &str) -> Result<Claims, AuthError> {
    let validation = Validation {
        leeway: 0,
        ..Validation::default()
    };
    let secret = keys.secret_for(token)?;
    let claims = decode::<AccessClaims>(token, &DecodingKey::from_secret(secret)?, &validation)
        .map(|data| data.claims)
        .map_err(|_| AuthError::InvalidCredentials)?;
    if claims.typ == TYPE_ACCESS && keys.is_own(claims.tid) {
        Ok(Claims {
            sub: claims.sub,
            sid: claims.sid,
            iat: claims.iat,
            exp: claims.exp,
        })
    } else {
        Err(AuthError::InvalidCredentials)
    }
}

#[derive(Serialize, Deserialize)]
struct TicketClaims {
    typ: String,
    tid: i32,
    sub: UserId,
    sid: Uuid,
    iat: i64,
    exp: i64,
    tkt: String,
    /// Names this one ticket, so that it can be refused once it has been used.
    jti: Uuid,
}

/// Signs a short-lived ticket for `user_id`'s session limited to `scope`.
pub fn encode_ticket(
    keys: &Keys,
    user_id: UserId,
    session_id: Uuid,
    scope: &str,
    ttl: Duration,
) -> Result<String, AuthError> {
    let now = OffsetDateTime::now_utc();
    let claims = TicketClaims {
        typ: TYPE_TICKET.to_owned(),
        tid: keys.tenant.0,
        sub: user_id,
        sid: session_id,
        iat: now.unix_timestamp(),
        exp: (now + ttl).unix_timestamp(),
        tkt: scope.to_owned(),
        jti: Uuid::now_v7(),
    };
    Ok(encode(
        &keys.header(),
        &claims,
        &EncodingKey::from_secret(&keys.current)?,
    )?)
}

/// Who a ticket was issued to, and which ticket it is.
#[derive(Clone, Copy, Debug)]
pub struct TicketGrant {
    pub user_id: UserId,
    pub session_id: Uuid,
    /// Unique to the ticket.
    pub id: Uuid,
    /// Unix timestamp at which the ticket expires.
    pub expires_at: i64,
}

/// Validates a ticket's signature, expiry and scope, returning whom it was issued to.
pub fn decode_ticket(keys: &Keys, token: &str, scope: &str) -> Result<TicketGrant, AuthError> {
    let validation = Validation {
        leeway: 0,
        ..Validation::default()
    };
    let secret = keys.secret_for(token)?;
    let claims = decode::<TicketClaims>(token, &DecodingKey::from_secret(secret)?, &validation)
        .map(|data| data.claims)
        .map_err(|_| AuthError::InvalidCredentials)?;
    if claims.typ == TYPE_TICKET && claims.tkt == scope && keys.is_own(claims.tid) {
        Ok(TicketGrant {
            user_id: claims.sub,
            session_id: claims.sid,
            id: claims.jti,
            expires_at: claims.exp,
        })
    } else {
        Err(AuthError::InvalidCredentials)
    }
}

/// Claims carried by the access token of an unlocked share link. Unknown claims are rejected,
/// so neither an access token nor a ticket passes as one.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShareClaims {
    typ: String,
    tid: i32,
    shr: Uuid,
    /// The password version it was granted for.
    pwv: i32,
    iat: i64,
    exp: i64,
}

/// Which share link, and which of its passwords, an access token was granted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShareGrant {
    pub share_id: Uuid,
    pub password_version: i32,
}

/// Signs proof that the password of version `grant.password_version` of a share link was
/// given, valid for `ttl`.
pub fn encode_share_access(
    keys: &Keys,
    grant: ShareGrant,
    ttl: Duration,
) -> Result<String, AuthError> {
    let now = OffsetDateTime::now_utc();
    let claims = ShareClaims {
        typ: TYPE_SHARE.to_owned(),
        tid: keys.tenant.0,
        shr: grant.share_id,
        pwv: grant.password_version,
        iat: now.unix_timestamp(),
        exp: (now + ttl).unix_timestamp(),
    };
    Ok(encode(
        &keys.header(),
        &claims,
        &EncodingKey::from_secret(&keys.current)?,
    )?)
}

/// Validates signature and expiry of a share access token, returning what it was granted for.
pub fn decode_share_access(keys: &Keys, token: &str) -> Result<ShareGrant, AuthError> {
    let validation = Validation {
        leeway: 0,
        ..Validation::default()
    };
    let secret = keys.secret_for(token)?;
    let claims = decode::<ShareClaims>(token, &DecodingKey::from_secret(secret)?, &validation)
        .map(|data| data.claims)
        .map_err(|_| AuthError::InvalidCredentials)?;
    if claims.typ == TYPE_SHARE && keys.is_own(claims.tid) {
        Ok(ShareGrant {
            share_id: claims.shr,
            password_version: claims.pwv,
        })
    } else {
        Err(AuthError::InvalidCredentials)
    }
}

/// Generates a random opaque refresh token.
pub fn new_refresh_token() -> String {
    let mut bytes = [0u8; REFRESH_TOKEN_BYTES];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generates the random secret of a share link: 32 bytes, base64url.
pub fn new_share_token() -> String {
    new_refresh_token()
}

/// Hash under which a refresh token is stored.
pub fn hash_refresh_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"test-secret";

    fn keys(secret: &[u8]) -> Keys {
        Keys::new(TenantId::DEFAULT, secret.to_vec(), Vec::new())
    }

    /// The keys of another tenant that was given the same secret.
    fn other_tenant(secret: &[u8]) -> Keys {
        Keys::new(TenantId(2), secret.to_vec(), Vec::new())
    }

    #[test]
    fn access_token_round_trips() {
        let user = UserId::new();
        let session = Uuid::now_v7();
        let token =
            encode_access(&keys(SECRET), user, session, Duration::minutes(5)).expect("encode");
        let claims = decode_access(&keys(SECRET), &token).expect("decode");
        assert_eq!(claims.sub, user);
        assert_eq!(claims.sid, session);
    }

    #[test]
    fn expired_access_token_is_rejected() {
        let token = encode_access(
            &keys(SECRET),
            UserId::new(),
            Uuid::now_v7(),
            Duration::seconds(-10),
        )
        .expect("encode");
        assert!(decode_access(&keys(SECRET), &token).is_err());
    }

    #[test]
    fn access_token_with_other_secret_is_rejected() {
        let token = encode_access(
            &keys(SECRET),
            UserId::new(),
            Uuid::now_v7(),
            Duration::minutes(5),
        )
        .expect("encode");
        assert!(decode_access(&keys(b"other"), &token).is_err());
    }

    fn grant() -> ShareGrant {
        ShareGrant {
            share_id: Uuid::now_v7(),
            password_version: 3,
        }
    }

    #[test]
    fn share_access_round_trips() {
        let grant = grant();
        let token = encode_share_access(&keys(SECRET), grant, Duration::hours(1)).expect("encode");
        assert_eq!(
            decode_share_access(&keys(SECRET), &token).expect("decode"),
            grant
        );
    }

    #[test]
    fn expired_share_access_is_rejected() {
        let token =
            encode_share_access(&keys(SECRET), grant(), Duration::seconds(-10)).expect("encode");
        assert!(decode_share_access(&keys(SECRET), &token).is_err());
    }

    #[test]
    fn access_token_is_not_share_access() {
        let token = encode_access(
            &keys(SECRET),
            UserId::new(),
            Uuid::now_v7(),
            Duration::minutes(5),
        )
        .expect("encode");
        assert!(decode_share_access(&keys(SECRET), &token).is_err());
    }

    #[test]
    fn share_access_is_not_an_access_token() {
        let token =
            encode_share_access(&keys(SECRET), grant(), Duration::hours(1)).expect("encode");
        assert!(decode_access(&keys(SECRET), &token).is_err());
    }

    #[test]
    fn ticket_round_trips_for_its_scope() {
        let user = UserId::new();
        let session = Uuid::now_v7();
        let ticket = encode_ticket(
            &keys(SECRET),
            user,
            session,
            "events",
            Duration::seconds(60),
        )
        .expect("encode");
        let grant = decode_ticket(&keys(SECRET), &ticket, "events").expect("decode");
        assert_eq!((grant.user_id, grant.session_id), (user, session));
    }

    #[test]
    fn access_token_without_a_session_is_rejected() {
        #[derive(Serialize)]
        struct Legacy {
            sub: UserId,
            iat: i64,
            exp: i64,
        }
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let claims = Legacy {
            sub: UserId::new(),
            iat: now,
            exp: now + 300,
        };
        let k = keys(SECRET);
        let token = encode(
            &k.header(),
            &claims,
            &EncodingKey::from_secret(&k.current).expect("key"),
        )
        .expect("encode");
        assert!(decode_access(&k, &token).is_err());
    }

    #[test]
    fn every_ticket_has_its_own_id() {
        let (user, session) = (UserId::new(), Uuid::now_v7());
        let id = || {
            let ticket = encode_ticket(
                &keys(SECRET),
                user,
                session,
                "events",
                Duration::seconds(60),
            )
            .expect("encode");
            decode_ticket(&keys(SECRET), &ticket, "events")
                .expect("decode")
                .id
        };
        assert_ne!(id(), id());
    }

    #[test]
    fn ticket_is_rejected_for_another_scope() {
        let ticket = encode_ticket(
            &keys(SECRET),
            UserId::new(),
            Uuid::now_v7(),
            "events",
            Duration::seconds(60),
        )
        .expect("encode");
        assert!(decode_ticket(&keys(SECRET), &ticket, "download:x").is_err());
    }

    #[test]
    fn expired_ticket_is_rejected() {
        let ticket = encode_ticket(
            &keys(SECRET),
            UserId::new(),
            Uuid::now_v7(),
            "events",
            Duration::seconds(-10),
        )
        .expect("encode");
        assert!(decode_ticket(&keys(SECRET), &ticket, "events").is_err());
    }

    #[test]
    fn ticket_is_not_an_access_token() {
        let ticket = encode_ticket(
            &keys(SECRET),
            UserId::new(),
            Uuid::now_v7(),
            "events",
            Duration::seconds(60),
        )
        .expect("encode");
        assert!(decode_access(&keys(SECRET), &ticket).is_err());
    }

    #[test]
    fn access_token_is_not_a_ticket() {
        let token = encode_access(
            &keys(SECRET),
            UserId::new(),
            Uuid::now_v7(),
            Duration::minutes(5),
        )
        .expect("encode");
        assert!(decode_ticket(&keys(SECRET), &token, "events").is_err());
    }

    /// A token shaped exactly like an access token is still refused under another `typ`.
    #[test]
    fn access_token_of_another_type_is_rejected() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let k = keys(SECRET);
        for typ in [TYPE_TICKET, ""] {
            let claims = AccessClaims {
                typ: typ.to_owned(),
                tid: k.tenant.0,
                sub: UserId::new(),
                sid: Uuid::now_v7(),
                iat: now,
                exp: now + 300,
            };
            let token = encode(
                &k.header(),
                &claims,
                &EncodingKey::from_secret(&k.current).expect("key"),
            )
            .expect("encode");
            assert!(decode_access(&k, &token).is_err(), "{typ:?}");
        }
    }

    #[test]
    fn ticket_of_another_type_is_rejected() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let k = keys(SECRET);
        let claims = TicketClaims {
            typ: TYPE_ACCESS.to_owned(),
            tid: k.tenant.0,
            sub: UserId::new(),
            sid: Uuid::now_v7(),
            iat: now,
            exp: now + 30,
            tkt: "events".to_owned(),
            jti: Uuid::now_v7(),
        };
        let token = encode(
            &k.header(),
            &claims,
            &EncodingKey::from_secret(&k.current).expect("key"),
        )
        .expect("encode");
        assert!(decode_ticket(&k, &token, "events").is_err());
    }

    #[test]
    fn token_signed_with_a_previous_secret_is_accepted() {
        let user = UserId::new();
        let token = encode_access(
            &keys(b"old-secret"),
            user,
            Uuid::now_v7(),
            Duration::minutes(5),
        )
        .expect("encode");
        let rotated = Keys::new(
            TenantId::DEFAULT,
            b"new-secret".to_vec(),
            vec![b"old-secret".to_vec()],
        );
        assert_eq!(decode_access(&rotated, &token).expect("decode").sub, user);
    }

    #[test]
    fn token_signed_with_a_dropped_secret_is_rejected() {
        let token = encode_access(
            &keys(b"old-secret"),
            UserId::new(),
            Uuid::now_v7(),
            Duration::minutes(5),
        )
        .expect("encode");
        assert!(decode_access(&keys(b"new-secret"), &token).is_err());
    }

    #[test]
    fn new_tokens_use_the_current_secret() {
        let rotated = Keys::new(
            TenantId::DEFAULT,
            b"new-secret".to_vec(),
            vec![b"old-secret".to_vec()],
        );
        let token = encode_access(
            &rotated,
            UserId::new(),
            Uuid::now_v7(),
            Duration::minutes(5),
        )
        .expect("encode");
        assert!(decode_access(&keys(b"new-secret"), &token).is_ok());
        assert!(decode_access(&keys(b"old-secret"), &token).is_err());
    }

    #[test]
    fn refresh_tokens_are_unique() {
        assert_ne!(new_refresh_token(), new_refresh_token());
    }

    #[test]
    fn tokens_of_another_tenant_are_refused_even_with_the_same_secret() {
        let ours = keys(SECRET);
        let theirs = other_tenant(SECRET);
        let user = UserId::new();
        let session = Uuid::now_v7();

        let access = encode_access(&theirs, user, session, Duration::minutes(5)).expect("access");
        assert!(decode_access(&ours, &access).is_err());
        assert!(decode_access(&theirs, &access).is_ok());

        let ticket =
            encode_ticket(&theirs, user, session, "events", Duration::seconds(30)).expect("ticket");
        assert!(decode_ticket(&ours, &ticket, "events").is_err());
        assert!(decode_ticket(&theirs, &ticket, "events").is_ok());

        let share = encode_share_access(&theirs, grant(), Duration::hours(1)).expect("share");
        assert!(decode_share_access(&ours, &share).is_err());
        assert!(decode_share_access(&theirs, &share).is_ok());
    }

    #[test]
    fn tokens_from_before_tenants_are_refused() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let k = keys(SECRET);
        let sign = |claims: serde_json::Value| {
            encode(
                &k.header(),
                &claims,
                &EncodingKey::from_secret(&k.current).expect("key"),
            )
            .expect("encode")
        };
        let (sub, sid) = (UserId::new(), Uuid::now_v7());
        let access = sign(serde_json::json!({
            "typ": TYPE_ACCESS, "sub": sub, "sid": sid, "iat": now, "exp": now + 300,
        }));
        assert!(decode_access(&k, &access).is_err());
        let ticket = sign(serde_json::json!({
            "typ": TYPE_TICKET, "sub": sub, "sid": sid, "iat": now, "exp": now + 30,
            "tkt": "events", "jti": Uuid::now_v7(),
        }));
        assert!(decode_ticket(&k, &ticket, "events").is_err());
        let share = sign(serde_json::json!({
            "typ": TYPE_SHARE, "shr": Uuid::now_v7(), "pwv": 1, "iat": now, "exp": now + 300,
        }));
        assert!(decode_share_access(&k, &share).is_err());
    }
}
