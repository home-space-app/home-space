//! The share dialog: its words, and turning what the owner chose into share requests.

use std::collections::HashSet;

use protocol::{CreateShare, Node, NodeId, NodeKind, Share, UpdateShare, UserId, UserRef};
use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// Fewest characters of a link password, as the server requires.
pub const MIN_SHARE_PASSWORD_LEN: usize = 8;
/// How far a saved expiry may be from a preset and still show as that preset's chip.
const PRESET_SLACK: Duration = Duration::minutes(1);
/// How many names of one kind the subtitle lists before summing up the rest.
const LISTED_NAMES: usize = 3;
const DAY: &str = "1 day";
const WEEK: &str = "7 days";
const MONTH: &str = "30 days";
const NEVER: &str = "Never";
/// The chip that asks for a date of the owner's choosing.
pub const PICK_A_DATE: &str = "Pick a date…";
pub const COPY_LINK: &str = "Copy link";
pub const LINK_COPIED: &str = "Copied";
pub const REQUIRE_PASSWORD: &str = "Require a password";
pub const CHANGE_PASSWORD: &str = "Change password";
pub const EXPIRES: &str = "Expires";
pub const STOP_SHARING: &str = "Stop sharing";
pub const PASSWORD_PLACEHOLDER: &str = "Password";
/// Stands for a password that is set but never shown.
pub const HIDDEN_PASSWORD: &str = "••••••••••";
pub const SHORT_PASSWORD: &str = "The password must be at least 8 characters.";
pub const PAST_DATE: &str = "Pick a date in the future.";
pub const BAD_DATE: &str = "Enter the date as YYYY-MM-DD HH:MM.";
/// What the date field of desktop clients expects.
pub const DATE_HINT: &str = "YYYY-MM-DD HH:MM";
pub const PEOPLE_TAB: &str = "People";
pub const LINK_TAB: &str = "Anyone with the link";
/// [`LINK_TAB`] where there is little room.
pub const LINK_TAB_SHORT: &str = "Link";
pub const PEOPLE_WITH_ACCESS: &str = "People with access";
pub const ADD_PEOPLE: &str = "Add people";
pub const ADD_PEOPLE_PLACEHOLDER: &str = "Add people by name";
pub const REMOVE_PERSON: &str = "Remove";
/// The only permission for now; shown, not chosen.
pub const CAN_VIEW: &str = "Can view";
pub const OWNER: &str = "Owner";
pub const NO_RECIPIENTS: &str = "Add at least one person.";
pub const PEOPLE_NOTE: &str =
    "People you add can view and download these items. They can’t share them with anyone else.";

/// When a link stops working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expiry {
    Day,
    Week,
    Month,
    Never,
    /// At a time the owner picked.
    On(OffsetDateTime),
}

impl Expiry {
    /// The chips offered before [`PICK_A_DATE`].
    pub const PRESETS: [Self; 4] = [Self::Day, Self::Week, Self::Month, Self::Never];

    /// The chip of this expiry; a picked date shows as [`PICK_A_DATE`].
    pub fn label(self) -> &'static str {
        match self {
            Self::Day => DAY,
            Self::Week => WEEK,
            Self::Month => MONTH,
            Self::Never => NEVER,
            Self::On(_) => PICK_A_DATE,
        }
    }

    /// When the link stops working if saved at `now`; `None` for never.
    pub fn expires_at(self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        match self {
            Self::Never => None,
            Self::On(at) => Some(at),
            preset => preset.length().map(|length| now + length),
        }
    }

    fn length(self) -> Option<Duration> {
        match self {
            Self::Day => Some(Duration::days(1)),
            Self::Week => Some(Duration::days(7)),
            Self::Month => Some(Duration::days(30)),
            Self::Never | Self::On(_) => None,
        }
    }

    /// The chip that most likely made a link created at `created_at` stop at `expires_at`.
    fn of_saved(created_at: OffsetDateTime, expires_at: Option<OffsetDateTime>) -> Self {
        let Some(at) = expires_at else {
            return Self::Never;
        };
        Self::PRESETS
            .into_iter()
            .find(|preset| {
                preset
                    .length()
                    .is_some_and(|length| (at - created_at - length).abs() < PRESET_SLACK)
            })
            .unwrap_or(Self::On(at))
    }

    /// Whether this is the chip of `other`, a picked date matching any picked date.
    pub fn same_chip(self, other: Self) -> bool {
        matches!((self, other), (Self::On(_), Self::On(_))) || self == other
    }
}

/// Who a share is for: the tabs of the dialog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Audience {
    /// Named users, who read it once signed in.
    #[default]
    People,
    /// Anyone holding the link.
    Link,
}

impl Audience {
    pub const ALL: [Self; 2] = [Self::People, Self::Link];

    pub fn of(share: &Share) -> Self {
        if share.is_link() {
            Self::Link
        } else {
            Self::People
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::People => PEOPLE_TAB,
            Self::Link => LINK_TAB,
        }
    }

    /// [`Self::label`] where there is little room.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::People => PEOPLE_TAB,
            Self::Link => LINK_TAB_SHORT,
        }
    }
}

/// Why the dialog cannot save yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftError {
    ShortPassword,
    PastDate,
    NoRecipients,
}

impl DraftError {
    pub fn message(self) -> &'static str {
        match self {
            Self::ShortPassword => SHORT_PASSWORD,
            Self::PastDate => PAST_DATE,
            Self::NoRecipients => NO_RECIPIENTS,
        }
    }
}

/// What the owner has chosen in one tab of the dialog, for a new share or an existing one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareDraft {
    pub audience: Audience,
    /// The toggle.
    pub require_password: bool,
    /// What is typed in the password field; unused while [`Self::editing_password`] is false.
    pub password: String,
    pub expiry: Expiry,
    had_password: bool,
    changing_password: bool,
    /// The chip shown for the saved share, and when it stops working.
    saved: Option<(Expiry, Option<OffsetDateTime>)>,
    /// Who a share with people is for, by username.
    recipients: Vec<UserRef>,
    saved_recipients: Option<Vec<UserId>>,
}

impl Default for ShareDraft {
    fn default() -> Self {
        Self {
            audience: Audience::Link,
            require_password: false,
            password: String::new(),
            expiry: Expiry::Week,
            had_password: false,
            changing_password: false,
            saved: None,
            recipients: Vec::new(),
            saved_recipients: None,
        }
    }
}

impl ShareDraft {
    /// The draft of a new link: no password, 7 days.
    pub fn new() -> Self {
        Self::default()
    }

    /// The draft of a new share with people: no one yet, never expiring.
    pub fn for_people() -> Self {
        Self {
            audience: Audience::People,
            expiry: Expiry::Never,
            ..Self::default()
        }
    }

    /// The draft of a new share for `audience`.
    pub fn for_audience(audience: Audience) -> Self {
        match audience {
            Audience::People => Self::for_people(),
            Audience::Link => Self::new(),
        }
    }

    /// The draft of an existing share, as it is saved.
    pub fn of(share: &Share) -> Self {
        let recipients: Vec<UserRef> = share
            .recipients
            .iter()
            .map(|recipient| recipient.user.clone())
            .collect();
        Self {
            audience: Audience::of(share),
            saved_recipients: Some(recipients.iter().map(|user| user.id).collect()),
            recipients,
            require_password: share.has_password,
            password: String::new(),
            expiry: Expiry::of_saved(share.created_at, share.expires_at),
            had_password: share.has_password,
            changing_password: false,
            saved: Some((
                Expiry::of_saved(share.created_at, share.expires_at),
                share.expires_at,
            )),
        }
    }

    /// Who the share is for, in the order they were added.
    pub fn recipients(&self) -> &[UserRef] {
        &self.recipients
    }

    /// Adds `user`, unless they are in already.
    pub fn add_recipient(&mut self, user: UserRef) {
        if !self.has_recipient(user.id) {
            self.recipients.push(user);
        }
    }

    pub fn remove_recipient(&mut self, id: UserId) {
        self.recipients.retain(|user| user.id != id);
    }

    pub fn has_recipient(&self, id: UserId) -> bool {
        self.recipients.iter().any(|user| user.id == id)
    }

    /// The `users` not added yet whose name contains `query`, ignoring case.
    pub fn candidates<'a>(&self, users: &'a [UserRef], query: &str) -> Vec<&'a UserRef> {
        let query = query.trim().to_lowercase();
        users
            .iter()
            .filter(|user| !self.has_recipient(user.id))
            .filter(|user| user.username.to_lowercase().contains(&query))
            .collect()
    }

    /// Whether the password field takes a new password; otherwise it shows
    /// [`HIDDEN_PASSWORD`] next to [`CHANGE_PASSWORD`].
    pub fn editing_password(&self) -> bool {
        self.require_password && (!self.had_password || self.changing_password)
    }

    /// Whether to offer [`CHANGE_PASSWORD`].
    pub fn offers_change_password(&self) -> bool {
        self.require_password && !self.editing_password()
    }

    /// Lets the owner type a new password in place of the saved one.
    pub fn change_password(&mut self) {
        self.changing_password = true;
        self.password.clear();
    }

    /// Turns the toggle on or off.
    pub fn set_require_password(&mut self, require: bool) {
        self.require_password = require;
        if !require {
            self.changing_password = false;
            self.password.clear();
        }
    }

    /// When the link stops working if saved at `now`.
    pub fn expires_at(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        match self.saved {
            Some((chip, saved)) if chip == self.expiry => saved,
            _ => self.expiry.expires_at(now),
        }
    }

    /// The first thing that keeps the draft from being saved at `now`.
    pub fn check(&self, now: OffsetDateTime) -> Result<(), DraftError> {
        let expiry_changed = self.expiry_change(now).is_some();
        if self.audience == Audience::People && self.recipients.is_empty() {
            Err(DraftError::NoRecipients)
        } else if self.editing_password() && self.password.chars().count() < MIN_SHARE_PASSWORD_LEN
        {
            Err(DraftError::ShortPassword)
        } else if expiry_changed && self.expires_at(now).is_some_and(|at| at <= now) {
            Err(DraftError::PastDate)
        } else {
            Ok(())
        }
    }

    /// The request that shares `node_ids` as drafted.
    pub fn create_request(
        &self,
        node_ids: Vec<NodeId>,
        now: OffsetDateTime,
    ) -> Result<CreateShare, DraftError> {
        self.check(now)?;
        Ok(CreateShare {
            node_ids,
            password: (self.audience == Audience::Link && self.editing_password())
                .then(|| self.password.clone()),
            expires_at: self.expires_at(now),
            recipients: self.recipient_ids(),
        })
    }

    fn recipient_ids(&self) -> Vec<UserId> {
        match self.audience {
            Audience::People => self.recipients.iter().map(|user| user.id).collect(),
            Audience::Link => Vec::new(),
        }
    }

    /// The request that saves the draft of an existing share; empty when nothing changed.
    pub fn update_request(&self, now: OffsetDateTime) -> Result<UpdateShare, DraftError> {
        self.check(now)?;
        let password = if self.audience == Audience::People {
            None
        } else if self.editing_password() {
            Some(Some(self.password.clone()))
        } else if self.had_password && !self.require_password {
            Some(None)
        } else {
            None
        };
        let ids = self.recipient_ids();
        let recipients_changed = self.audience == Audience::People
            && self.saved_recipients.as_ref().is_none_or(|saved| {
                saved.len() != ids.len() || !ids.iter().all(|id| saved.contains(id))
            });
        Ok(UpdateShare {
            password,
            expires_at: self.expiry_change(now),
            recipients: recipients_changed.then_some(ids),
        })
    }

    fn expiry_change(&self, now: OffsetDateTime) -> Option<Option<OffsetDateTime>> {
        let expires_at = self.expires_at(now);
        match self.saved {
            Some((_, saved)) if saved == expires_at => None,
            _ => Some(expires_at),
        }
    }
}

/// The title of the dialog: `Share “report.pdf”` or `Share 3 items`.
pub fn share_title(items: &[Node]) -> String {
    match items {
        [item] => format!("Share “{}”", item.name),
        items => format!("Share {} items", items.len()),
    }
}

/// Asked before a link stops working: `Stop sharing “report.pdf”?` or `Stop sharing 3 items?`.
pub fn stop_sharing_question(items: &[Node]) -> String {
    match items {
        [item] => format!("{STOP_SHARING} “{}”?", item.name),
        items => format!("{STOP_SHARING} {} items?", items.len()),
    }
}

/// Said under [`stop_sharing_question`] for a link.
pub const STOP_SHARING_NOTE: &str = "Anyone with the link loses access right away.";
/// Said under [`stop_sharing_question`] for a share with people.
pub const STOP_SHARING_PEOPLE_NOTE: &str = "The people it is shared with lose access right away.";

/// Said under [`stop_sharing_question`] for a share for `audience`.
pub fn stop_sharing_note(audience: Audience) -> &'static str {
    match audience {
        Audience::People => STOP_SHARING_PEOPLE_NOTE,
        Audience::Link => STOP_SHARING_NOTE,
    }
}

/// What is shared, by kind: `Invoices and Photos (folders) · report.pdf (file)`.
pub fn share_subtitle(items: &[Node]) -> String {
    [NodeKind::Folder, NodeKind::File]
        .into_iter()
        .filter_map(|kind| {
            let names: Vec<&str> = items
                .iter()
                .filter(|item| item.kind == kind)
                .map(|item| item.name.as_str())
                .collect();
            let noun = match (kind, names.len()) {
                (_, 0) => return None,
                (NodeKind::Folder, 1) => "folder",
                (NodeKind::Folder, _) => "folders",
                (NodeKind::File, 1) => "file",
                (NodeKind::File, _) => "files",
            };
            Some(format!("{} ({noun})", listed(&names)))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `a`, `a and b`, `a, b and c`, or `a, b, c and 2 more`.
fn listed(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_owned(),
        names if names.len() <= LISTED_NAMES => {
            let (last, rest) = names.split_last().map_or((&"", names), |(l, r)| (l, r));
            format!("{} and {last}", rest.join(", "))
        }
        names => format!(
            "{} and {} more",
            names[..LISTED_NAMES].join(", "),
            names.len() - LISTED_NAMES
        ),
    }
}

/// The share for `audience` among `shares` that shares exactly `items`, to edit instead of
/// making another.
pub fn matching_share<'a>(
    shares: &'a [Share],
    items: &[NodeId],
    audience: Audience,
) -> Option<&'a Share> {
    let wanted: HashSet<NodeId> = items.iter().copied().collect();
    shares.iter().find(|share| {
        Audience::of(share) == audience
            && share.items.len() == wanted.len()
            && share.items.iter().all(|item| wanted.contains(&item.id))
    })
}

/// Reads a picked date typed as `YYYY-MM-DD HH:MM` (or with a `T`, as HTML date inputs give
/// it), in the local time of `offset`.
pub fn parse_expiry_date(text: &str, offset: UtcOffset) -> Option<OffsetDateTime> {
    let (date, time) = text.trim().split_once([' ', 'T'])?;
    let mut date_parts = date.split('-').map(str::parse::<i32>);
    let (Some(Ok(year)), Some(Ok(month)), Some(Ok(day)), None) = (
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
    ) else {
        return None;
    };
    let (hour, minute) = time.trim().split_once(':')?;
    let month = u8::try_from(month).ok()?.try_into().ok()?;
    let date = Date::from_calendar_date(year, month, u8::try_from(day).ok()?).ok()?;
    let time = Time::from_hms(hour.parse().ok()?, minute.get(..2)?.parse().ok()?, 0).ok()?;
    Some(PrimitiveDateTime::new(date, time).assume_offset(offset))
}

/// `at` as a desktop date field shows it, in the local time of `offset`.
pub fn format_expiry_date(at: OffsetDateTime, offset: UtcOffset) -> String {
    let at = at.to_offset(offset);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute()
    )
}

/// The settings tab listing the owner's links.
pub const SHARES_TAB: &str = "Shares";
pub const NEVER_EXPIRES: &str = "Never expires";
pub const EXPIRED: &str = "Expired";
pub const PASSWORD_PROTECTED: &str = "Password";
pub const EDIT_SHARE: &str = "Edit…";
pub const OPEN_LINK: &str = "Open link";
pub const NO_SHARES: &str =
    "You have not shared anything yet. Share files and folders from their menu.";
/// The name of a link whose items are all in the recycle bin.
pub const NOTHING_SHARED: &str = "Items in the recycle bin";

/// `sam (you)`, for the owner's row among the people with access.
pub fn you(username: &str) -> String {
    format!("{username} (you)")
}

/// Who a share with people is for: `dana`, `dana and alex`, `dana, alex, kim and 2 more`.
pub fn recipient_names(share: &Share) -> String {
    let names: Vec<&str> = share
        .recipients
        .iter()
        .map(|recipient| recipient.user.username.as_str())
        .collect();
    listed(&names)
}

/// What a link goes by in a list: `report.pdf`, or `Invoices and 2 more`.
pub fn share_name(items: &[Node]) -> String {
    match items {
        [] => NOTHING_SHARED.to_owned(),
        [item] => item.name.clone(),
        [first, rest @ ..] => format!("{} and {} more", first.name, rest.len()),
    }
}

/// Whether a link still works, as seen at `now`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareExpiry {
    Never,
    /// It works until then.
    Until(OffsetDateTime),
    /// It stopped working then.
    Ended(OffsetDateTime),
}

impl ShareExpiry {
    pub fn of(share: &Share, now: OffsetDateTime) -> Self {
        match share.expires_at {
            None => Self::Never,
            Some(at) if at <= now => Self::Ended(at),
            Some(at) => Self::Until(at),
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::{SharePermission, ShareRecipient};
    use time::macros::{datetime, offset};
    use uuid::Uuid;

    use super::*;

    const NOW: OffsetDateTime = datetime!(2026-11-15 10:00 UTC);

    fn node(name: &str, kind: NodeKind) -> Node {
        Node {
            id: NodeId(Uuid::from_u128(name.bytes().map(u128::from).sum())),
            parent_id: None,
            kind,
            name: name.to_owned(),
            size: 0,
            sha256: None,
            mime: String::new(),
            version: 1,
            created_at: NOW,
            updated_at: NOW,
            edited_on: None,
        }
    }

    fn share(items: Vec<Node>, has_password: bool, expires_at: Option<OffsetDateTime>) -> Share {
        Share {
            id: Uuid::nil(),
            token: Some("token".to_owned()),
            items,
            has_password,
            permission: SharePermission::Read,
            expires_at,
            created_at: NOW,
            recipients: Vec::new(),
        }
    }

    fn user(name: &str) -> UserRef {
        UserRef {
            id: UserId(Uuid::from_u128(name.bytes().map(u128::from).sum())),
            username: name.to_owned(),
        }
    }

    fn people_share(items: Vec<Node>, users: &[&str]) -> Share {
        Share {
            token: None,
            recipients: users
                .iter()
                .map(|name| ShareRecipient {
                    user: user(name),
                    permission: SharePermission::Read,
                    added_at: NOW,
                })
                .collect(),
            ..share(items, false, None)
        }
    }

    #[test]
    fn one_item_is_named_in_the_title() {
        assert_eq!(
            share_title(&[node("report.pdf", NodeKind::File)]),
            "Share “report.pdf”"
        );
    }

    #[test]
    fn several_items_are_counted_in_the_title() {
        let items = [
            node("a", NodeKind::File),
            node("b", NodeKind::File),
            node("c", NodeKind::Folder),
        ];
        assert_eq!(share_title(&items), "Share 3 items");
    }

    #[test]
    fn stopping_names_what_stops_being_shared() {
        assert_eq!(
            stop_sharing_question(&[node("Documents", NodeKind::Folder)]),
            "Stop sharing “Documents”?"
        );
    }

    #[test]
    fn the_subtitle_lists_folders_then_files() {
        let items = [
            node("report.pdf", NodeKind::File),
            node("Invoices", NodeKind::Folder),
            node("Photos", NodeKind::Folder),
        ];
        assert_eq!(
            share_subtitle(&items),
            "Invoices and Photos (folders) · report.pdf (file)"
        );
    }

    #[test]
    fn the_subtitle_sums_up_many_names() {
        let items: Vec<Node> = ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(|name| node(name, NodeKind::File))
            .collect();
        assert_eq!(share_subtitle(&items), "a, b, c and 2 more (files)");
    }

    #[test]
    fn a_new_link_lasts_a_week_without_a_password() {
        let request = ShareDraft::new()
            .create_request(vec![], NOW)
            .expect("request");
        assert_eq!(request.password, None);
        assert_eq!(request.expires_at, Some(datetime!(2026-11-22 10:00 UTC)));
    }

    #[test]
    fn a_short_password_is_refused() {
        let mut draft = ShareDraft::new();
        draft.set_require_password(true);
        draft.password = "short".to_owned();
        assert_eq!(draft.check(NOW), Err(DraftError::ShortPassword));
    }

    #[test]
    fn a_picked_date_in_the_past_is_refused() {
        let draft = ShareDraft {
            expiry: Expiry::On(datetime!(2026-11-14 10:00 UTC)),
            ..ShareDraft::new()
        };
        assert_eq!(draft.check(NOW), Err(DraftError::PastDate));
    }

    #[test]
    fn an_unchanged_link_saves_nothing() {
        let saved = share(vec![], true, Some(datetime!(2026-11-19 14:30 UTC)));
        let request = ShareDraft::of(&saved).update_request(NOW).expect("request");
        assert_eq!(request, UpdateShare::default());
    }

    #[test]
    fn a_link_saved_for_a_week_shows_the_week_chip() {
        let saved = share(vec![], false, Some(NOW + Duration::days(7)));
        let draft = ShareDraft::of(&saved);
        assert_eq!(draft.expiry, Expiry::Week);
        assert_eq!(
            draft.expires_at(NOW + Duration::days(2)),
            Some(NOW + Duration::days(7))
        );
    }

    #[test]
    fn a_link_saved_for_another_time_shows_its_date() {
        let at = datetime!(2026-11-19 14:30 UTC);
        assert_eq!(
            ShareDraft::of(&share(vec![], false, Some(at))).expiry,
            Expiry::On(at)
        );
    }

    #[test]
    fn choosing_the_week_again_later_moves_the_expiry() {
        let mut draft = ShareDraft::of(&share(vec![], false, Some(NOW + Duration::days(7))));
        draft.expiry = Expiry::Day;
        draft.expiry = Expiry::Week;
        assert_eq!(draft.update_request(NOW).expect("request").expires_at, None);
        let later = NOW + Duration::days(2);
        draft.expiry = Expiry::Month;
        assert_eq!(
            draft.update_request(later).expect("request").expires_at,
            Some(Some(later + Duration::days(30)))
        );
    }

    #[test]
    fn a_saved_password_is_kept_until_changed() {
        let draft = ShareDraft::of(&share(vec![], true, None));
        assert!(!draft.editing_password());
        assert!(draft.offers_change_password());
    }

    #[test]
    fn turning_the_password_off_removes_it() {
        let mut draft = ShareDraft::of(&share(vec![], true, None));
        draft.set_require_password(false);
        assert_eq!(
            draft.update_request(NOW).expect("request").password,
            Some(None)
        );
    }

    #[test]
    fn a_changed_password_is_sent() {
        let mut draft = ShareDraft::of(&share(vec![], true, None));
        draft.change_password();
        draft.password = "new secret".to_owned();
        assert_eq!(
            draft.update_request(NOW).expect("request").password,
            Some(Some("new secret".to_owned()))
        );
    }

    #[test]
    fn a_saved_expiry_in_the_past_does_not_block_other_changes() {
        let mut draft =
            ShareDraft::of(&share(vec![], false, Some(datetime!(2026-11-01 10:00 UTC))));
        draft.set_require_password(true);
        draft.password = "long enough".to_owned();
        assert_eq!(draft.update_request(NOW).expect("request").expires_at, None);
    }

    #[test]
    fn choosing_never_clears_the_expiry() {
        let mut draft =
            ShareDraft::of(&share(vec![], false, Some(datetime!(2026-11-19 14:30 UTC))));
        draft.expiry = Expiry::Never;
        assert_eq!(
            draft.update_request(NOW).expect("request").expires_at,
            Some(None)
        );
    }

    #[test]
    fn the_link_of_exactly_the_selection_is_found() {
        let a = node("a", NodeKind::File);
        let b = node("b", NodeKind::File);
        let shares = [
            share(vec![a.clone()], false, None),
            share(vec![a.clone(), b.clone()], false, None),
        ];
        let found = matching_share(&shares, &[b.id, a.id], Audience::Link).expect("share");
        assert_eq!(found.items.len(), 2);
        assert!(matching_share(&shares, &[b.id], Audience::Link).is_none());
    }

    #[test]
    fn a_typed_date_is_read_in_local_time() {
        assert_eq!(
            parse_expiry_date("2026-11-19 14:30", offset!(+2)),
            Some(datetime!(2026-11-19 12:30 UTC))
        );
        assert_eq!(
            parse_expiry_date("2026-11-19T14:30", offset!(UTC)),
            Some(datetime!(2026-11-19 14:30 UTC))
        );
        assert_eq!(parse_expiry_date("19/11/2026", offset!(UTC)), None);
        assert_eq!(parse_expiry_date("2026-02-30 10:00", offset!(UTC)), None);
    }

    #[test]
    fn a_date_is_shown_as_it_is_typed() {
        assert_eq!(
            format_expiry_date(datetime!(2026-11-19 12:30 UTC), offset!(+2)),
            "2026-11-19 14:30"
        );
    }

    #[test]
    fn a_link_of_one_item_goes_by_its_name() {
        assert_eq!(
            share_name(&[node("report.pdf", NodeKind::File)]),
            "report.pdf"
        );
    }

    #[test]
    fn a_link_of_several_items_names_the_first_and_counts_the_rest() {
        let items = [
            node("Invoices", NodeKind::Folder),
            node("a", NodeKind::File),
            node("b", NodeKind::File),
        ];
        assert_eq!(share_name(&items), "Invoices and 2 more");
    }

    #[test]
    fn a_link_with_every_item_binned_says_so() {
        assert_eq!(share_name(&[]), NOTHING_SHARED);
    }

    #[test]
    fn a_link_without_expiry_never_expires() {
        assert_eq!(
            ShareExpiry::of(&share(vec![], false, None), NOW),
            ShareExpiry::Never
        );
    }

    #[test]
    fn a_link_works_until_its_expiry() {
        let at = NOW + Duration::days(1);
        assert_eq!(
            ShareExpiry::of(&share(vec![], false, Some(at)), NOW),
            ShareExpiry::Until(at)
        );
    }

    #[test]
    fn a_link_past_its_expiry_has_ended() {
        let at = NOW - Duration::minutes(1);
        assert_eq!(
            ShareExpiry::of(&share(vec![], false, Some(at)), NOW),
            ShareExpiry::Ended(at)
        );
    }

    #[test]
    fn a_new_share_with_people_never_expires() {
        let draft = ShareDraft::for_people();
        assert_eq!(draft.expiry, Expiry::Never);
        assert_eq!(draft.audience, Audience::People);
    }

    #[test]
    fn a_share_with_people_needs_someone() {
        let draft = ShareDraft::for_people();
        assert_eq!(
            draft.create_request(Vec::new(), NOW),
            Err(DraftError::NoRecipients)
        );
    }

    #[test]
    fn a_share_with_people_sends_who_it_is_for() {
        let mut draft = ShareDraft::for_people();
        draft.add_recipient(user("dana"));
        draft.add_recipient(user("dana"));
        let request = draft.create_request(Vec::new(), NOW).expect("request");
        assert_eq!(request.recipients, vec![user("dana").id]);
        assert_eq!(request.password, None);
    }

    #[test]
    fn a_new_link_is_for_no_one_in_particular() {
        let request = ShareDraft::new()
            .create_request(Vec::new(), NOW)
            .expect("request");
        assert!(request.recipients.is_empty());
    }

    #[test]
    fn unchanged_people_save_nothing() {
        let saved = people_share(vec![node("a", NodeKind::File)], &["dana", "alex"]);
        let draft = ShareDraft::of(&saved);
        assert_eq!(draft.audience, Audience::People);
        assert_eq!(draft.update_request(NOW), Ok(UpdateShare::default()));
    }

    #[test]
    fn removing_someone_sends_who_is_left() {
        let saved = people_share(vec![node("a", NodeKind::File)], &["dana", "alex"]);
        let mut draft = ShareDraft::of(&saved);
        draft.remove_recipient(user("dana").id);
        let request = draft.update_request(NOW).expect("request");
        assert_eq!(request.recipients, Some(vec![user("alex").id]));
    }

    #[test]
    fn removing_everyone_is_refused() {
        let saved = people_share(vec![node("a", NodeKind::File)], &["dana"]);
        let mut draft = ShareDraft::of(&saved);
        draft.remove_recipient(user("dana").id);
        assert_eq!(draft.update_request(NOW), Err(DraftError::NoRecipients));
    }

    #[test]
    fn candidates_leave_out_who_is_added_and_match_the_name() {
        let users = [user("dana"), user("alex"), user("Daniel")];
        let mut draft = ShareDraft::for_people();
        draft.add_recipient(user("dana"));
        let found: Vec<&str> = draft
            .candidates(&users, "DAN")
            .into_iter()
            .map(|user| user.username.as_str())
            .collect();
        assert_eq!(found, ["Daniel"]);
    }

    #[test]
    fn the_share_of_the_selection_is_found_for_each_audience() {
        let a = node("a", NodeKind::File);
        let link = share(vec![a.clone()], false, None);
        let people = Share {
            id: Uuid::from_u128(1),
            ..people_share(vec![a.clone()], &["dana"])
        };
        let shares = [link, people];
        assert_eq!(
            matching_share(&shares, &[a.id], Audience::People).map(|share| share.id),
            Some(Uuid::from_u128(1))
        );
        assert_eq!(
            matching_share(&shares, &[a.id], Audience::Link).map(|share| share.id),
            Some(Uuid::nil())
        );
    }

    #[test]
    fn recipients_are_listed_by_name() {
        let shared = people_share(Vec::new(), &["dana", "alex", "kim", "lee", "max"]);
        assert_eq!(recipient_names(&shared), "dana, alex, kim and 2 more");
    }

    #[test]
    fn the_owner_is_marked_as_you() {
        assert_eq!(you("sam"), "sam (you)");
    }
}
