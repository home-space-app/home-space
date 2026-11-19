use uuid::Uuid;

use db::UserId;

/// Who makes a change: the user, and the device they make it on when the client named one.
/// The device is recorded with the change, so that device does not sync it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Actor {
    pub user: UserId,
    pub device: Option<Uuid>,
}

impl Actor {
    pub fn new(user: UserId, device: Option<Uuid>) -> Self {
        Self { user, device }
    }

    /// `user`, on no device in particular.
    pub fn user(user: UserId) -> Self {
        Self::new(user, None)
    }
}
