use serde::{Deserialize, Serialize};

use uuid::Uuid;

use crate::{DeviceInfo, Node, NodeId, TransferState, TransferSummary};

/// A change to a user's tree, pushed over the websocket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    NodeCreated {
        node: Node,
    },
    NodeUpdated {
        node: Node,
    },
    NodeDeleted {
        id: NodeId,
        parent_id: Option<NodeId>,
    },
    /// Files entered or left the recycle bin.
    TrashChanged,
    /// A device of the user came online or went offline.
    DevicesChanged,
    /// A transfer to a device of the user is ready to be downloaded. Clients that are not
    /// `transfer.recipient_device` may only refresh what they show.
    TransferReceived {
        transfer: TransferSummary,
    },
    /// A transfer the user sent or receives changed state.
    TransferChanged {
        id: Uuid,
        state: TransferState,
    },
    /// Events were missed; the client should refetch what it displays.
    Resync,
}

/// A message a client sends over the websocket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Names the device on the other end, so the server can register it and mark it online.
    /// Sent once, right after connecting.
    Hello { device: DeviceInfo },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resync_serializes_as_a_bare_tag() {
        assert_eq!(
            serde_json::to_string(&Event::Resync).expect("json"),
            r#"{"type":"resync"}"#
        );
    }

    #[test]
    fn trash_changed_serializes_as_a_bare_tag() {
        assert_eq!(
            serde_json::to_string(&Event::TrashChanged).expect("json"),
            r#"{"type":"trash_changed"}"#
        );
    }

    #[test]
    fn devices_changed_serializes_as_a_bare_tag() {
        assert_eq!(
            serde_json::to_string(&Event::DevicesChanged).expect("json"),
            r#"{"type":"devices_changed"}"#
        );
    }

    #[test]
    fn hello_round_trips() {
        let hello = ClientMessage::Hello {
            device: DeviceInfo {
                id: Uuid::nil(),
                name: "Laptop".to_owned(),
                platform: crate::PLATFORM_LINUX.to_owned(),
            },
        };
        let json = serde_json::to_string(&hello).expect("json");
        assert!(json.contains(r#""type":"hello""#));
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&json).expect("back"),
            hello
        );
    }

    #[test]
    fn deleted_event_round_trips() {
        let event = Event::NodeDeleted {
            id: NodeId(Uuid::nil()),
            parent_id: None,
        };
        let json = serde_json::to_string(&event).expect("json");
        assert!(json.contains(r#""type":"node_deleted""#));
        assert_eq!(serde_json::from_str::<Event>(&json).expect("back"), event);
    }
}
