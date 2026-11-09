//! Labels and proportions for storage usage and the administration status.

use time::{Duration, OffsetDateTime};

use protocol::{DiskSpace, ServerStatus, Storage, StorageUsage, UserStorage};

use crate::format::human_size;

const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 3600;
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;

/// Formats a byte count, e.g. `1.5 MB`.
pub fn bytes(value: u64) -> String {
    human_size(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `part` as a share of `whole`, between 0 and 1.
pub fn fraction(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64).clamp(0.0, 1.0)
    }
}

/// The sidebar line, e.g. `12.4 GB used · 1.8 TB free`.
pub fn usage_summary(storage: &Storage) -> String {
    let used = format!("{} used", bytes(storage.usage.total()));
    match storage.disk {
        Some(disk) => format!("{used} · {} free", bytes(disk.available)),
        None => used,
    }
}

/// How the disk of the storage root is split, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskBreakdown {
    /// What Home Space stores for all users.
    pub home_space: u64,
    /// Everything else on the filesystem, including what the system keeps back.
    pub other: u64,
    pub free: u64,
    pub total: u64,
}

/// Splits `disk` between Home Space's `usage` and everything else.
pub fn disk_breakdown(disk: DiskSpace, usage: StorageUsage) -> DiskBreakdown {
    let free = disk.available.min(disk.total);
    let used = disk.total - free;
    let home_space = usage.total().min(used);
    DiskBreakdown {
        home_space,
        other: used - home_space,
        free,
        total: disk.total,
    }
}

/// How long the server has run, e.g. `up 6 days`.
pub fn uptime(started_at: OffsetDateTime, now: OffsetDateTime) -> String {
    let seconds = (now - started_at).whole_seconds().max(0);
    if seconds < SECONDS_PER_MINUTE {
        "up just now".to_owned()
    } else {
        format!("up {}", duration(Duration::seconds(seconds)))
    }
}

/// A duration in its largest whole unit, e.g. `30 days`, `15 minutes`.
pub fn duration(value: Duration) -> String {
    let seconds = value.whole_seconds().max(0);
    let (count, unit) = if seconds >= SECONDS_PER_DAY {
        (seconds / SECONDS_PER_DAY, "day")
    } else if seconds >= SECONDS_PER_HOUR {
        (seconds / SECONDS_PER_HOUR, "hour")
    } else if seconds >= SECONDS_PER_MINUTE {
        (seconds / SECONDS_PER_MINUTE, "minute")
    } else {
        (seconds, "second")
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

/// The Home Space part of the disk card, e.g. `current files 1.0 GB · previous versions …`.
pub fn usage_breakdown(usage: StorageUsage) -> String {
    format!(
        "current files {} · previous versions {} · recycle bins {}",
        bytes(usage.files),
        bytes(usage.versions),
        bytes(usage.trash)
    )
}

/// The heading line of the per-user card, e.g. `91.2 GB in total for 4 users`.
pub fn users_total(status: &ServerStatus) -> String {
    let count = status.users.len();
    let users = if count == 1 { "user" } else { "users" };
    format!(
        "{} in total for {count} {users}",
        bytes(status.total_usage().total())
    )
}

/// A column of the storage-by-user table that the rows can be sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserColumn {
    Name,
    Storage,
    Files,
    Versions,
    Trash,
}

/// How the storage-by-user table is ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserSort {
    pub column: UserColumn,
    pub descending: bool,
}

impl Default for UserSort {
    /// By name, A to Z, as the server sends them.
    fn default() -> Self {
        Self {
            column: UserColumn::Name,
            descending: false,
        }
    }
}

impl UserSort {
    /// The order after clicking `column`: the same column flips direction, a new one starts
    /// with names A to Z and sizes largest first.
    pub fn toggle(self, column: UserColumn) -> Self {
        if self.column == column {
            Self {
                column,
                descending: !self.descending,
            }
        } else {
            Self {
                column,
                descending: column != UserColumn::Name,
            }
        }
    }

    /// Orders `users`; ties fall back to the username, A to Z.
    pub fn apply(self, users: &mut [UserStorage]) {
        users.sort_by(|a, b| {
            let size = |user: &UserStorage| match self.column {
                UserColumn::Name => 0,
                UserColumn::Storage => user.usage.total(),
                UserColumn::Files => user.usage.files,
                UserColumn::Versions => user.usage.versions,
                UserColumn::Trash => user.usage.trash,
            };
            let by_name = || a.username.to_lowercase().cmp(&b.username.to_lowercase());
            let order = if self.column == UserColumn::Name {
                by_name()
            } else {
                size(a).cmp(&size(b))
            };
            let order = if self.descending {
                order.reverse()
            } else {
                order
            };
            order.then_with(by_name)
        });
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn usage(files: u64, versions: u64, trash: u64) -> StorageUsage {
        StorageUsage {
            files,
            versions,
            trash,
        }
    }

    #[test]
    fn summary_names_used_and_free_space() {
        let storage = Storage {
            usage: usage(10 * GB, 2 * GB, GB / 2),
            disk: Some(DiskSpace {
                total: 4000 * GB,
                available: 2048 * GB,
            }),
        };
        assert_eq!(usage_summary(&storage), "12.5 GB used · 2.0 TB free");
    }

    #[test]
    fn summary_without_disk_names_only_usage() {
        let storage = Storage {
            usage: usage(GB, 0, 0),
            disk: None,
        };
        assert_eq!(usage_summary(&storage), "1.0 GB used");
    }

    #[test]
    fn disk_splits_into_home_space_other_and_free() {
        let disk = DiskSpace {
            total: 100,
            available: 40,
        };
        assert_eq!(
            disk_breakdown(disk, usage(20, 5, 5)),
            DiskBreakdown {
                home_space: 30,
                other: 30,
                free: 40,
                total: 100
            }
        );
    }

    #[test]
    fn home_space_never_exceeds_the_used_disk() {
        let disk = DiskSpace {
            total: 100,
            available: 90,
        };
        let split = disk_breakdown(disk, usage(50, 0, 0));
        assert_eq!((split.home_space, split.other, split.free), (10, 0, 90));
    }

    #[test]
    fn fraction_is_clamped_and_safe_for_zero() {
        assert_eq!(fraction(5, 0), 0.0);
        assert_eq!(fraction(5, 10), 0.5);
        assert_eq!(fraction(20, 10), 1.0);
    }

    #[test]
    fn durations_use_the_largest_whole_unit() {
        assert_eq!(duration(Duration::seconds(900)), "15 minutes");
        assert_eq!(duration(Duration::days(30)), "30 days");
        assert_eq!(duration(Duration::hours(1)), "1 hour");
        assert_eq!(duration(Duration::seconds(5)), "5 seconds");
    }

    #[test]
    fn uptime_reads_naturally() {
        let start = datetime!(2026-11-01 0:00 UTC);
        assert_eq!(uptime(start, start), "up just now");
        assert_eq!(uptime(start, datetime!(2026-11-07 3:00 UTC)), "up 6 days");
        assert_eq!(uptime(start, datetime!(2026-11-01 2:30 UTC)), "up 2 hours");
    }

    fn named(name: &str, files: u64, versions: u64, trash: u64) -> UserStorage {
        UserStorage {
            id: protocol::UserId(uuid::Uuid::nil()),
            username: name.to_owned(),
            is_admin: false,
            external: false,
            usage: usage(files, versions, trash),
        }
    }

    fn names(users: &[UserStorage]) -> Vec<&str> {
        users.iter().map(|user| user.username.as_str()).collect()
    }

    #[test]
    fn users_sort_by_any_column_both_ways() {
        let mut users = vec![
            named("bea", 1, 9, 0),
            named("Al", 5, 0, 2),
            named("cy", 3, 1, 1),
        ];
        UserSort::default().apply(&mut users);
        assert_eq!(names(&users), ["Al", "bea", "cy"]);

        let sort = UserSort::default().toggle(UserColumn::Storage);
        assert!(sort.descending);
        sort.apply(&mut users);
        assert_eq!(names(&users), ["bea", "Al", "cy"]);

        sort.toggle(UserColumn::Storage).apply(&mut users);
        assert_eq!(names(&users), ["cy", "Al", "bea"]);

        UserSort::default()
            .toggle(UserColumn::Trash)
            .apply(&mut users);
        assert_eq!(names(&users), ["Al", "cy", "bea"]);

        let by_name = UserSort::default().toggle(UserColumn::Name);
        assert!(by_name.descending);
        by_name.apply(&mut users);
        assert_eq!(names(&users), ["cy", "bea", "Al"]);
    }

    #[test]
    fn equal_sizes_fall_back_to_the_name() {
        let mut users = vec![named("b", 1, 0, 0), named("a", 1, 0, 0)];
        UserSort::default()
            .toggle(UserColumn::Files)
            .apply(&mut users);
        assert_eq!(names(&users), ["a", "b"]);
    }
}
