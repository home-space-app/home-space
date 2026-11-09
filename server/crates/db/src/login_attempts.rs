use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{Db, DbError};

impl Db {
    /// Counts a sign-in attempt for `username` from `source`, as a failure until
    /// [`clear_login_attempts`](Self::clear_login_attempts) takes it back, unless attempts for
    /// that pair are blocked. Other sources of the same username are not affected, so a block
    /// cannot be used to keep the owner out. `block` maps the number of failures, this attempt included, to how long the
    /// next attempt must wait, if at all.
    ///
    /// Returns `Err` with the time the block ends when the attempt must not be checked. Counting
    /// before the password is checked keeps a burst of parallel guesses from all getting through
    /// before the first one is recorded.
    pub async fn begin_login_attempt(
        &self,
        username: &str,
        source: &str,
        block: impl FnOnce(u32) -> Option<Duration>,
    ) -> Result<Result<(), OffsetDateTime>, DbError> {
        let mut tx = self.pool().begin().await?;
        sqlx::query(
            "INSERT INTO login_attempts (username, source) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(username)
        .bind(source)
        .execute(&mut *tx)
        .await?;
        let (failures, blocked_until) = sqlx::query_as::<_, (i32, Option<OffsetDateTime>)>(
            "SELECT failures, blocked_until FROM login_attempts \
             WHERE username = $1 AND source = $2 FOR UPDATE",
        )
        .bind(username)
        .bind(source)
        .fetch_one(&mut *tx)
        .await?;
        let now = OffsetDateTime::now_utc();
        if let Some(until) = blocked_until.filter(|until| *until > now) {
            tx.commit().await?;
            return Ok(Err(until));
        }
        let failures = failures.saturating_add(1);
        let blocked_until = block(failures.unsigned_abs()).map(|wait| now + wait);
        sqlx::query(
            "UPDATE login_attempts SET failures = $3, blocked_until = $4, last_failed_at = $5 \
             WHERE username = $1 AND source = $2",
        )
        .bind(username)
        .bind(source)
        .bind(failures)
        .bind(blocked_until)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok(()))
    }

    /// Whether `device_id` is a device the user called `username` has signed in from before.
    /// One query whether or not there is such a user, so that it takes as long either way.
    pub async fn is_device_of_username(
        &self,
        username: &str,
        device_id: Uuid,
    ) -> Result<bool, DbError> {
        let known = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM devices d JOIN users u ON u.id = d.user_id \
             WHERE u.username = $1 AND d.id = $2)",
        )
        .bind(username)
        .bind(device_id)
        .fetch_one(self.pool())
        .await?;
        Ok(known)
    }

    /// Forgets the failed attempts of `username` from `source`, after a correct password.
    /// Those from other sources stay, so a guesser does not start afresh when the owner signs in.
    pub async fn clear_login_attempts(&self, username: &str, source: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM login_attempts WHERE username = $1 AND source = $2")
            .bind(username)
            .bind(source)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Forgets the failed attempts of usernames that are not blocked and have not failed since
    /// `before`, returning how many were forgotten.
    pub async fn prune_login_attempts(&self, before: OffsetDateTime) -> Result<u64, DbError> {
        let pruned = sqlx::query(
            "DELETE FROM login_attempts WHERE last_failed_at < $1 \
             AND (blocked_until IS NULL OR blocked_until <= now())",
        )
        .bind(before)
        .execute(self.pool())
        .await?;
        Ok(pruned.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    const HOME: &str = "192.0.2.1";
    const ELSEWHERE: &str = "198.51.100.7";

    fn after_two(failures: u32) -> Option<Duration> {
        (failures >= 2).then_some(Duration::minutes(1))
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn attempts_are_blocked_once_the_policy_says_so(pool: PgPool) {
        let db = Db::from_pool(pool);
        assert_eq!(
            db.begin_login_attempt("alice", HOME, after_two)
                .await
                .expect("db"),
            Ok(())
        );
        assert_eq!(
            db.begin_login_attempt("alice", HOME, after_two)
                .await
                .expect("db"),
            Ok(())
        );
        let blocked = db
            .begin_login_attempt("alice", HOME, after_two)
            .await
            .expect("db");
        assert!(blocked.is_err());
        assert_eq!(
            db.begin_login_attempt("bob", HOME, after_two)
                .await
                .expect("db"),
            Ok(())
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn clearing_starts_the_count_again(pool: PgPool) {
        let db = Db::from_pool(pool);
        db.begin_login_attempt("alice", HOME, |_| None)
            .await
            .expect("db")
            .expect("free");
        db.clear_login_attempts("alice", HOME).await.expect("clear");
        let failures = std::cell::Cell::new(0);
        db.begin_login_attempt("alice", HOME, |count| {
            failures.set(count);
            None
        })
        .await
        .expect("db")
        .expect("free");
        assert_eq!(failures.get(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn pruning_keeps_blocked_usernames(pool: PgPool) {
        let db = Db::from_pool(pool);
        db.begin_login_attempt("alice", HOME, |_| None)
            .await
            .expect("db")
            .expect("free");
        db.begin_login_attempt("bob", HOME, |_| Some(Duration::hours(1)))
            .await
            .expect("db")
            .expect("free");
        let later = OffsetDateTime::now_utc() + Duration::seconds(1);
        assert_eq!(db.prune_login_attempts(later).await.expect("prune"), 1);
        assert!(
            db.begin_login_attempt("bob", HOME, |_| None)
                .await
                .expect("db")
                .is_err()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_block_holds_only_for_the_source_that_earned_it(pool: PgPool) {
        let db = Db::from_pool(pool);
        for _ in 0..2 {
            db.begin_login_attempt("alice", ELSEWHERE, after_two)
                .await
                .expect("db")
                .expect("free");
        }
        assert!(
            db.begin_login_attempt("alice", ELSEWHERE, after_two)
                .await
                .expect("db")
                .is_err()
        );
        assert_eq!(
            db.begin_login_attempt("alice", HOME, after_two)
                .await
                .expect("db"),
            Ok(())
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn clearing_one_source_keeps_the_block_of_another(pool: PgPool) {
        let db = Db::from_pool(pool);
        db.begin_login_attempt("alice", ELSEWHERE, |_| Some(Duration::hours(1)))
            .await
            .expect("db")
            .expect("free");
        db.begin_login_attempt("alice", HOME, |_| None)
            .await
            .expect("db")
            .expect("free");
        db.clear_login_attempts("alice", HOME).await.expect("clear");
        assert!(
            db.begin_login_attempt("alice", ELSEWHERE, |_| None)
                .await
                .expect("db")
                .is_err()
        );
    }
}
