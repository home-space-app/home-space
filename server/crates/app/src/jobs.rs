//! Background jobs of a running server. Each runs once at start and then on its interval, and
//! logs its failures instead of stopping.

use std::{fmt::Display, time::Duration};

use time::OffsetDateTime;
use tokio::time::{MissedTickBehavior, interval};

use api::AppState;
use db::Db;
use files::FileServices;
use search::SearchIndex;

use crate::config::Config;

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);

/// Starts every background job of the server; the clean-ups of the database's own records
/// work on `db` directly.
pub fn spawn(db: &Db, state: &AppState, config: &Config) {
    tokio::spawn(catch_up_search(state.search.clone()));
    tokio::spawn(prune(
        db.clone(),
        config.changes_retention_days.map(time::Duration::days),
        time::Duration::seconds(config.refresh_ttl_secs),
    ));
    tokio::spawn(expire_uploads(
        state.files.clone(),
        Duration::from_secs(config.upload_expiry_hours.saturating_mul(HOUR.as_secs())),
    ));
    tokio::spawn(expire_transfers(state.files.clone()));
    tokio::spawn(delete_users(state.files.clone()));
}

/// Indexes what changed while the server was not running.
async fn catch_up_search(search: SearchIndex) {
    match search.catch_up().await {
        Ok(report) => tracing::info!(
            names_indexed = report.names_indexed,
            content_queued = report.content_queued,
            "search index is up to date"
        ),
        Err(err) => tracing::error!(error = %err, "search index catch-up failed"),
    }
}

/// Runs [`prune_once`] daily.
async fn prune(db: Db, retention: Option<time::Duration>, device_retention: time::Duration) {
    let mut daily = interval(DAY);
    loop {
        daily.tick().await;
        prune_once(&db, retention, device_retention).await;
    }
}

/// Compacts the change journal, drops its entries older than `retention` when there is one,
/// removes expired sign-in state and old failed sign-ins, and forgets devices signed out for
/// longer than `device_retention`.
async fn prune_once(db: &Db, retention: Option<time::Duration>, device_retention: time::Duration) {
    let now = OffsetDateTime::now_utc();
    report(
        (
            "change journal compacted",
            "compacting the change journal failed",
        ),
        db.compact_changes().await,
    );
    if let Some(retention) = retention {
        report(
            ("change journal pruned", "pruning the change journal failed"),
            db.prune_changes(now - retention).await,
        );
    }
    report(
        (
            "removed expired single sign-on state",
            "pruning single sign-on state failed",
        ),
        db.prune_oidc().await,
    );
    report(
        (
            "removed expired sessions",
            "pruning expired sessions failed",
        ),
        db.prune_sessions().await,
    );
    report(
        (
            "removed old failed sign-in attempts",
            "pruning failed sign-in attempts failed",
        ),
        db.prune_login_attempts(now - auth::LOGIN_ATTEMPTS_MEMORY)
            .await,
    );
    report(
        (
            "forgot signed-out devices",
            "pruning signed-out devices failed",
        ),
        db.prune_devices(now - device_retention).await,
    );
}

/// Ends resumable uploads idle for longer than `expiry`, hourly.
async fn expire_uploads(files: FileServices, expiry: Duration) {
    let mut hourly = interval(HOUR);
    loop {
        hourly.tick().await;
        report(
            (
                "expired unfinished uploads",
                "expiring unfinished uploads failed",
            ),
            files.uploads.expire_uploads(expiry).await,
        );
    }
}

/// Removes transfers to devices whose time ran out, hourly; the file service tells both sides.
async fn expire_transfers(files: FileServices) {
    let mut hourly = interval(HOUR);
    loop {
        hourly.tick().await;
        report(
            ("expired transfers", "expiring transfers failed"),
            files
                .transfers
                .expire_transfers()
                .await
                .map(|expired| expired.len()),
        );
    }
}

/// Deletes users whose deletion an administrator scheduled and did not take back, with all
/// their files; every minute, so a deletion happens close to the time the administration
/// shows for it.
async fn delete_users(files: FileServices) {
    let mut every_minute = interval(MINUTE);
    every_minute.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        every_minute.tick().await;
        match files.delete_due_users().await {
            Ok(deleted) => {
                for user in deleted {
                    tracing::info!(%user, "deleted user and their files");
                }
            }
            Err(err) => tracing::error!(error = %err, "deleting users failed"),
        }
    }
}

/// Logs how many items a clean-up step removed (nothing when it removed none) with the first
/// message, or why it failed with the second.
fn report<N, E>((done, failed): (&str, &str), outcome: Result<N, E>)
where
    N: TryInto<u64>,
    E: Display,
{
    match outcome.map(|count| count.try_into().unwrap_or(u64::MAX)) {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "{done}"),
        Err(err) => tracing::error!(error = %err, "{failed}"),
    }
}
