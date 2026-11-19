//! What the share service answers with.

use db::{Node, OwnedShare, ReceivedShare, ShareLink, ShareRecipient};

/// A share as its owner sees it, with its live items and recipients.
#[derive(Clone, Debug)]
pub struct ShareDetails {
    pub share: OwnedShare,
    /// The shared items not in the owner's recycle bin.
    pub items: Vec<Node>,
    /// Who it is for; empty for a link.
    pub recipients: Vec<ShareRecipient>,
}

/// A share made for the reader, with its live items.
#[derive(Clone, Debug)]
pub struct ReceivedDetails {
    pub share: ReceivedShare,
    pub items: Vec<Node>,
}

/// A link as a visitor sees it.
#[derive(Clone, Debug)]
pub struct LinkView {
    pub link: ShareLink,
    /// The shared items; `None` while the link waits for its password.
    pub items: Option<Vec<Node>>,
}
