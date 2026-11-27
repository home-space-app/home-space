use std::{net::IpAddr, sync::Arc};

use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::sync::Semaphore;

use protocol::NodeId;

use super::*;
use crate::{
    hasher::turn,
    oidc::Identity,
    oidc_login::{FALLBACK_USERNAME, append_query, username_base, username_candidate},
    throttle::{
        FIRST_LOGIN_BLOCK, FREE_ADDRESS_FAILURES, MAX_ADDRESS_BLOCK, MAX_LOGIN_BLOCK,
        address_block, attempt_source, login_block,
    },
};

fn service(pool: PgPool) -> AuthService {
    let config = AuthConfig {
        tenant: TenantId::DEFAULT,
        jwt_secret: b"test-secret".to_vec(),
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
        version: None,
        requires: Vec::new(),
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
        version: None,
        requires: Vec::new(),
    };
    let result = auth.login("alice", "password-1", Some(&device)).await;
    assert!(matches!(result, Err(AuthError::InvalidInput(_))));
}

fn laptop() -> DeviceInfo {
    DeviceInfo {
        id: Uuid::now_v7(),
        name: "Laptop".to_owned(),
        platform: protocol::PLATFORM_LINUX.to_owned(),
        version: None,
        requires: Vec::new(),
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
    assert!(matches!(result, Err(AuthError::WrongLogin)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_fails_for_unknown_user(pool: PgPool) {
    let result = service(pool).login("nobody", "password-1", None).await;
    assert!(matches!(result, Err(AuthError::WrongLogin)));
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
            Err(AuthError::WrongLogin)
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
            Err(AuthError::WrongLogin)
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
            Err(AuthError::WrongLogin)
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

/// Wrong passwords from `address`, each for a username not tried before.
async fn flood(auth: &AuthService, address: Option<IpAddr>) {
    for attempt in 0..FREE_ADDRESS_FAILURES {
        let result = auth
            .login_from(&format!("name-{attempt}"), "wrong-password", None, address)
            .await;
        assert!(matches!(result, Err(AuthError::WrongLogin)), "{result:?}");
    }
}

async fn stored_attempts(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM login_attempts")
        .fetch_one(pool)
        .await
        .expect("count")
}

async fn address_failures(pool: &PgPool, address: &str) -> i32 {
    sqlx::query_scalar("SELECT failures FROM login_attempts WHERE username = '' AND source = $1")
        .bind(address)
        .fetch_one(pool)
        .await
        .expect("failures")
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_address_trying_a_new_username_each_time_has_to_wait(pool: PgPool) {
    let auth = service(pool.clone());
    auth.create_user("alice", "password-1").await.expect("user");
    let flooder = address("198.51.100.7");
    flood(&auth, flooder).await;
    let stored = stored_attempts(&pool).await;
    let wait = retry_after(
        auth.login_from("one-more", "wrong-password", None, flooder)
            .await,
    );
    assert!((29..=31).contains(&wait), "{wait}");
    // Not even the right password of a real user is checked from there...
    retry_after(auth.login_from("alice", "password-1", None, flooder).await);
    // ...nothing more is stored for what is refused...
    assert_eq!(stored_attempts(&pool).await, stored);
    // ...and everyone else signs in as before.
    auth.login_from("alice", "password-1", None, address("192.0.2.1"))
        .await
        .expect("owner");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_known_device_can_sign_in_from_an_address_that_has_to_wait(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let shared = address("198.51.100.7");
    let phone = laptop();
    auth.login_from("alice", "password-1", Some(&phone), shared)
        .await
        .expect("first sign-in");
    flood(&auth, shared).await;
    retry_after(auth.login_from("alice", "password-1", None, shared).await);
    auth.login_from("alice", "password-1", Some(&phone), shared)
        .await
        .expect("known device");
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_wrong_passwords_count_against_an_address(pool: PgPool) {
    let auth = service(pool.clone());
    auth.create_user("alice", "password-1").await.expect("user");
    auth.create_user("bob", "password-2").await.expect("user");
    let home = address("192.0.2.1");
    for _ in 0..3 {
        auth.login_from("alice", "wrong-password", None, home)
            .await
            .expect_err("wrong");
    }
    // Trying again while the username waits checks no password...
    for _ in 0..FREE_ADDRESS_FAILURES {
        retry_after(auth.login_from("alice", "password-1", None, home).await);
    }
    // ...and neither does a sign-in that works, however often.
    for _ in 0..3 {
        auth.login_from("bob", "password-2", None, home)
            .await
            .expect("bob");
    }
    assert_eq!(address_failures(&pool, "192.0.2.1").await, 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_sign_in_without_a_username_is_refused_and_not_counted(pool: PgPool) {
    let auth = service(pool.clone());
    let result = auth
        .login_from("", "password-1", None, address("192.0.2.1"))
        .await;
    assert!(matches!(result, Err(AuthError::WrongLogin)));
    assert_eq!(stored_attempts(&pool).await, 0);
}

#[test]
fn an_address_waits_less_long_than_a_username() {
    assert_eq!(address_block(FREE_ADDRESS_FAILURES - 1), None);
    assert_eq!(
        address_block(FREE_ADDRESS_FAILURES),
        Some(FIRST_LOGIN_BLOCK)
    );
    assert_eq!(
        address_block(FREE_ADDRESS_FAILURES + 1),
        Some(FIRST_LOGIN_BLOCK * 2)
    );
    assert_eq!(address_block(u32::MAX), Some(MAX_ADDRESS_BLOCK));
    assert_eq!(login_block(u32::MAX), Some(MAX_LOGIN_BLOCK));
}

#[tokio::test]
async fn a_password_is_not_hashed_while_all_turns_are_taken() {
    let slots = Arc::new(Semaphore::new(1));
    let wait = std::time::Duration::from_millis(20);
    let taken = turn(&slots, wait).await.expect("first");
    assert!(matches!(
        turn(&slots, wait).await,
        Err(AuthError::Busy { retry_after: 1 })
    ));
    drop(taken);
    let _free = turn(&slots, wait).await.expect("free again");
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
    assert!(matches!(result, Err(AuthError::WrongLogin)));
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
async fn a_stream_ticket_opens_its_file_until_it_expires(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let pair = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let grant = grant_of(&auth, &pair).await;
    let node_id = NodeId::new();
    let issued = auth
        .issue_ticket(
            grant.user_id,
            grant.session_id,
            &TicketRequest::Stream { node_id },
        )
        .expect("ticket");
    assert_eq!(issued.expires_in, 3600);
    let download = TicketRequest::Download { node_id };

    for _ in 0..3 {
        assert!(auth.verify_ticket(&issued.ticket, &download).await.is_ok());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_stream_ticket_opens_nothing_else(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let pair = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let grant = grant_of(&auth, &pair).await;
    let node_id = NodeId::new();
    let ticket = auth
        .issue_ticket(
            grant.user_id,
            grant.session_id,
            &TicketRequest::Stream { node_id },
        )
        .expect("ticket")
        .ticket;
    let others = [
        TicketRequest::Download {
            node_id: NodeId::new(),
        },
        TicketRequest::DownloadVersion {
            node_id,
            version: 1,
        },
        TicketRequest::SharedFile {
            share_id: Uuid::now_v7(),
            node_id,
        },
        TicketRequest::Events,
    ];
    for other in others {
        assert!(
            auth.verify_ticket(&ticket, &other).await.is_err(),
            "{other:?}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_shared_stream_ticket_opens_its_shared_file(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let pair = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let grant = grant_of(&auth, &pair).await;
    let (share_id, node_id) = (Uuid::now_v7(), NodeId::new());
    let ticket = auth
        .issue_ticket(
            grant.user_id,
            grant.session_id,
            &TicketRequest::SharedStream { share_id, node_id },
        )
        .expect("ticket")
        .ticket;
    let file = TicketRequest::SharedFile { share_id, node_id };

    assert!(auth.verify_ticket(&ticket, &file).await.is_ok());
    assert!(auth.verify_ticket(&ticket, &file).await.is_ok());
    let elsewhere = TicketRequest::SharedFile {
        share_id: Uuid::now_v7(),
        node_id,
    };
    assert!(auth.verify_ticket(&ticket, &elsewhere).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_stream_ticket_ends_with_its_session(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let pair = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let grant = grant_of(&auth, &pair).await;
    let node_id = NodeId::new();
    let ticket = auth
        .issue_ticket(
            grant.user_id,
            grant.session_id,
            &TicketRequest::Stream { node_id },
        )
        .expect("ticket")
        .ticket;
    let download = TicketRequest::Download { node_id };
    assert!(auth.verify_ticket(&ticket, &download).await.is_ok());

    auth.logout(&pair.refresh_token).await.expect("logout");

    assert!(auth.verify_ticket(&ticket, &download).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_download_ticket_still_works_once(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let pair = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let grant = grant_of(&auth, &pair).await;
    let download = TicketRequest::Download {
        node_id: NodeId::new(),
    };
    let ticket = auth
        .issue_ticket(grant.user_id, grant.session_id, &download)
        .expect("ticket");
    assert_eq!(ticket.expires_in, 30);

    assert!(auth.verify_ticket(&ticket.ticket, &download).await.is_ok());
    assert!(matches!(
        auth.verify_ticket(&ticket.ticket, &download).await,
        Err(AuthError::InvalidCredentials)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn used_tickets_are_forgotten_once_they_expire(pool: PgPool) {
    let auth = service(pool);
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let (expired, live) = (Uuid::now_v7(), Uuid::now_v7());
    assert!(auth.tickets.use_once(expired, now - 1));
    assert!(auth.tickets.use_once(live, now + 30));
    assert!(!auth.tickets.use_once(live, now + 30));
    let used = auth.tickets.used.lock().expect("lock");
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
async fn a_shared_token_sent_twice_at_once_keeps_the_session(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let first = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let second = auth
        .refresh_shared(&first.refresh_token)
        .await
        .expect("refresh");
    let late = auth
        .refresh_shared(&first.refresh_token)
        .await
        .expect("the tab that came second");
    // It gets into the session, but only the first tab was handed the next token.
    assert!(late.refresh_token.is_empty());
    assert!(auth.verify_access(&late.access_token).await.is_ok());
    assert!(auth.refresh_shared(&second.refresh_token).await.is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_shared_token_reused_later_ends_the_session(pool: PgPool) {
    let auth = service(pool.clone());
    auth.create_user("alice", "password-1").await.expect("user");
    let first = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let second = auth
        .refresh_shared(&first.refresh_token)
        .await
        .expect("refresh");
    sqlx::query("UPDATE refresh_tokens SET revoked_at = revoked_at - interval '11 seconds'")
        .execute(&pool)
        .await
        .expect("age the used token");
    assert!(auth.refresh_shared(&first.refresh_token).await.is_err());
    assert!(auth.refresh_shared(&second.refresh_token).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_token_that_is_not_shared_has_no_grace(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let first = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let second = auth
        .refresh_shared(&first.refresh_token)
        .await
        .expect("refresh");
    assert!(auth.refresh(&first.refresh_token).await.is_err());
    assert!(auth.refresh_shared(&second.refresh_token).await.is_err());
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

#[sqlx::test(migrations = "../../migrations")]
async fn unchecked_logins_do_not_block_the_username_or_address(pool: PgPool) {
    let auth = service(pool.clone());
    auth.create_user("alice", "password-1").await.expect("user");
    // A closed semaphore refuses hashing immediately through the same Busy path as
    // a timeout. Restore capacity after three unchecked attempts.
    auth.hasher.slots.close();
    let home = address("192.0.2.1");
    for _ in 0..3 {
        assert!(matches!(
            auth.login_from("alice", "password-1", None, home).await,
            Err(AuthError::Busy { .. })
        ));
    }
    assert_eq!(address_failures(&pool, "192.0.2.1").await, 0);
    let mut auth = auth;
    auth.hasher.slots = Arc::new(Semaphore::new(1));
    auth.login_from("alice", "password-1", None, home)
        .await
        .expect("not blocked");
}

#[sqlx::test(migrations = "../../migrations")]
async fn unchecked_password_changes_do_not_block_the_session(pool: PgPool) {
    let mut auth = service(pool);
    let user = auth.create_user("alice", "password-1").await.expect("user");
    let pair = auth
        .login("alice", "password-1", None)
        .await
        .expect("login");
    let session = grant_of(&auth, &pair).await.session_id;
    auth.hasher.slots.close();
    for _ in 0..3 {
        assert!(matches!(
            auth.change_password(user.id, session, "password-1", "password-2")
                .await,
            Err(AuthError::Busy { .. })
        ));
    }
    auth.hasher.slots = Arc::new(Semaphore::new(1));
    auth.change_password(user.id, session, "password-1", "password-2")
        .await
        .expect("not blocked");
}
#[sqlx::test(migrations = "../../migrations")]
async fn unchecked_and_blocked_attempts_preserve_real_failures(pool: PgPool) {
    let mut auth = service(pool.clone());
    auth.create_user("alice", "password-1").await.expect("user");
    for _ in 0..2 {
        assert!(matches!(
            auth.login("alice", "wrong", None).await,
            Err(AuthError::WrongLogin)
        ));
    }
    auth.hasher.slots.close();
    assert!(matches!(
        auth.login("alice", "password-1", None).await,
        Err(AuthError::Busy { .. })
    ));
    auth.hasher.slots = Arc::new(Semaphore::new(1));
    assert!(matches!(
        auth.login("alice", "wrong", None).await,
        Err(AuthError::WrongLogin)
    ));
    for _ in 0..3 {
        assert!(matches!(
            auth.login("alice", "password-1", None).await,
            Err(AuthError::TooManyAttempts { .. })
        ));
    }
    let failures: i32 = sqlx::query_scalar(
        "SELECT failures FROM login_attempts WHERE username = 'alice' AND source = ''",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(failures, 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_records_the_client_version(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let auth = service(pool);
    let user = auth.create_user("alice", "password-1").await.expect("user");
    let device = DeviceInfo {
        version: Some("0.13.0".to_owned()),
        requires: vec!["device_remove".to_owned()],
        ..laptop()
    };

    auth.login("alice", "password-1", Some(&device))
        .await
        .expect("login");

    let devices = db.devices(user.id).await.expect("devices");
    assert_eq!(devices[0].client_version.as_deref(), Some("0.13.0"));
    assert_eq!(devices[0].client_requires, ["device_remove"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_hello_from_an_updated_client_records_its_new_version(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let auth = service(pool);
    let user = auth.create_user("alice", "password-1").await.expect("user");
    let device = DeviceInfo {
        version: Some("0.13.0".to_owned()),
        ..laptop()
    };
    let pair = auth
        .login("alice", "password-1", Some(&device))
        .await
        .expect("login");
    let updated = DeviceInfo {
        version: Some("0.14.0".to_owned()),
        ..device.clone()
    };

    auth.session_device(&grant_of(&auth, &pair).await, &updated)
        .await
        .expect("hello");

    let devices = db.devices(user.id).await.expect("devices");
    assert_eq!(devices[0].client_version.as_deref(), Some("0.14.0"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_overlong_client_version_is_refused(pool: PgPool) {
    let auth = service(pool);
    auth.create_user("alice", "password-1").await.expect("user");
    let device = DeviceInfo {
        version: Some("1".repeat(65)),
        ..laptop()
    };
    let result = auth.login("alice", "password-1", Some(&device)).await;
    assert!(matches!(result, Err(AuthError::InvalidInput(_))));
}

mod reauth;
