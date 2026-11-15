//! The share dialog: a read-only link to some files and folders, its password and expiry.
//!
//! The dialog's rules live in `client_core::share`; [`ShareForm`] holds what the user has
//! chosen so far and the link it edits, and the apps draw [`ShareFormState`].

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use client_core::share::{
    Expiry, ShareDraft, matching_share, share_subtitle, share_title, stop_sharing_question,
};
use protocol::{Node, NodeKind, Share, UpdateShare};
use time::{Duration, OffsetDateTime};

use crate::{CoreClient, CoreError, Entry, EntryKind, off_ui_thread, parse_id};

/// How far ahead a picked date starts when nothing else suggests one.
const PICKED_DATE_AHEAD: Duration = Duration::days(7);
/// The `code` of a [`CoreError::Failed`] for a choice the dialog cannot save.
const INVALID_CODE: &str = "invalid";

/// The expiry chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ShareExpiry {
    Day,
    Week,
    Month,
    Never,
    /// A date the user picked; see [`ShareFormState::picked_date`].
    Date,
}

/// How a save ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ShareSaved {
    /// A new link: the dialog stays open to show it.
    Created,
    /// The link changed, or there was nothing to change; the dialog closes.
    Updated,
}

/// What the share dialog shows.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ShareFormState {
    /// `Share “report.pdf”` or `Share 3 items`.
    pub title: String,
    /// `Invoices and Photos (folders) · report.pdf (file)`.
    pub subtitle: String,
    /// Asked before the link stops working.
    pub stop_question: String,
    /// The link to copy, once it is saved.
    pub url: Option<String>,
    pub require_password: bool,
    /// Whether the password field takes a new password, rather than standing for the saved one.
    pub editing_password: bool,
    /// Whether to offer changing the saved password.
    pub offers_change_password: bool,
    pub expiry: ShareExpiry,
    /// When the link stops working if saved now; `None` for never.
    pub expires_at: Option<SystemTime>,
}

struct Inner {
    share: Option<Share>,
    draft: ShareDraft,
}

/// The share dialog's choices for some files and folders, and the link that shares exactly them.
#[derive(uniffi::Object)]
pub struct ShareForm {
    client: api_client::Client,
    nodes: Vec<Node>,
    inner: Mutex<Inner>,
}

impl ShareForm {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn show(inner: &mut Inner, share: Share) {
        inner.draft = ShareDraft::of(&share);
        inner.share = Some(share);
    }
}

#[uniffi::export]
impl ShareForm {
    pub fn state(&self) -> ShareFormState {
        let inner = self.lock();
        let draft = &inner.draft;
        ShareFormState {
            title: share_title(&self.nodes),
            subtitle: share_subtitle(&self.nodes),
            stop_question: stop_sharing_question(&self.nodes),
            url: inner
                .share
                .as_ref()
                .map(|share| self.client.share_url(&share.token)),
            require_password: draft.require_password,
            editing_password: draft.editing_password(),
            offers_change_password: draft.offers_change_password(),
            expiry: chip(draft.expiry),
            expires_at: draft
                .expires_at(OffsetDateTime::now_utc())
                .map(SystemTime::from),
        }
    }

    pub fn set_require_password(&self, require: bool) {
        self.lock().draft.set_require_password(require);
    }

    pub fn set_password(&self, password: String) {
        self.lock().draft.password = password;
    }

    /// Lets the user type a new password in place of the saved one.
    pub fn change_password(&self) {
        self.lock().draft.change_password();
    }

    /// Picks one of the preset chips; [`ShareExpiry::Date`] keeps a date already picked or
    /// starts one from the current expiry.
    pub fn set_expiry(&self, expiry: ShareExpiry) {
        let mut inner = self.lock();
        let now = OffsetDateTime::now_utc();
        inner.draft.expiry = match expiry {
            ShareExpiry::Day => Expiry::Day,
            ShareExpiry::Week => Expiry::Week,
            ShareExpiry::Month => Expiry::Month,
            ShareExpiry::Never => Expiry::Never,
            ShareExpiry::Date => match inner.draft.expiry {
                picked @ Expiry::On(_) => picked,
                _ => Expiry::On(
                    inner
                        .draft
                        .expires_at(now)
                        .unwrap_or(now + PICKED_DATE_AHEAD),
                ),
            },
        };
    }

    /// Makes the link stop working at `at`.
    pub fn pick_date(&self, at: SystemTime) {
        self.lock().draft.expiry = Expiry::On(OffsetDateTime::from(at));
    }

    /// Saves the link: makes it, or changes the saved one. A choice that cannot be saved, such
    /// as a short password, fails with code `invalid` and a message to show.
    pub async fn save(&self) -> Result<ShareSaved, CoreError> {
        let now = OffsetDateTime::now_utc();
        let (saved, draft) = {
            let inner = self.lock();
            (
                inner.share.as_ref().map(|share| share.id),
                inner.draft.clone(),
            )
        };
        let client = self.client.clone();
        match saved {
            Some(id) => {
                let request = draft.update_request(now).map_err(invalid)?;
                if request == UpdateShare::default() {
                    return Ok(ShareSaved::Updated);
                }
                let share =
                    off_ui_thread(async move { client.update_share(id, &request).await }).await?;
                Self::show(&mut self.lock(), share);
                Ok(ShareSaved::Updated)
            }
            None => {
                let ids = self.nodes.iter().map(|node| node.id).collect();
                let request = draft.create_request(ids, now).map_err(invalid)?;
                let share =
                    off_ui_thread(async move { client.create_share(&request).await }).await?;
                Self::show(&mut self.lock(), share);
                Ok(ShareSaved::Created)
            }
        }
    }

    /// Stops sharing: the link stops working at once.
    pub async fn stop(&self) -> Result<(), CoreError> {
        let Some(id) = self.lock().share.as_ref().map(|share| share.id) else {
            return Ok(());
        };
        let client = self.client.clone();
        off_ui_thread(async move { client.delete_share(id).await }).await?;
        self.lock().share = None;
        Ok(())
    }
}

#[uniffi::export]
impl CoreClient {
    /// The share dialog for `entries`, editing the link that shares exactly them if there is one.
    pub async fn share_form(&self, entries: Vec<Entry>) -> Result<Arc<ShareForm>, CoreError> {
        let nodes = entries.iter().map(node_of).collect::<Result<Vec<_>, _>>()?;
        let client = self.inner.clone();
        let first = nodes.first().map(|node| node.id);
        let shares = off_ui_thread(async move { client.shares(first).await }).await?;
        let ids: Vec<_> = nodes.iter().map(|node| node.id).collect();
        let mut inner = Inner {
            share: None,
            draft: ShareDraft::new(),
        };
        if let Some(found) = matching_share(&shares, &ids).cloned() {
            ShareForm::show(&mut inner, found);
        }
        Ok(Arc::new(ShareForm {
            client: self.inner.clone(),
            nodes,
            inner: Mutex::new(inner),
        }))
    }
}

fn chip(expiry: Expiry) -> ShareExpiry {
    match expiry {
        Expiry::Day => ShareExpiry::Day,
        Expiry::Week => ShareExpiry::Week,
        Expiry::Month => ShareExpiry::Month,
        Expiry::Never => ShareExpiry::Never,
        Expiry::On(_) => ShareExpiry::Date,
    }
}

fn invalid(problem: client_core::share::DraftError) -> CoreError {
    CoreError::Failed {
        code: INVALID_CODE.to_owned(),
        detail: problem.message().to_owned(),
    }
}

/// What the dialog's words need of an entry.
fn node_of(entry: &Entry) -> Result<Node, CoreError> {
    let updated_at = OffsetDateTime::from(entry.updated_at);
    Ok(Node {
        id: parse_id(&entry.id)?,
        parent_id: entry.parent_id.as_deref().map(parse_id).transpose()?,
        kind: match entry.kind {
            EntryKind::Folder => NodeKind::Folder,
            EntryKind::File => NodeKind::File,
        },
        name: entry.name.clone(),
        size: entry.size,
        sha256: entry.sha256.clone(),
        mime: entry.mime.clone(),
        version: entry.version,
        created_at: updated_at,
        updated_at,
        edited_on: None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::tests::node;

    const SHARE_ID: &str = "00000000-0000-0000-0000-0000000000a1";
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    async fn signed_in(server: &MockServer) -> CoreClient {
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "access_token": "a", "refresh_token": "r", "expires_in": 900 }),
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "username": "alice",
                "lost_and_found_id": "00000000-0000-0000-0000-0000000000f0",
            })))
            .mount(server)
            .await;
        let client = CoreClient::new(server.uri());
        client
            .login("alice".to_owned(), "pw".to_owned())
            .await
            .expect("login");
        client
    }

    fn entry(name: &str) -> Entry {
        serde_json::from_value::<Node>(node(name, "file", 3))
            .expect("node")
            .into()
    }

    async fn shares_answer(server: &MockServer, shares: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/api/v1/shares"))
            .respond_with(ResponseTemplate::new(200).set_body_json(shares))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_new_link_is_made_and_shown() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        shares_answer(&server, json!([])).await;
        let file = entry("a.txt");
        Mock::given(method("POST"))
            .and(path("/api/v1/shares"))
            .and(body_partial_json(json!({ "node_ids": [file.id] })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": SHARE_ID,
                "token": TOKEN,
                "items": [node("a.txt", "file", 3)],
                "has_password": false,
                "permission": "read",
                "expires_at": null,
                "created_at": "2026-11-15T10:00:00Z",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let form = client.share_form(vec![file]).await.expect("form");
        assert_eq!(form.state().url, None);
        assert_eq!(form.state().expiry, ShareExpiry::Week);
        assert_eq!(form.save().await.expect("save"), ShareSaved::Created);

        assert_eq!(
            form.state().url,
            Some(format!("{}/s/{TOKEN}", server.uri()))
        );
    }

    #[tokio::test]
    async fn a_short_password_is_refused_before_asking_the_server() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        shares_answer(&server, json!([])).await;
        let form = client.share_form(vec![entry("a.txt")]).await.expect("form");
        form.set_require_password(true);
        form.set_password("short".to_owned());

        let err = form.save().await.expect_err("refused");

        assert!(matches!(err, CoreError::Failed { code, .. } if code == INVALID_CODE));
    }

    #[tokio::test]
    async fn the_link_of_the_entries_is_edited() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        let file = entry("a.txt");
        Mock::given(method("GET"))
            .and(path("/api/v1/shares"))
            .and(query_param("node_id", file.id.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": SHARE_ID,
                "token": TOKEN,
                "items": [node("a.txt", "file", 3)],
                "has_password": true,
                "permission": "read",
                "expires_at": null,
                "created_at": "2026-11-15T10:00:00Z",
            }])))
            .mount(&server)
            .await;

        let state = client.share_form(vec![file]).await.expect("form").state();

        assert!(state.url.is_some());
        assert!(state.offers_change_password);
        assert_eq!(state.expiry, ShareExpiry::Never);
    }
}
