use sqlx::PgPool;
use time::Duration;

use super::*;
use crate::{AuthConfig, AuthService};

const PASSWORD: &str = "password-1";

fn service(db: &Db) -> AuthService {
    let config = AuthConfig {
        tenant: TenantId::DEFAULT,
        jwt_secret: b"test-secret".to_vec(),
        previous_jwt_secrets: Vec::new(),
        access_ttl: Duration::minutes(15),
        refresh_ttl: Duration::days(30),
    };
    AuthService::new(db.clone(), config).expect("service")
}

async fn other_tenant(pool: &PgPool) -> TenantId {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Other') RETURNING id")
        .fetch_one(pool)
        .await
        .map(TenantId)
        .expect("tenant")
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_created_user_signs_in_with_the_password(pool: PgPool) {
    let db = Db::from_pool(pool);
    create_user(&db, TenantId::DEFAULT, "alice", PASSWORD)
        .await
        .expect("user");
    assert!(service(&db).login("alice", PASSWORD, None).await.is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_user_is_created_in_the_given_tenant(pool: PgPool) {
    let tenant = other_tenant(&pool).await;
    let db = Db::from_pool(pool);
    create_user(&db, tenant, "alice", PASSWORD)
        .await
        .expect("user");
    let found = db
        .find_user_by_username(tenant, "alice")
        .await
        .expect("find");
    let default = db
        .find_user_by_username(TenantId::DEFAULT, "alice")
        .await
        .expect("find");
    assert!(found.is_some());
    assert!(default.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_taken_username_is_refused(pool: PgPool) {
    let db = Db::from_pool(pool);
    create_user(&db, TenantId::DEFAULT, "alice", PASSWORD)
        .await
        .expect("user");
    let again = create_user(&db, TenantId::DEFAULT, "alice", PASSWORD).await;
    assert!(matches!(again, Err(AuthError::UsernameTaken)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_short_password_is_refused(pool: PgPool) {
    let db = Db::from_pool(pool);
    let created = create_user(&db, TenantId::DEFAULT, "alice", "short").await;
    assert!(matches!(created, Err(AuthError::InvalidInput(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_new_password_replaces_the_old_one(pool: PgPool) {
    let db = Db::from_pool(pool);
    let user = create_user(&db, TenantId::DEFAULT, "alice", PASSWORD)
        .await
        .expect("user");
    set_password(&db, user.id, "password-2")
        .await
        .expect("password");
    let auth = service(&db);
    assert!(auth.login("alice", PASSWORD, None).await.is_err());
    assert!(auth.login("alice", "password-2", None).await.is_ok());
}
