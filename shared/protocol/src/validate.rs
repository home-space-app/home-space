//! What each request must satisfy before the server acts on it.
//!
//! The server checks every JSON body and query with [`Validate`] and answers a failure with
//! one `validation-failed` problem that lists every field at fault, so a client learns all of
//! them at once. Only rules that need nothing but the request are here; rules that depend on
//! stored state stay with the server's services.

use std::collections::HashSet;
use std::hash::Hash;

use time::OffsetDateTime;

use crate::rules::{
    MAX_COPY_ITEMS, MAX_SHARE_ITEMS, MAX_SHARE_RECIPIENTS, device_name, node_name_problem,
    password_problem, username_problem,
};
use crate::{
    BrowserLoginDecision, BrowserLoginExchangeRequest, BrowserLoginStartQuery,
    ChangePasswordRequest, ChangesQuery, ContentQuery, CopyShared, CreateAccount,
    CreateFolderRequest, CreateShare, CreateTransfer, CreateUpload, DeviceSessionRequest,
    FieldError, IncomingQuery, ListQuery, LoginRequest, MoveRequest, OidcCallbackQuery,
    OidcExchangeRequest, OidcStartQuery, RefreshRequest, RenameDevice, RestoreRequest,
    SaveTransfer, SearchQuery, SetDeviceVisibility, SetPassword, ShareAccessQuery,
    ShareArchiveQuery, SharedFolderQuery, SharesQuery, TicketQuery, TicketRequest, UnlockShare,
    UpdateAccount, UpdateRequest, UpdateShare, UploadQuery, UploadsQuery,
};

/// A request that can say which of its fields are not acceptable.
pub trait Validate {
    /// Every field at fault, or `Ok` when there is none. The default has no rules.
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        Ok(())
    }
}

/// Collects the fields at fault while a request is checked.
#[derive(Default)]
struct Violations(Vec<FieldError>);

impl Violations {
    /// Records `problem` for the field at `pointer`, if there is one.
    fn check(&mut self, pointer: &str, problem: Option<impl Into<String>>) {
        if let Some(problem) = problem {
            self.0.push(FieldError::new(pointer, problem));
        }
    }

    fn finish(self) -> Result<(), Vec<FieldError>> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(self.0)
        }
    }
}

/// Why `count` items are not between 1 and `max`, as in "a share holds 1 to 1000 items".
fn count_problem(count: usize, max: usize, what: &str) -> Option<String> {
    (count == 0 || count > max).then(|| format!("{what} 1 to {max} items"))
}

fn distinct<T: Eq + Hash>(items: &[T]) -> usize {
    items.iter().collect::<HashSet<_>>().len()
}

fn future_problem(at: Option<OffsetDateTime>) -> Option<&'static str> {
    at.is_some_and(|at| at <= OffsetDateTime::now_utc())
        .then_some("the expiry must be in the future")
}

fn recipients_problem(recipients: &[impl Eq + Hash]) -> Option<String> {
    let count = distinct(recipients);
    (count == 0 || count > MAX_SHARE_RECIPIENTS)
        .then(|| format!("a share is for 1 to {MAX_SHARE_RECIPIENTS} users"))
}

impl Validate for CreateAccount {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check("/username", username_problem(&self.username));
        found.check("/password", password_problem(&self.password));
        found.finish()
    }
}

impl Validate for SetPassword {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check("/password", password_problem(&self.password));
        found.finish()
    }
}

impl Validate for ChangePasswordRequest {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check("/new_password", password_problem(&self.new_password));
        found.finish()
    }
}

impl Validate for RenameDevice {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        let blank = device_name(&self.name).is_none();
        found.check(
            "/name",
            blank.then_some("device name must be 1-100 characters"),
        );
        found.finish()
    }
}

impl Validate for CreateFolderRequest {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check("/name", node_name_problem(&self.name));
        found.finish()
    }
}

impl Validate for UpdateRequest {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check("/name", self.name.as_deref().and_then(node_name_problem));
        found.finish()
    }
}

impl Validate for UploadQuery {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check("/name", node_name_problem(&self.name));
        found.finish()
    }
}

impl Validate for CreateUpload {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        // Only a new file takes its name from here; a replacement or a transfer item has one.
        if self.node_id.is_none() && self.transfer_item_id.is_none() {
            found.check("/name", node_name_problem(&self.name));
        }
        found.finish()
    }
}

impl Validate for CreateShare {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check(
            "/node_ids",
            count_problem(distinct(&self.node_ids), MAX_SHARE_ITEMS, "a share holds"),
        );
        found.check("/expires_at", future_problem(self.expires_at));
        if !self.recipients.is_empty() {
            found.check("/recipients", recipients_problem(&self.recipients));
            found.check(
                "/password",
                self.password
                    .is_some()
                    .then_some("a share with users has no password"),
            );
        }
        found.finish()
    }
}

impl Validate for UpdateShare {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check(
            "/expires_at",
            future_problem(self.expires_at.value().copied()),
        );
        if let Some(recipients) = &self.recipients {
            found.check("/recipients", recipients_problem(recipients));
        }
        found.finish()
    }
}

impl Validate for CopyShared {
    fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut found = Violations::default();
        found.check(
            "/node_ids",
            count_problem(self.node_ids.len(), MAX_COPY_ITEMS, "a copy holds"),
        );
        found.finish()
    }
}

/// Requests whose shape is all there is to check here; the server's services check what
/// depends on stored state.
macro_rules! shape_only {
    ($($request:ty),* $(,)?) => {
        $(impl Validate for $request {})*
    };
}

shape_only!(
    BrowserLoginDecision,
    BrowserLoginExchangeRequest,
    BrowserLoginStartQuery,
    ChangesQuery,
    ContentQuery,
    CreateTransfer,
    DeviceSessionRequest,
    IncomingQuery,
    ListQuery,
    LoginRequest,
    MoveRequest,
    OidcCallbackQuery,
    OidcExchangeRequest,
    OidcStartQuery,
    RefreshRequest,
    RestoreRequest,
    SaveTransfer,
    SearchQuery,
    SetDeviceVisibility,
    ShareAccessQuery,
    ShareArchiveQuery,
    SharedFolderQuery,
    SharesQuery,
    TicketQuery,
    TicketRequest,
    UnlockShare,
    UpdateAccount,
    UploadsQuery,
);

#[cfg(test)]
mod tests {
    use time::Duration;
    use uuid::Uuid;

    use super::*;
    use crate::{NodeId, UserId};

    fn share(node_ids: usize, recipients: usize) -> CreateShare {
        CreateShare {
            node_ids: (1..=node_ids as u128)
                .map(|n| NodeId(Uuid::from_u128(n)))
                .collect(),
            password: None,
            expires_at: None,
            recipients: (1..=recipients as u128)
                .map(|n| UserId(Uuid::from_u128(n)))
                .collect(),
        }
    }

    fn pointers(result: Result<(), Vec<FieldError>>) -> Vec<String> {
        result
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|error| error.pointer)
            .collect()
    }

    #[test]
    fn every_field_at_fault_is_reported_at_once() {
        let mut request = share(0, 1);
        request.password = Some("secret".to_owned());
        request.expires_at = Some(OffsetDateTime::now_utc() - Duration::hours(1));
        assert_eq!(
            pointers(request.validate()),
            ["/node_ids", "/expires_at", "/password"]
        );
    }

    #[test]
    fn a_link_may_have_a_password_and_repeated_items_count_once() {
        let mut request = share(1, 0);
        request.node_ids.push(request.node_ids[0]);
        request.password = Some("secret".to_owned());
        assert_eq!(request.validate(), Ok(()));
    }

    #[test]
    fn a_share_is_for_at_most_a_hundred_users() {
        assert_eq!(
            pointers(share(1, MAX_SHARE_RECIPIENTS + 1).validate()),
            ["/recipients"]
        );
    }

    #[test]
    fn a_new_account_needs_a_username_and_a_long_enough_password() {
        let request = CreateAccount {
            username: String::new(),
            password: "short".to_owned(),
            is_admin: false,
            quota_bytes: None,
        };
        assert_eq!(pointers(request.validate()), ["/username", "/password"]);
    }
}
