//! The keys of the folders the user unlocked, each locked again after a while without activity.
//!
//! Time is whatever monotonic clock the caller has (`Instant` on the desktop, `performance.now()`
//! in the browser), given as the time since a fixed origin of the caller's choosing.

use std::collections::HashMap;
use std::time::Duration;

use protocol::NodeId;
use vault_core::{FolderKeys, Recipient};

use super::ProtectedError;

/// How long an unlocked folder stays unlocked without activity in it: change this to change the
/// auto-lock of every client.
pub const AUTO_LOCK_AFTER: Duration = Duration::from_secs(15 * 60);

/// The unlocked folders: their keys, wiped when locked, and when each was last used.
#[derive(Debug)]
pub struct UnlockedFolders {
    idle_limit: Duration,
    folders: HashMap<NodeId, Unlocked>,
}

#[derive(Debug)]
struct Unlocked {
    keys: FolderKeys,
    recipient: Recipient,
    last_active: Duration,
}

impl Default for UnlockedFolders {
    fn default() -> Self {
        Self::new(AUTO_LOCK_AFTER)
    }
}

impl UnlockedFolders {
    /// No folder unlocked; each locks after `idle_limit` without activity.
    pub fn new(idle_limit: Duration) -> Self {
        Self {
            idle_limit,
            folders: HashMap::new(),
        }
    }

    /// Keeps the keys of `folder`, once they are checked against `listed_recipient`, the public
    /// key the server lists for it: files are encrypted to it without the keys at hand.
    pub fn unlock(
        &mut self,
        folder: NodeId,
        keys: FolderKeys,
        listed_recipient: &str,
        now: Duration,
    ) -> Result<(), ProtectedError> {
        let recipient = keys.check_recipient(listed_recipient)?;
        self.folders.insert(
            folder,
            Unlocked {
                keys,
                recipient,
                last_active: now,
            },
        );
        Ok(())
    }

    /// Whether `folder` is unlocked and not past its idle limit.
    pub fn is_unlocked(&self, folder: NodeId, now: Duration) -> bool {
        self.folders
            .get(&folder)
            .is_some_and(|unlocked| !self.is_idle(unlocked, now))
    }

    /// The keys of `folder`, counting this as activity in it; `None` when it is locked. A
    /// folder past its idle limit locks here, should the timer that locks it not have run yet.
    pub fn use_keys(&mut self, folder: NodeId, now: Duration) -> Option<&FolderKeys> {
        if !self.is_unlocked(folder, now) {
            self.folders.remove(&folder);
            return None;
        }
        self.folders.get_mut(&folder).map(|unlocked| {
            unlocked.last_active = now;
            &unlocked.keys
        })
    }

    /// The checked public key of `folder`, to encrypt an upload to. Counts as activity. The
    /// caller keeps a copy for an upload in progress, which goes on after the folder locks.
    pub fn recipient(&mut self, folder: NodeId, now: Duration) -> Option<Recipient> {
        self.use_keys(folder, now)?;
        self.folders
            .get(&folder)
            .map(|unlocked| unlocked.recipient.clone())
    }

    /// Locks `folder`, wiping its keys; whether it was unlocked.
    pub fn lock(&mut self, folder: NodeId) -> bool {
        self.folders.remove(&folder).is_some()
    }

    /// Locks every folder; the ones that were unlocked.
    pub fn lock_all(&mut self) -> Vec<NodeId> {
        self.folders.drain().map(|(folder, _)| folder).collect()
    }

    /// Locks the folders past their idle limit; the ones it locked.
    pub fn lock_idle(&mut self, now: Duration) -> Vec<NodeId> {
        let idle: Vec<NodeId> = self
            .folders
            .iter()
            .filter(|(_, unlocked)| self.is_idle(unlocked, now))
            .map(|(folder, _)| *folder)
            .collect();
        idle.iter().for_each(|folder| {
            self.folders.remove(folder);
        });
        idle
    }

    /// How long until the next folder locks by itself; `None` when none is unlocked.
    pub fn next_lock_in(&self, now: Duration) -> Option<Duration> {
        self.folders
            .values()
            .map(|unlocked| (unlocked.last_active + self.idle_limit).saturating_sub(now))
            .min()
    }

    /// The folders that are unlocked.
    pub fn folders(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.folders.keys().copied()
    }

    fn is_idle(&self, unlocked: &Unlocked, now: Duration) -> bool {
        now.saturating_sub(unlocked.last_active) >= self.idle_limit
    }
}
