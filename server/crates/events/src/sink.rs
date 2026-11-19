use std::{fmt, sync::Arc};

use futures_util::future::{self, BoxFuture};

use crate::DomainEvent;

/// Whatever reacts to domain events.
///
/// A trait because the reactions (search, websockets) live in crates that depend on the
/// services publishing the events, so the services cannot name them.
pub trait EventSink: Send + Sync + fmt::Debug {
    /// Reacts to `events`, in order. Failures are the sink's to log: the change the events
    /// tell of has already happened.
    fn publish(&self, events: Vec<DomainEvent>) -> BoxFuture<'_, ()>;
}

/// Where a service publishes its events; cheap to clone.
#[derive(Clone, Debug)]
pub struct Events(Arc<dyn EventSink>);

impl Events {
    pub fn new(sink: impl EventSink + 'static) -> Self {
        Self(Arc::new(sink))
    }

    /// Events that nobody hears, for services used without a server around them.
    pub fn none() -> Self {
        Self::new(Discard)
    }

    /// Publishes `events` and waits until they are handled.
    pub async fn publish(&self, events: Vec<DomainEvent>) {
        if !events.is_empty() {
            self.0.publish(events).await;
        }
    }

    pub async fn publish_one(&self, event: DomainEvent) {
        self.publish(vec![event]).await;
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::none()
    }
}

#[derive(Debug)]
struct Discard;

impl EventSink for Discard {
    fn publish(&self, _events: Vec<DomainEvent>) -> BoxFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}
