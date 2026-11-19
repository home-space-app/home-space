//! Domain events: what a service changed, told to whoever reacts to it.
//!
//! Services publish an event for every change they make, and do not know who listens. The
//! server's dispatcher turns each one into search index updates and websocket messages, so a
//! new endpoint that changes files gets both without remembering either.

mod event;
mod sink;

pub use event::DomainEvent;
pub use sink::{EventSink, Events};
