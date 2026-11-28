//! Protected folders on the desktop (`docs/todo/protected-folders.md`): which folders are
//! protected, the keys of the ones the user unlocked, and the file work their encryption needs.
//! The cryptography is `vault-core`'s and the shared rules are `client_core::protected`'s.
//!
//! Work that runs on its own, such as a transfer or a conversion, gets an [`Inside`]: a copy of
//! the keys, which it keeps until it ends even when the folder locks meanwhile. Everything the
//! user sees goes through [`Protected`], which forgets the keys on lock.

pub mod convert;
pub mod files;
pub mod security_key;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use api_client::{ApiError, Client};
use client_core::protected::{
    self as core, ProtectedError, UnlockedFolders, open_listing, open_name, open_path, seal_name,
    shown_mime,
};
use protocol::{Event, Node, NodeId, NodeKind, ProtectedFolder, ProtectionState};
use vault_core::{FolderKeys, Recipient};

/// How often the app looks for folders to lock after their time without activity.
pub const LOCK_CHECK: Duration = Duration::from_secs(15);
/// How often the app looks whether the security key that unlocked a folder is still plugged in.
pub const KEY_CHECK: Duration = Duration::from_secs(2);
/// Said of a folder that locked while something needed its keys.
pub const LOCKED_MEANWHILE: &str = "The folder locked meanwhile; unlock it again.";

/// The keys of an unlocked folder, shared with the work that needs them.
pub type Keys = Arc<FolderKeys>;

/// The protected folders the server lists, with where each is.
#[derive(Debug, Clone, Default)]
pub struct Listed {
    pub folders: Vec<ProtectedFolder>,
    /// The ancestors of each protected folder, to tell a folder that holds one.
    pub ancestors: HashMap<NodeId, Vec<NodeId>>,
}

/// Lists the protected folders and where they are; none on a server without them.
pub async fn fetch(client: Client) -> Result<Listed, ApiError> {
    if !client.supports(protocol::features::PROTECTED_FOLDERS) {
        return Ok(Listed::default());
    }
    let folders = client.protected_folders().await?;
    let mut ancestors = HashMap::new();
    for folder in &folders {
        let path = client.ancestors(folder.node_id).await?;
        ancestors.insert(
            folder.node_id,
            path.into_iter().map(|node| node.id).collect(),
        );
    }
    Ok(Listed { folders, ancestors })
}

/// The protected folders and the keys of the unlocked ones.
#[derive(Debug)]
pub struct Protected {
    listed: Listed,
    unlocked: UnlockedFolders,
    /// Where the security key that unlocked a folder is plugged in: the folder locks when it is
    /// unplugged.
    unlocked_with: HashMap<NodeId, String>,
    /// The unlocked folders the user agreed to take plain copies out of (opening with another
    /// app, downloading), until they lock.
    plain_copies: HashSet<NodeId>,
    started: Instant,
}

impl Default for Protected {
    fn default() -> Self {
        Self {
            listed: Listed::default(),
            unlocked: UnlockedFolders::default(),
            unlocked_with: HashMap::new(),
            plain_copies: HashSet::new(),
            started: Instant::now(),
        }
    }
}

impl Protected {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    /// Takes in the server's list; returns the folders that were unlocked and are no longer
    /// protected, now locked.
    pub fn loaded(&mut self, listed: Listed) -> Vec<NodeId> {
        let gone: Vec<NodeId> = self
            .unlocked
            .folders()
            .filter(|id| !listed.folders.iter().any(|folder| folder.node_id == *id))
            .collect();
        for id in &gone {
            self.unlocked.lock(*id);
        }
        self.listed = listed;
        gone
    }

    /// The protected folder `id`, when it is one.
    pub fn get(&self, id: NodeId) -> Option<&ProtectedFolder> {
        self.listed
            .folders
            .iter()
            .find(|folder| folder.node_id == id)
    }

    /// The protected folder that holds the folder whose path, Home's child first, is `path`.
    pub fn enclosing(&self, path: &[Node]) -> Option<&ProtectedFolder> {
        core::enclosing(&self.listed.folders, path.iter().map(|node| node.id))
    }

    /// Whether `id` is a protected folder or holds one.
    pub fn is_or_holds(&self, id: NodeId) -> bool {
        self.get(id).is_some()
            || self
                .listed
                .ancestors
                .values()
                .any(|ancestors| ancestors.contains(&id))
    }

    /// The ids of every protected folder.
    pub fn ids(&self) -> Vec<NodeId> {
        self.listed
            .folders
            .iter()
            .map(|folder| folder.node_id)
            .collect()
    }

    pub fn is_unlocked(&self, id: NodeId) -> bool {
        self.unlocked.is_unlocked(id, self.now())
    }

    /// Whether any folder is unlocked.
    pub fn any_unlocked(&self) -> bool {
        self.unlocked.folders().next().is_some()
    }

    /// How many folders are unlocked.
    pub fn unlocked_count(&self) -> usize {
        self.unlocked.folders().count()
    }

    /// Whether `id` is protected, and then whether it is unlocked: what its icon shows.
    pub fn lock_of(&self, id: NodeId) -> Option<bool> {
        self.get(id).map(|_| self.is_unlocked(id))
    }

    /// What work inside the unlocked folder `root` needs, counting as activity in it; `None`
    /// when it is locked or not protected.
    pub fn inside(&mut self, root: NodeId) -> Option<Inside> {
        let state = self.get(root)?.state;
        let now = self.now();
        let keys = self.unlocked.shared_keys(root, now)?;
        Some(Inside { root, state, keys })
    }

    /// Keeps the keys of `folder`, once they match the public key the server lists for it.
    pub fn unlock(&mut self, folder: &ProtectedFolder, keys: Keys) -> Result<(), ProtectedError> {
        self.plain_copies.remove(&folder.node_id);
        let now = self.now();
        self.unlocked
            .unlock(folder.node_id, keys, &folder.recipient, now)
    }

    /// Remembers that the security key plugged in at `path` unlocked `id`, so that unplugging
    /// it locks the folder.
    pub fn unlocked_with_key(&mut self, id: NodeId, path: String) {
        self.unlocked_with.insert(id, path);
    }

    /// Remembers that the user agreed to take plain copies out of `id` until it locks.
    pub fn allow_plain_copies(&mut self, id: NodeId) {
        self.plain_copies.insert(id);
    }

    /// Whether the user agreed to take plain copies out of `id`, unlocked since.
    pub fn plain_copies_allowed(&self, id: NodeId) -> bool {
        self.plain_copies.contains(&id) && self.is_unlocked(id)
    }

    /// Whether a folder is unlocked that locks when its security key is unplugged.
    pub fn any_unlocked_with_key(&self) -> bool {
        self.unlocked_with.keys().any(|id| self.is_unlocked(*id))
    }

    /// Locks the folders whose security key is no longer plugged in, `plugged` listing where
    /// keys are; the ones it locked.
    pub fn lock_unplugged(&mut self, plugged: &[String]) -> Vec<NodeId> {
        let unplugged: Vec<NodeId> = self
            .unlocked_with
            .iter()
            .filter(|(_, path)| !plugged.contains(path))
            .map(|(id, _)| *id)
            .collect();
        unplugged.into_iter().filter(|id| self.lock(*id)).collect()
    }

    /// Locks `id`, wiping its keys and the decrypted copies opened from it; whether it was
    /// unlocked.
    pub fn lock(&mut self, id: NodeId) -> bool {
        files::forget_opened(id);
        self.unlocked_with.remove(&id);
        self.plain_copies.remove(&id);
        self.unlocked.lock(id)
    }

    /// Locks every folder; the ones that were unlocked.
    pub fn lock_all(&mut self) -> Vec<NodeId> {
        self.unlocked_with.clear();
        self.plain_copies.clear();
        let locked = self.unlocked.lock_all();
        locked.iter().copied().for_each(files::forget_opened);
        locked
    }

    /// Locks the folders past their time without activity; the ones it locked.
    pub fn lock_idle(&mut self) -> Vec<NodeId> {
        let now = self.now();
        let locked = self.unlocked.lock_idle(now);
        locked.iter().copied().for_each(files::forget_opened);
        for id in &locked {
            self.unlocked_with.remove(id);
            self.plain_copies.remove(id);
        }
        locked
    }

    /// Takes in a protection the server answered with, such as a new one or a new state.
    pub fn remember(&mut self, folder: ProtectedFolder) {
        match self
            .listed
            .folders
            .iter_mut()
            .find(|known| known.node_id == folder.node_id)
        {
            Some(known) => *known = folder,
            None => self.listed.folders.push(folder),
        }
    }

    /// Drops `id`, no longer protected, locking it.
    pub fn forget(&mut self, id: NodeId) {
        self.lock(id);
        self.listed.folders.retain(|folder| folder.node_id != id);
        self.listed.ancestors.remove(&id);
    }
}

/// What work inside an unlocked protected folder needs: which folder it is and its keys.
#[derive(Clone)]
pub struct Inside {
    pub root: NodeId,
    pub state: ProtectionState,
    keys: Keys,
}

impl std::fmt::Debug for Inside {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inside")
            .field("root", &self.root)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Inside {
    #[cfg(test)]
    pub fn new(root: NodeId, state: ProtectionState, keys: FolderKeys) -> Self {
        Self {
            root,
            state,
            keys: Arc::new(keys),
        }
    }

    pub fn keys(&self) -> &FolderKeys {
        &self.keys
    }

    /// The public key files are encrypted to.
    pub fn recipient(&self) -> Recipient {
        self.keys.recipient()
    }

    /// A listing with its names opened.
    pub fn open_listing(&self, nodes: Vec<Node>) -> Vec<Node> {
        open_listing(&self.keys, nodes, self.state)
    }

    /// A path, Home's child first, with the names below the protected folder opened.
    pub fn open_path(&self, path: Vec<Node>) -> Vec<Node> {
        open_path(&self.keys, path, self.root, self.state)
    }

    /// `node` with its name opened, and a file's type from it; as it is when it does not open.
    pub fn open_node(&self, mut node: Node) -> Node {
        if let Some(name) = open_name(&self.keys, &node) {
            if node.kind == NodeKind::File {
                node.mime = shown_mime(&name).to_owned();
            }
            node.name = name;
        }
        node
    }

    /// The opened name of `node`; `None` when it is not sealed for this folder.
    pub fn name_of(&self, node: &Node) -> Option<String> {
        open_name(&self.keys, node)
    }

    /// `event` with the names of the nodes in it opened.
    pub fn open_event(&self, event: &Event) -> Event {
        match event {
            Event::NodeCreated { node } => Event::NodeCreated {
                node: self.open_node(node.clone()),
            },
            Event::NodeUpdated { node } => Event::NodeUpdated {
                node: self.open_node(node.clone()),
            },
            other => other.clone(),
        }
    }

    /// `name` sealed for a `kind` in `parent`, which is inside the folder.
    pub fn seal(
        &self,
        parent: Option<NodeId>,
        kind: NodeKind,
        name: &str,
    ) -> Result<String, String> {
        let parent = parent.ok_or_else(|| NOT_INSIDE.to_owned())?;
        seal_name(&self.keys, parent, kind, name).map_err(|err| err.to_string())
    }
}

const NOT_INSIDE: &str = "Only the folders inside a protected folder hold its names.";

/// What the folder shown is, as far as protection goes.
#[derive(Debug, Clone, Default)]
pub enum Protection {
    /// An ordinary folder.
    #[default]
    None,
    /// Inside a protected folder, or one itself, that is locked: nothing of it is listed.
    Locked(ProtectedFolder),
    /// Inside an unlocked protected folder, or one itself.
    Unlocked(ProtectedFolder),
}

impl Protection {
    /// The protected folder it is in, locked or not.
    pub fn folder(&self) -> Option<&ProtectedFolder> {
        match self {
            Self::None => None,
            Self::Locked(folder) | Self::Unlocked(folder) => Some(folder),
        }
    }

    pub fn root(&self) -> Option<NodeId> {
        self.folder().map(|folder| folder.node_id)
    }

    pub fn is_locked(&self) -> bool {
        matches!(self, Self::Locked(_))
    }

    /// Whether the folder is being unprotected, when nothing may change in it.
    pub fn is_frozen(&self) -> bool {
        self.folder()
            .is_some_and(|folder| folder.state == ProtectionState::Unprotecting)
    }
}

#[cfg(test)]
mod tests;
