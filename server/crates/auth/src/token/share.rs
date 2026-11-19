//! Share-access tokens, handed out once a password-protected share link is unlocked.

use super::*;

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
