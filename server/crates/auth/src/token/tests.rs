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
    let token = encode_access(&keys(SECRET), user, session, Duration::minutes(5)).expect("encode");
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
    let token = encode_share_access(&keys(SECRET), grant(), Duration::hours(1)).expect("encode");
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
