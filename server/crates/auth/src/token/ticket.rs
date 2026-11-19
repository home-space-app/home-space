//! Tickets: single-use JWTs for one download or the events socket, for requests that cannot send headers.

use super::*;

#[derive(Serialize, Deserialize)]
pub(super) struct TicketClaims {
    pub(super) typ: String,
    pub(super) tid: i32,
    pub(super) sub: UserId,
    pub(super) sid: Uuid,
    pub(super) iat: i64,
    pub(super) exp: i64,
    pub(super) tkt: String,
    /// Names this one ticket, so that it can be refused once it has been used.
    pub(super) jti: Uuid,
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
