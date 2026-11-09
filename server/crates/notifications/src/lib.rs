//! Per-user live update events delivered over websockets, and which devices have one open.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use tokio::sync::broadcast;
use uuid::Uuid;

use db::UserId;

pub use protocol::Event;

const CHANNEL_CAPACITY: usize = 256;

type Channels = HashMap<UserId, broadcast::Sender<Event>>;
type Connections = HashMap<(UserId, Uuid), usize>;

/// Fan-out of events to the connected sessions of each user, and the count of open
/// websockets per device, which makes a device online.
#[derive(Clone, Debug, Default)]
pub struct Hub {
    channels: Arc<Mutex<Channels>>,
    connections: Arc<Mutex<Connections>>,
}

/// One open websocket of a device; the device stays online while any of its guards lives.
#[derive(Debug)]
#[must_use = "the device goes offline again once the guard is dropped"]
pub struct Presence {
    hub: Hub,
    user: UserId,
    device: Uuid,
}

impl Drop for Presence {
    fn drop(&mut self) {
        let key = (self.user, self.device);
        let offline = {
            let mut connections = self.hub.connections();
            match connections.get_mut(&key) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => connections.remove(&key).is_some(),
                None => false,
            }
        };
        if offline {
            self.hub.publish(self.user, Event::DevicesChanged);
        }
    }
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribes to the events of `user`.
    pub fn subscribe(&self, user: UserId) -> broadcast::Receiver<Event> {
        self.lock()
            .entry(user)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .subscribe()
    }

    /// Subscribes to the events of `user` unless `limit` subscribers already listen to them.
    pub fn subscribe_within(
        &self,
        user: UserId,
        limit: usize,
    ) -> Option<broadcast::Receiver<Event>> {
        let mut channels = self.lock();
        let sender = channels
            .entry(user)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        (sender.receiver_count() < limit).then(|| sender.subscribe())
    }

    /// Sends `event` to every session of `user`; a no-op when nobody listens.
    pub fn publish(&self, user: UserId, event: Event) {
        let mut channels = self.lock();
        let delivered = channels
            .get(&user)
            .is_some_and(|sender| sender.send(event).is_ok());
        if !delivered {
            channels.remove(&user);
        }
    }

    /// Marks `device` of `user` online for as long as the returned guard lives. The first
    /// connection of a device tells the user's sessions with [`Event::DevicesChanged`].
    pub fn connect(&self, user: UserId, device: Uuid) -> Presence {
        let online = {
            let mut connections = self.connections();
            let count = connections.entry((user, device)).or_default();
            *count += 1;
            *count == 1
        };
        if online {
            self.publish(user, Event::DevicesChanged);
        }
        Presence {
            hub: self.clone(),
            user,
            device,
        }
    }

    /// Whether `device` of `user` has a websocket open.
    pub fn is_online(&self, user: UserId, device: Uuid) -> bool {
        self.connections().contains_key(&(user, device))
    }

    fn lock(&self) -> MutexGuard<'_, Channels> {
        self.channels.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn connections(&self) -> MutexGuard<'_, Connections> {
        self.connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use db::NodeId;

    use super::*;

    fn deleted() -> Event {
        Event::NodeDeleted {
            id: NodeId::new(),
            parent_id: None,
        }
    }

    #[tokio::test]
    async fn subscriber_receives_events_for_its_user() {
        let hub = Hub::new();
        let user = UserId::new();
        let mut rx = hub.subscribe(user);
        let event = deleted();
        hub.publish(user, event.clone());
        assert_eq!(rx.recv().await.expect("event"), event);
    }

    #[tokio::test]
    async fn other_users_do_not_receive_events() {
        let hub = Hub::new();
        let mut other = hub.subscribe(UserId::new());
        hub.publish(UserId::new(), deleted());
        assert!(other.try_recv().is_err());
    }

    #[test]
    fn a_device_is_online_while_it_is_connected() {
        let hub = Hub::new();
        let user = UserId::new();
        let device = Uuid::now_v7();
        let presence = hub.connect(user, device);
        assert!(hub.is_online(user, device));
        drop(presence);
        assert!(!hub.is_online(user, device));
    }

    #[test]
    fn a_device_stays_online_until_its_last_socket_closes() {
        let hub = Hub::new();
        let user = UserId::new();
        let device = Uuid::now_v7();
        let first = hub.connect(user, device);
        let second = hub.connect(user, device);
        drop(first);
        assert!(hub.is_online(user, device));
        drop(second);
        assert!(!hub.is_online(user, device));
    }

    #[test]
    fn presence_is_per_user() {
        let hub = Hub::new();
        let device = Uuid::now_v7();
        let _alice = hub.connect(UserId::new(), device);
        assert!(!hub.is_online(UserId::new(), device));
    }

    #[test]
    fn going_online_and_offline_tells_the_user() {
        let hub = Hub::new();
        let user = UserId::new();
        let mut rx = hub.subscribe(user);
        let presence = hub.connect(user, Uuid::now_v7());
        assert_eq!(rx.try_recv().expect("online"), Event::DevicesChanged);
        drop(presence);
        assert_eq!(rx.try_recv().expect("offline"), Event::DevicesChanged);
    }

    #[test]
    fn another_socket_of_an_online_device_tells_nobody() {
        let hub = Hub::new();
        let user = UserId::new();
        let device = Uuid::now_v7();
        let _first = hub.connect(user, device);
        let mut rx = hub.subscribe(user);
        let second = hub.connect(user, device);
        drop(second);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn publishing_without_subscribers_drops_the_channel() {
        let hub = Hub::new();
        let user = UserId::new();
        drop(hub.subscribe(user));
        hub.publish(user, deleted());
        assert!(hub.lock().is_empty());
    }
}
