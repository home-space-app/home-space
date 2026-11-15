//! The share dialog: some files and folders shared read-only with people on the server, or
//! through a link with its password and expiry.
//!
//! The dialog's rules live in `client_core::share`; [`ShareForm`] holds what the user has
//! chosen so far in each tab and the shares they edit, and the apps draw [`ShareFormState`].

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use client_core::share::{
    Audience, Expiry, ShareDraft, matching_share, share_subtitle, share_title, stop_sharing_note,
    stop_sharing_question, you,
};
use protocol::{Node, NodeKind, Share, UpdateShare, UserId, UserRef};
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

/// Who a share is for: the tabs of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ShareAudience {
    People,
    Link,
}

impl From<Audience> for ShareAudience {
    fn from(audience: Audience) -> Self {
        match audience {
            Audience::People => Self::People,
            Audience::Link => Self::Link,
        }
    }
}

impl From<ShareAudience> for Audience {
    fn from(audience: ShareAudience) -> Self {
        match audience {
            ShareAudience::People => Self::People,
            ShareAudience::Link => Self::Link,
        }
    }
}

/// Another user, among those a share is for or offered to add.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharePerson {
    pub id: String,
    pub username: String,
    /// When they were added to the saved share; `None` until it is saved with them.
    pub added_at: Option<SystemTime>,
}

/// How a save ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ShareSaved {
    /// A new link: the dialog stays open to show it.
    Created,
    /// A share changed or was made with people, or there was nothing to change; the dialog
    /// closes.
    Updated,
}

/// What the share dialog shows.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ShareFormState {
    /// `Share “report.pdf”` or `Share 3 items`.
    pub title: String,
    /// `Invoices and Photos (folders) · report.pdf (file)`.
    pub subtitle: String,
    /// The open tab.
    pub audience: ShareAudience,
    /// `sam (you)`, the owner's line among the people with access.
    pub owner: String,
    /// Who the share with people is for.
    pub people: Vec<SharePerson>,
    /// Whether the open tab edits a saved share, which can stop.
    pub has_share: bool,
    /// Asked before the share of the open tab stops working.
    pub stop_question: String,
    /// Said under [`Self::stop_question`].
    pub stop_note: String,
    /// The link to copy, once it is saved.
    pub url: Option<String>,
    pub require_password: bool,
    /// Whether the password field takes a new password, rather than standing for the saved one.
    pub editing_password: bool,
    /// Whether to offer changing the saved password.
    pub offers_change_password: bool,
    /// The expiry chip of the open tab.
    pub expiry: ShareExpiry,
    /// When the share of the open tab stops working if saved now; `None` for never.
    pub expires_at: Option<SystemTime>,
}

/// One tab: what is drafted there, and the share it edits once there is one.
struct Tab {
    share: Option<Share>,
    draft: ShareDraft,
}

impl Tab {
    fn new(audience: Audience) -> Self {
        Self {
            share: None,
            draft: ShareDraft::for_audience(audience),
        }
    }

    fn show(&mut self, share: Share) {
        self.draft = ShareDraft::of(&share);
        self.share = Some(share);
    }
}

struct Inner {
    audience: Audience,
    people: Tab,
    link: Tab,
    /// The other users of the server.
    users: Vec<UserRef>,
}

impl Inner {
    fn tab(&self) -> &Tab {
        match self.audience {
            Audience::People => &self.people,
            Audience::Link => &self.link,
        }
    }

    fn tab_mut(&mut self) -> &mut Tab {
        match self.audience {
            Audience::People => &mut self.people,
            Audience::Link => &mut self.link,
        }
    }
}

/// The share dialog's choices for some files and folders, and the shares of exactly them.
#[derive(uniffi::Object)]
pub struct ShareForm {
    client: api_client::Client,
    nodes: Vec<Node>,
    owner: String,
    inner: Mutex<Inner>,
}

impl ShareForm {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn person(user: &UserRef, added_at: Option<SystemTime>) -> SharePerson {
    SharePerson {
        id: user.id.to_string(),
        username: user.username.clone(),
        added_at,
    }
}

#[uniffi::export]
impl ShareForm {
    pub fn state(&self) -> ShareFormState {
        let inner = self.lock();
        let link = &inner.link.draft;
        let draft = &inner.tab().draft;
        let saved = inner.people.share.as_ref();
        let people = inner
            .people
            .draft
            .recipients()
            .iter()
            .map(|user| {
                let added_at = saved.and_then(|share| {
                    share
                        .recipients
                        .iter()
                        .find(|recipient| recipient.user.id == user.id)
                        .map(|recipient| SystemTime::from(recipient.added_at))
                });
                person(user, added_at)
            })
            .collect();
        ShareFormState {
            title: share_title(&self.nodes),
            subtitle: share_subtitle(&self.nodes),
            audience: inner.audience.into(),
            owner: you(&self.owner),
            people,
            has_share: inner.tab().share.is_some(),
            stop_question: stop_sharing_question(&self.nodes),
            stop_note: stop_sharing_note(inner.audience).to_owned(),
            url: inner
                .link
                .share
                .as_ref()
                .and_then(|share| share.token.as_deref())
                .map(|token| self.client.share_url(token)),
            require_password: link.require_password,
            editing_password: link.editing_password(),
            offers_change_password: link.offers_change_password(),
            expiry: chip(draft.expiry),
            expires_at: draft
                .expires_at(OffsetDateTime::now_utc())
                .map(SystemTime::from),
        }
    }

    /// Opens the tab for `audience`.
    pub fn set_audience(&self, audience: ShareAudience) {
        self.lock().audience = audience.into();
    }

    /// The users not added yet whose name contains `query`, ignoring case.
    pub fn candidates(&self, query: String) -> Vec<SharePerson> {
        let inner = self.lock();
        inner
            .people
            .draft
            .candidates(&inner.users, &query)
            .into_iter()
            .map(|user| person(user, None))
            .collect()
    }

    /// Adds the user with `id` to the people the share is for.
    pub fn add_person(&self, id: String) {
        let mut inner = self.lock();
        if let Some(user) = inner
            .users
            .iter()
            .find(|user| user.id.to_string() == id)
            .cloned()
        {
            inner.people.draft.add_recipient(user);
        }
    }

    pub fn remove_person(&self, id: String) -> Result<(), CoreError> {
        let id: UserId = id.parse().map_err(|_| CoreError::InvalidId { id })?;
        self.lock().people.draft.remove_recipient(id);
        Ok(())
    }

    pub fn set_require_password(&self, require: bool) {
        self.lock().link.draft.set_require_password(require);
    }

    pub fn set_password(&self, password: String) {
        self.lock().link.draft.password = password;
    }

    /// Lets the user type a new password in place of the saved one.
    pub fn change_password(&self) {
        self.lock().link.draft.change_password();
    }

    /// Picks one of the preset chips; [`ShareExpiry::Date`] keeps a date already picked or
    /// starts one from the current expiry.
    pub fn set_expiry(&self, expiry: ShareExpiry) {
        let mut inner = self.lock();
        let now = OffsetDateTime::now_utc();
        let draft = &mut inner.tab_mut().draft;
        draft.expiry = match expiry {
            ShareExpiry::Day => Expiry::Day,
            ShareExpiry::Week => Expiry::Week,
            ShareExpiry::Month => Expiry::Month,
            ShareExpiry::Never => Expiry::Never,
            ShareExpiry::Date => match draft.expiry {
                picked @ Expiry::On(_) => picked,
                _ => Expiry::On(draft.expires_at(now).unwrap_or(now + PICKED_DATE_AHEAD)),
            },
        };
    }

    /// Makes the share of the open tab stop working at `at`.
    pub fn pick_date(&self, at: SystemTime) {
        self.lock().tab_mut().draft.expiry = Expiry::On(OffsetDateTime::from(at));
    }

    /// Saves the open tab: makes its share, or changes the saved one. A choice that cannot be
    /// saved, such as a short password or no one to share with, fails with code `invalid` and
    /// a message to show.
    pub async fn save(&self) -> Result<ShareSaved, CoreError> {
        let now = OffsetDateTime::now_utc();
        let (saved, draft) = {
            let inner = self.lock();
            let tab = inner.tab();
            (tab.share.as_ref().map(|share| share.id), tab.draft.clone())
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
                self.lock().tab_mut().show(share);
                Ok(ShareSaved::Updated)
            }
            None => {
                let ids = self.nodes.iter().map(|node| node.id).collect();
                let request = draft.create_request(ids, now).map_err(invalid)?;
                let share =
                    off_ui_thread(async move { client.create_share(&request).await }).await?;
                let made_link = share.is_link();
                self.lock().tab_mut().show(share);
                Ok(if made_link {
                    ShareSaved::Created
                } else {
                    ShareSaved::Updated
                })
            }
        }
    }

    /// Stops sharing the open tab's share: the link or the people lose access at once.
    pub async fn stop(&self) -> Result<(), CoreError> {
        let Some(id) = self.lock().tab().share.as_ref().map(|share| share.id) else {
            return Ok(());
        };
        let client = self.client.clone();
        off_ui_thread(async move { client.delete_share(id).await }).await?;
        let mut inner = self.lock();
        let audience = inner.audience;
        *inner.tab_mut() = Tab::new(audience);
        Ok(())
    }
}

#[uniffi::export]
impl CoreClient {
    /// The share dialog of `owner` for `entries`, editing the shares of exactly them if there
    /// are any. It opens on the people tab, unless only a link shares them.
    pub async fn share_form(
        &self,
        entries: Vec<Entry>,
        owner: String,
    ) -> Result<Arc<ShareForm>, CoreError> {
        let nodes = entries.iter().map(node_of).collect::<Result<Vec<_>, _>>()?;
        let client = self.inner.clone();
        let first = nodes.first().map(|node| node.id);
        let (shares, users) =
            off_ui_thread(async move { tokio::try_join!(client.shares(first), client.users()) })
                .await?;
        let ids: Vec<_> = nodes.iter().map(|node| node.id).collect();
        let mut inner = Inner {
            audience: Audience::People,
            people: Tab::new(Audience::People),
            link: Tab::new(Audience::Link),
            users,
        };
        let with_people = matching_share(&shares, &ids, Audience::People).cloned();
        let with_link = matching_share(&shares, &ids, Audience::Link).cloned();
        if with_people.is_none() && with_link.is_some() {
            inner.audience = Audience::Link;
        }
        if let Some(found) = with_people {
            inner.people.show(found);
        }
        if let Some(found) = with_link {
            inner.link.show(found);
        }
        Ok(Arc::new(ShareForm {
            client: self.inner.clone(),
            nodes,
            owner,
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
    const DANA: &str = "00000000-0000-0000-0000-0000000000d1";

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
        Mock::given(method("GET"))
            .and(path("/api/v1/users"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": DANA, "username": "dana" },
                { "id": "00000000-0000-0000-0000-0000000000d2", "username": "daniel" },
            ])))
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

        let form = client
            .share_form(vec![file], "alice".to_owned())
            .await
            .expect("form");
        form.set_audience(ShareAudience::Link);
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
        let form = client
            .share_form(vec![entry("a.txt")], "alice".to_owned())
            .await
            .expect("form");
        form.set_audience(ShareAudience::Link);
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

        let state = client
            .share_form(vec![file], "alice".to_owned())
            .await
            .expect("form")
            .state();

        assert_eq!(state.audience, ShareAudience::Link);
        assert!(state.url.is_some());
        assert!(state.offers_change_password);
        assert_eq!(state.expiry, ShareExpiry::Never);
    }

    #[tokio::test]
    async fn a_share_with_people_is_made_and_closes() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        shares_answer(&server, json!([])).await;
        let file = entry("a.txt");
        Mock::given(method("POST"))
            .and(path("/api/v1/shares"))
            .and(body_partial_json(
                json!({ "node_ids": [file.id], "recipients": [DANA] }),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": SHARE_ID,
                "items": [node("a.txt", "file", 3)],
                "has_password": false,
                "permission": "read",
                "expires_at": null,
                "created_at": "2026-11-15T10:00:00Z",
                "recipients": [{
                    "user": { "id": DANA, "username": "dana" },
                    "permission": "read",
                    "added_at": "2026-11-15T10:00:00Z"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let form = client
            .share_form(vec![file], "alice".to_owned())
            .await
            .expect("form");
        let state = form.state();
        assert_eq!(state.audience, ShareAudience::People);
        assert_eq!(state.owner, "alice (you)");
        assert_eq!(state.expiry, ShareExpiry::Never);
        let found = form.candidates("DAN".to_owned());
        assert_eq!(found.len(), 2);
        form.add_person(DANA.to_owned());
        assert_eq!(form.candidates("dan".to_owned()).len(), 1);

        assert_eq!(form.save().await.expect("save"), ShareSaved::Updated);
        let people = form.state().people;
        assert_eq!(people.len(), 1);
        assert!(people[0].added_at.is_some());
    }

    #[tokio::test]
    async fn a_share_with_no_one_is_refused() {
        let server = MockServer::start().await;
        let client = signed_in(&server).await;
        shares_answer(&server, json!([])).await;
        let form = client
            .share_form(vec![entry("a.txt")], "alice".to_owned())
            .await
            .expect("form");
        form.add_person(DANA.to_owned());
        form.remove_person(DANA.to_owned()).expect("remove");

        let err = form.save().await.expect_err("refused");

        assert!(matches!(err, CoreError::Failed { code, .. } if code == INVALID_CODE));
    }
}
