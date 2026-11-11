//! Labels, filters and form checks for account management by administrators.

use time::OffsetDateTime;

use protocol::Account;

use crate::storage::{bytes, fraction};

const GIB: u64 = 1024 * 1024 * 1024;
const TIB: u64 = 1024 * GIB;
/// From this share of the quota on, the usage bar warns.
pub const QUOTA_WARNING: f64 = 0.9;

/// The unit a quota is typed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuotaUnit {
    #[default]
    Gb,
    Tb,
}

impl QuotaUnit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gb => "GB",
            Self::Tb => "TB",
        }
    }

    fn bytes(self) -> u64 {
        match self {
            Self::Gb => GIB,
            Self::Tb => TIB,
        }
    }
}

/// Why a typed quota was refused when it is no number.
pub const NOT_A_QUOTA: &str = "Enter the quota as a number, or leave it empty for no quota.";

/// The quota typed as `text` in `unit`: `None` for an empty field, which means no quota.
pub fn parse_quota(text: &str, unit: QuotaUnit) -> Result<Option<u64>, &'static str> {
    let text = text.trim().replace(',', ".");
    if text.is_empty() {
        return Ok(None);
    }
    let value: f64 = text.parse().map_err(|_| NOT_A_QUOTA)?;
    if !value.is_finite() || value <= 0.0 {
        return Err("The quota must be more than zero.");
    }
    let bytes = value * unit.bytes() as f64;
    if bytes >= u64::MAX as f64 {
        return Err("The quota is too large.");
    }
    Ok(Some(bytes.round() as u64))
}

/// How a stored quota is shown in the form: whole terabytes in TB, anything else in GB.
pub fn quota_input(quota: Option<u64>) -> (String, QuotaUnit) {
    match quota {
        None => (String::new(), QuotaUnit::Gb),
        Some(bytes) if bytes >= TIB && bytes % TIB == 0 => {
            ((bytes / TIB).to_string(), QuotaUnit::Tb)
        }
        Some(bytes) => {
            let value = bytes as f64 / GIB as f64;
            let text = format!("{value:.2}");
            let text = text.trim_end_matches('0').trim_end_matches('.');
            (text.to_owned(), QuotaUnit::Gb)
        }
    }
}

/// The storage column, e.g. `31.0 GB of 50.0 GB` or `12.4 GB · no quota`.
pub fn storage_label(account: &Account) -> String {
    let used = bytes(account.usage.total());
    match account.quota_bytes {
        Some(quota) => format!("{used} of {}", bytes(quota)),
        None => format!("{used} · no quota"),
    }
}

/// How full the usage bar is: against the quota, or against `largest` user when there is none.
pub fn storage_fill(account: &Account, largest: u64) -> f64 {
    let used = account.usage.total();
    match account.quota_bytes {
        Some(quota) => fraction(used, quota),
        None => fraction(used, largest),
    }
}

/// Whether the user is at or near their quota.
pub fn near_quota(account: &Account) -> bool {
    account
        .quota_bytes
        .is_some_and(|quota| fraction(account.usage.total(), quota) >= QUOTA_WARNING)
}

/// How the user signs in.
pub fn sign_in_method(account: &Account, provider: Option<&str>) -> String {
    if account.external {
        provider.unwrap_or("Single sign-on").to_owned()
    } else {
        "Password".to_owned()
    }
}

/// Which accounts the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccountFilter {
    #[default]
    All,
    Admins,
    /// Disabled or marked for deletion: everyone who cannot sign in.
    Disabled,
}

impl AccountFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Admins => "Admins",
            Self::Disabled => "Disabled",
        }
    }

    pub fn matches(self, account: &Account) -> bool {
        match self {
            Self::All => true,
            Self::Admins => account.is_admin,
            Self::Disabled => !account.active(),
        }
    }
}

/// Whether `account`'s username contains `query`, ignoring case; an empty query matches all.
pub fn matches_search(account: &Account, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty() || account.username.to_lowercase().contains(&query)
}

/// What is wrong with a new password and its repetition, if anything. The server checks the
/// rules for passwords themselves.
pub fn password_problem(password: &str, repeat: &str) -> Option<&'static str> {
    if password.is_empty() {
        Some("Enter a password.")
    } else if password != repeat {
        Some("The passwords do not match.")
    } else {
        None
    }
}

/// How long until a marked account is deleted, in whole minutes left, e.g. `in 9 minutes`.
pub fn deletion_countdown(delete_at: OffsetDateTime, now: OffsetDateTime) -> String {
    let seconds = (delete_at - now).whole_seconds();
    let minutes = seconds / 60;
    match (seconds, minutes) {
        (..=0, _) => "any moment now".to_owned(),
        (_, 0) => "in less than a minute".to_owned(),
        (_, 1) => "in 1 minute".to_owned(),
        (_, 2..90) => format!("in {minutes} minutes"),
        _ => format!("in {} hours", (minutes + 30) / 60),
    }
}

#[cfg(test)]
mod tests {
    use protocol::{StorageUsage, UserId};
    use time::Duration;

    use super::*;

    fn account(used: u64, quota: Option<u64>) -> Account {
        Account {
            id: UserId(uuid::Uuid::nil()),
            username: "Peter".to_owned(),
            is_admin: false,
            external: false,
            created_at: OffsetDateTime::UNIX_EPOCH,
            disabled: false,
            quota_bytes: quota,
            delete_at: None,
            usage: StorageUsage {
                files: used,
                versions: 0,
                trash: 0,
            },
            devices: 0,
            last_seen_at: None,
        }
    }

    #[test]
    fn quotas_are_typed_in_gb_or_tb() {
        assert_eq!(parse_quota("", QuotaUnit::Gb), Ok(None));
        assert_eq!(parse_quota(" 50 ", QuotaUnit::Gb), Ok(Some(50 * GIB)));
        assert_eq!(parse_quota("1,5", QuotaUnit::Tb), Ok(Some(TIB + TIB / 2)));
        assert!(parse_quota("0", QuotaUnit::Gb).is_err());
        assert!(parse_quota("-1", QuotaUnit::Gb).is_err());
        assert!(parse_quota("lots", QuotaUnit::Gb).is_err());
    }

    #[test]
    fn a_stored_quota_reads_back_as_typed() {
        assert_eq!(quota_input(None), (String::new(), QuotaUnit::Gb));
        assert_eq!(
            quota_input(Some(50 * GIB)),
            ("50".to_owned(), QuotaUnit::Gb)
        );
        assert_eq!(quota_input(Some(2 * TIB)), ("2".to_owned(), QuotaUnit::Tb));
        assert_eq!(
            quota_input(Some(GIB / 2)),
            ("0.5".to_owned(), QuotaUnit::Gb)
        );
    }

    #[test]
    fn storage_reads_against_the_quota() {
        assert_eq!(
            storage_label(&account(GIB, Some(5 * GIB))),
            "1.0 GB of 5.0 GB"
        );
        assert_eq!(storage_label(&account(GIB, None)), "1.0 GB · no quota");
        assert!(near_quota(&account(19 * GIB, Some(20 * GIB))));
        assert!(!near_quota(&account(GIB, Some(20 * GIB))));
        assert!(!near_quota(&account(GIB, None)));
    }

    #[test]
    fn the_disabled_filter_includes_accounts_marked_for_deletion() {
        let mut marked = account(0, None);
        marked.delete_at = Some(OffsetDateTime::UNIX_EPOCH);
        marked.disabled = true;
        assert!(AccountFilter::Disabled.matches(&marked));
        assert!(!AccountFilter::Disabled.matches(&account(0, None)));
        assert!(matches_search(&marked, "pet"));
        assert!(!matches_search(&marked, "anna"));
    }

    #[test]
    fn passwords_must_be_repeated() {
        assert_eq!(password_problem("", ""), Some("Enter a password."));
        assert_eq!(
            password_problem("a-password", "a-passw0rd"),
            Some("The passwords do not match.")
        );
        assert_eq!(password_problem("a-password", "a-password"), None);
    }

    #[test]
    fn the_countdown_rounds_up_to_whole_minutes() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            deletion_countdown(now + Duration::seconds(570), now),
            "in 9 minutes"
        );
        assert_eq!(
            deletion_countdown(now + Duration::seconds(30), now),
            "in less than a minute"
        );
        assert_eq!(
            deletion_countdown(now + Duration::seconds(90), now),
            "in 1 minute"
        );
        assert_eq!(
            deletion_countdown(now - Duration::seconds(5), now),
            "any moment now"
        );
        assert_eq!(
            deletion_countdown(now + Duration::hours(3), now),
            "in 3 hours"
        );
    }
}
