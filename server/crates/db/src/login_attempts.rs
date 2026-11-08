use time::{Duration, OffsetDateTime};

use crate::{Db, DbError};

impl Db {
    /// Counts a sign-in attempt for `username`, as a failure until
    /// [`clear_login_attempts`](Self::clear_login_attempts) takes it back, unless attempts for it
    /// are blocked. `block` maps the number of failures, this attempt included, to how long the
    /// next attempt must wait, if at all.
    ///
    /// Returns `Err` with the time the block ends when the attempt must not be checked. Counting
    /// before the password is checked keeps a burst of parallel guesses from all getting through
    /// before the first one is recorded.
    pub async fn begin_login_attempt(
        &self,
        username: &str,
        block: impl FnOnce(u32) -> Option<Duration>,
    ) -> Result<Result<(), OffsetDateTime>, DbError> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("INSERT INTO login_attempts (username) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(username)
            .execute(&mut *tx)
            .await?;
        let (failures, blocked_until) = sqlx::query_as::<_, (i32, Option<OffsetDateTime>)>(
            "SELECT failures, blocked_until FROM login_attempts WHERE username = $1 FOR UPDATE",
        )
        .bind(username)
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
            "UPDATE login_attempts SET failures = $2, blocked_until = $3, last_failed_at = $4 \
             WHERE username = $1",
        )
        .bind(username)
        .bind(failures)
        .bind(blocked_until)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok(()))
    }

    /// Forgets the failed attempts of `username`, after a correct password.
    pub async fn clear_login_attempts(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM login_attempts WHERE username = $1")
            .bind(username)
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

    fn after_two(failures: u32) -> Option<Duration> {
        (failures >= 2).then_some(Duration::minutes(1))
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn attempts_are_blocked_once_the_policy_says_so(pool: PgPool) {
        let db = Db::from_pool(pool);
        assert_eq!(
            db.begin_login_attempt("alice", after_two)
                .await
                .expect("db"),
            Ok(())
        );
        assert_eq!(
            db.begin_login_attempt("alice", after_two)
                .await
                .expect("db"),
            Ok(())
        );
        let blocked = db
            .begin_login_attempt("alice", after_two)
            .await
            .expect("db");
        assert!(blocked.is_err());
        assert_eq!(
            db.begin_login_attempt("bob", after_two).await.expect("db"),
            Ok(())
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn clearing_starts_the_count_again(pool: PgPool) {
        let db = Db::from_pool(pool);
        db.begin_login_attempt("alice", |_| None)
            .await
            .expect("db")
            .expect("free");
        db.clear_login_attempts("alice").await.expect("clear");
        let failures = std::cell::Cell::new(0);
        db.begin_login_attempt("alice", |count| {
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
        db.begin_login_attempt("alice", |_| None)
            .await
            .expect("db")
            .expect("free");
        db.begin_login_attempt("bob", |_| Some(Duration::hours(1)))
            .await
            .expect("db")
            .expect("free");
        let later = OffsetDateTime::now_utc() + Duration::seconds(1);
        assert_eq!(db.prune_login_attempts(later).await.expect("prune"), 1);
        assert!(
            db.begin_login_attempt("bob", |_| None)
                .await
                .expect("db")
                .is_err()
        );
    }
}
