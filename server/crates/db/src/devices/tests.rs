use super::*;

const LAPTOP: &str = "Laptop";

fn device(id: Uuid) -> NewDevice<'static> {
    NewDevice {
        id,
        name: LAPTOP,
        platform: "desktop-linux",
        version: None,
        requires: &[],
    }
}

async fn session(db: &Db, user: UserId, device: Option<Uuid>) -> Uuid {
    let id = Uuid::now_v7();
    let expires = OffsetDateTime::now_utc() + time::Duration::days(1);
    db.start_session(id, user, device, id.as_bytes(), expires)
        .await
        .expect("session");
    id
}

#[sqlx::test(migrations = "../../migrations")]
async fn signing_in_again_keeps_the_name_the_user_chose(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");
    db.rename_device(alice, id, "Work").await.expect("rename");

    db.register_device(alice, device(id)).await.expect("again");

    let devices = db.devices(alice).await.expect("devices");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Work");
}

#[sqlx::test(migrations = "../../migrations")]
async fn users_of_one_install_each_get_their_own_device(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(crate::TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id)).await.expect("alice");
    db.register_device(bob, device(id)).await.expect("bob");

    assert!(matches!(
        db.rename_device(bob, Uuid::now_v7(), "x").await,
        Err(DbError::NotFound)
    ));
    db.rename_device(bob, id, "Bob's").await.expect("rename");
    assert_eq!(db.devices(alice).await.expect("alice")[0].name, LAPTOP);
}

#[sqlx::test(migrations = "../../migrations")]
async fn signing_out_a_device_ends_only_its_sessions(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let (laptop, phone) = (Uuid::now_v7(), Uuid::now_v7());
    db.register_device(alice, device(laptop))
        .await
        .expect("laptop");
    db.register_device(alice, device(phone))
        .await
        .expect("phone");
    let on_laptop = session(&db, alice, Some(laptop)).await;
    let on_phone = session(&db, alice, Some(phone)).await;
    let unnamed = session(&db, alice, None).await;

    db.sign_out_device(alice, laptop).await.expect("sign out");

    assert_eq!(
        db.live_session(crate::TenantId::DEFAULT, on_laptop, alice)
            .await
            .expect("laptop"),
        None
    );
    assert!(
        db.live_session(crate::TenantId::DEFAULT, on_phone, alice)
            .await
            .expect("phone")
            .is_some()
    );
    assert!(
        db.live_session(crate::TenantId::DEFAULT, unnamed, alice)
            .await
            .expect("unnamed")
            .is_some()
    );
    let signed_in = db
        .devices(alice)
        .await
        .expect("devices")
        .into_iter()
        .map(|device| (device.id, device.signed_in))
        .collect::<Vec<_>>();
    assert_eq!(signed_in.len(), 2);
    assert!(signed_in.contains(&(laptop, false)));
    assert!(signed_in.contains(&(phone, true)));
}

async fn last_seen(db: &Db, user: UserId, id: Uuid, days_ago: i64) {
    sqlx::query(
        "UPDATE devices SET last_seen_at = now() - make_interval(days => $3) \
         WHERE user_id = $1 AND id = $2",
    )
    .bind(user)
    .bind(id)
    .bind(i32::try_from(days_ago).expect("days"))
    .execute(db.pool())
    .await
    .expect("last seen");
}

fn days_ago(days: i64) -> OffsetDateTime {
    OffsetDateTime::now_utc() - time::Duration::days(days)
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_signed_out_for_long_is_forgotten(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");
    last_seen(&db, alice, id, 31).await;

    assert_eq!(db.prune_devices(days_ago(30)).await.expect("prune"), 1);
    assert!(db.devices(alice).await.expect("devices").is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_signed_out_recently_is_kept(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");
    last_seen(&db, alice, id, 2).await;

    assert_eq!(db.prune_devices(days_ago(30)).await.expect("prune"), 0);
    assert_eq!(db.devices(alice).await.expect("devices").len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_signed_in_device_is_kept(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");
    session(&db, alice, Some(id)).await;
    last_seen(&db, alice, id, 31).await;

    assert_eq!(db.prune_devices(days_ago(30)).await.expect("prune"), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn touching_a_device_marks_it_seen(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");
    last_seen(&db, alice, id, 2).await;

    db.touch_device(alice, id).await.expect("touch");

    let seen = db.devices(alice).await.expect("devices")[0].last_seen_at;
    assert!(seen > days_ago(1));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_session_without_a_device_takes_the_one_attached(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");
    let unnamed = session(&db, alice, None).await;

    db.attach_device(unnamed, alice, id).await.expect("attach");

    let live = db
        .live_session(crate::TenantId::DEFAULT, unnamed, alice)
        .await
        .expect("live");
    assert_eq!(live.map(|live| live.device_id), Some(Some(id)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_session_keeps_the_device_it_has(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let (laptop, other) = (Uuid::now_v7(), Uuid::now_v7());
    db.register_device(alice, device(laptop))
        .await
        .expect("laptop");
    db.register_device(alice, device(other))
        .await
        .expect("other");
    let on_laptop = session(&db, alice, Some(laptop)).await;

    db.attach_device(on_laptop, alice, other)
        .await
        .expect("attach");

    let live = db
        .live_session(crate::TenantId::DEFAULT, on_laptop, alice)
        .await
        .expect("live");
    assert_eq!(live.map(|live| live.device_id), Some(Some(laptop)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_users_device_cannot_be_signed_out(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(crate::TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");
    let on_laptop = session(&db, alice, Some(id)).await;

    assert!(matches!(
        db.sign_out_device(bob, id).await,
        Err(DbError::NotFound)
    ));
    assert!(
        db.live_session(crate::TenantId::DEFAULT, on_laptop, alice)
            .await
            .expect("live")
            .is_some()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn visibility_decides_who_can_send_to_a_device(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(crate::TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let carol = db
        .create_user(crate::TenantId::DEFAULT, "carol", "x")
        .await
        .expect("carol")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");
    let can = async |viewer| db.can_send_to(viewer, alice, id).await.expect("can");

    assert_eq!(
        db.devices(alice).await.expect("devices")[0].visibility,
        DeviceVisibility::Owner
    );
    assert!(can(alice).await);
    assert!(!can(bob).await);

    db.set_device_visibility(alice, id, DeviceVisibility::Hidden, &[])
        .await
        .expect("hidden");
    assert!(!can(alice).await);
    assert!(!can(bob).await);

    let affected = db
        .set_device_visibility(alice, id, DeviceVisibility::Users, &[bob])
        .await
        .expect("users");
    assert_eq!(affected, vec![bob]);
    assert!(can(alice).await);
    assert!(can(bob).await);
    assert!(!can(carol).await);
    assert_eq!(db.device_viewers(alice).await.expect("viewers").len(), 1);

    db.set_device_visibility(alice, id, DeviceVisibility::Everyone, &[bob])
        .await
        .expect("everyone");
    assert!(can(carol).await);
    assert!(db.device_viewers(alice).await.expect("viewers").is_empty());

    let targets = db.transfer_targets(bob).await.expect("targets");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].owner_name, "alice");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_cannot_be_shown_to_its_owner_or_unknown_users(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(crate::TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");

    for viewers in [vec![alice], vec![UserId::new()]] {
        assert!(matches!(
            db.set_device_visibility(alice, id, DeviceVisibility::Users, &viewers)
                .await,
            Err(DbError::Invalid)
        ));
    }
    assert!(matches!(
        db.set_device_visibility(bob, id, DeviceVisibility::Everyone, &[])
            .await,
        Err(DbError::NotFound)
    ));
    assert_eq!(
        db.devices(alice).await.expect("devices")[0].visibility,
        DeviceVisibility::Owner
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn devices_are_seen_and_shown_only_within_their_tenant(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let other = crate::tenants::add_test_tenant(&db).await;
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(crate::TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let mallory = db
        .create_user(other, "mallory", "x")
        .await
        .expect("mallory")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");

    let affected = db
        .set_device_visibility(alice, id, DeviceVisibility::Everyone, &[])
        .await
        .expect("everyone");
    assert_eq!(affected, vec![bob]);
    assert!(
        db.transfer_targets(mallory)
            .await
            .expect("targets")
            .is_empty()
    );
    assert!(!db.can_send_to(mallory, alice, id).await.expect("can send"));
    assert!(db.can_send_to(bob, alice, id).await.expect("can send"));

    assert!(matches!(
        db.set_device_visibility(alice, id, DeviceVisibility::Users, &[mallory])
            .await,
        Err(DbError::Invalid)
    ));
    assert!(db.device_viewers(alice).await.expect("viewers").is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn removing_a_device_forgets_it_and_its_sessions(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(crate::TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let (laptop, phone) = (Uuid::now_v7(), Uuid::now_v7());
    db.register_device(alice, device(laptop))
        .await
        .expect("laptop");
    db.register_device(alice, device(phone))
        .await
        .expect("phone");
    db.set_device_visibility(alice, laptop, DeviceVisibility::Users, &[bob])
        .await
        .expect("visible to bob");
    let on_laptop = session(&db, alice, Some(laptop)).await;

    let affected = db.remove_device(alice, laptop).await.expect("remove");

    assert_eq!(affected, vec![bob]);
    let devices = db.devices(alice).await.expect("devices");
    assert_eq!(
        devices.iter().map(|device| device.id).collect::<Vec<_>>(),
        vec![phone]
    );
    assert_eq!(
        db.live_session(crate::TenantId::DEFAULT, on_laptop, alice)
            .await
            .expect("session"),
        None
    );
    assert!(db.device_viewers(alice).await.expect("viewers").is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_the_owner_can_remove_a_device(pool: sqlx::PgPool) {
    let db = Db::from_pool(pool);
    let alice = db
        .create_user(crate::TenantId::DEFAULT, "alice", "x")
        .await
        .expect("alice")
        .id;
    let bob = db
        .create_user(crate::TenantId::DEFAULT, "bob", "x")
        .await
        .expect("bob")
        .id;
    let id = Uuid::now_v7();
    db.register_device(alice, device(id))
        .await
        .expect("register");

    assert!(matches!(
        db.remove_device(bob, id).await,
        Err(DbError::NotFound)
    ));
    assert_eq!(db.devices(alice).await.expect("devices").len(), 1);
}
