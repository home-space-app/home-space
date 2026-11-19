//! Access tokens: short-lived JWTs naming a user and their session.

use super::*;

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
pub(super) struct AccessClaims {
    pub(super) typ: String,
    pub(super) tid: i32,
    pub(super) sub: UserId,
    pub(super) sid: Uuid,
    pub(super) iat: i64,
    pub(super) exp: i64,
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
