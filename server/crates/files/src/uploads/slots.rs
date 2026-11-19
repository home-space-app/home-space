//! What this server keeps in memory about the uploads in progress: the running hash of each
//! one's bytes, behind a lock that also keeps its chunks in order.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, PoisonError},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The running hash of an upload's received bytes.
pub(super) struct Hashing {
    pub(super) hasher: Sha256,
    pub(super) hashed: u64,
}

impl Hashing {
    pub(super) fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            hashed: 0,
        }
    }
}

/// One upload's in-memory state. Its lock also keeps chunks of the same upload in order.
pub(super) type Slot = Arc<tokio::sync::Mutex<Option<Hashing>>>;

/// In-memory state of the uploads in progress on this server.
#[derive(Clone, Default)]
pub(crate) struct Uploads {
    slots: Arc<Mutex<HashMap<Uuid, Slot>>>,
}

impl fmt::Debug for Uploads {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Uploads").finish_non_exhaustive()
    }
}

impl Uploads {
    pub(super) fn slot(&self, id: Uuid) -> Slot {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.entry(id).or_default().clone()
    }

    pub(super) fn insert(&self, id: Uuid, hashing: Option<Hashing>) {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.insert(id, Arc::new(tokio::sync::Mutex::new(hashing)));
    }

    pub(crate) fn remove(&self, id: Uuid) {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.remove(&id);
    }

    #[cfg(test)]
    pub(crate) fn forget_all(&self) {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.clear();
    }
}
