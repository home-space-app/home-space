//! Live events from the server, while the app is in front.
//!
//! The apps open a stream when they come to the front and stop it when they leave: iOS suspends
//! a backgrounded app within seconds and Android cuts its network, so a socket kept across that
//! would die without either end noticing. Every connection, the first included, is followed by
//! [`LiveEvent::Resync`], since whatever is shown may have changed while no socket was open.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use api_client::ApiError;
use futures::{SinkExt, StreamExt};
use protocol::{Event, TicketRequest};
use rustls::ClientConfig;
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time::{Instant, sleep, timeout};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config, tungstenite,
};

use crate::{CoreClient, RUNTIME};

const MIN_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// How long connecting, TLS and the upgrade may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The server pings every 15 seconds, so this long without a frame means the connection is
/// dead even though no error came, as after switching between Wi-Fi and mobile data.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
/// A connection that lasted this long counts as working, and the next retry starts quickly again.
const STABLE_AFTER: Duration = Duration::from_secs(20);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The same certificate checks as the rest of the client: the platform's own.
static TLS: LazyLock<Option<Arc<ClientConfig>>> = LazyLock::new(|| {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(BuilderVerifierExt::with_platform_verifier)
        .map(|config| Arc::new(config.with_no_client_auth()))
        .ok()
});

/// What the server pushed, as much of it as the apps act on.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LiveEvent {
    /// A file or folder was created, changed, moved or deleted. `parent_id` is the folder it is
    /// in now (`None` for Home), or was in when deleted. A shown folder that lists `id` or is
    /// `parent_id` needs reloading; a moved node may have left the shown folder.
    NodeChanged {
        id: String,
        parent_id: Option<String>,
    },
    /// Files entered or left the recycle bin.
    TrashChanged,
    /// A device of the user came online, went offline, or was renamed or signed out.
    DevicesChanged,
    /// A transfer arrived for the user, or one they sent or received changed: reload the lists.
    TransfersChanged,
    /// Events may have been missed: reload whatever is shown.
    Resync,
}

impl LiveEvent {
    /// The event the apps are told about.
    fn from_event(event: Event) -> Option<Self> {
        Some(match event {
            Event::NodeCreated { node } | Event::NodeUpdated { node } => Self::NodeChanged {
                id: node.id.to_string(),
                parent_id: node.parent_id.map(|id| id.to_string()),
            },
            Event::NodeDeleted { id, parent_id } => Self::NodeChanged {
                id: id.to_string(),
                parent_id: parent_id.map(|id| id.to_string()),
            },
            Event::TrashChanged => Self::TrashChanged,
            Event::DevicesChanged => Self::DevicesChanged,
            Event::Resync => Self::Resync,
            Event::TransferReceived { .. } | Event::TransferChanged { .. } => {
                Self::TransfersChanged
            }
        })
    }
}

/// Told what the server pushes.
///
/// Called on a background thread, so hand it to the UI thread. It must not throw.
#[uniffi::export(with_foreign)]
pub trait EventsListener: Send + Sync {
    fn event(&self, event: LiveEvent);
    /// The session ended; no further events come. Sign the user out.
    fn session_ended(&self);
}

/// A running stream of live events; it ends on [`stop`](Self::stop) or when dropped.
#[derive(uniffi::Object)]
pub struct EventStream {
    stop: watch::Sender<bool>,
}

#[uniffi::export]
impl EventStream {
    /// Closes the socket and stops reconnecting. Stopping twice does nothing.
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

#[uniffi::export]
impl CoreClient {
    /// Starts passing live events to `listener` until the stream is stopped or the session
    /// ends. Start it when the app comes to the front and stop it when the app leaves.
    pub fn listen_events(&self, listener: Arc<dyn EventsListener>) -> Arc<EventStream> {
        let (stop, stopped) = watch::channel(false);
        RUNTIME.spawn(run(self.inner.clone(), listener, stopped));
        Arc::new(EventStream { stop })
    }
}

async fn run(
    client: api_client::Client,
    listener: Arc<dyn EventsListener>,
    mut stopped: watch::Receiver<bool>,
) {
    let mut delay = MIN_RETRY;
    while !*stopped.borrow() {
        let attempt = async {
            match client.ticket(&TicketRequest::Events).await {
                Ok(ticket) => Ok(connect(&client, &client.events_url(&ticket.ticket)).await),
                Err(err) => Err(err),
            }
        };
        let socket = tokio::select! {
            () = stopping(&mut stopped) => return,
            attempt = attempt => attempt,
        };
        match socket {
            Ok(Some(socket)) => {
                let opened = Instant::now();
                listener.event(LiveEvent::Resync);
                if forward(socket, listener.as_ref(), &mut stopped).await {
                    return;
                }
                if opened.elapsed() >= STABLE_AFTER {
                    delay = MIN_RETRY;
                }
            }
            Ok(None) => {}
            Err(ApiError::Unauthorized) => {
                listener.session_ended();
                return;
            }
            Err(_) => {}
        }
        tokio::select! {
            () = stopping(&mut stopped) => return,
            () = sleep(delay) => {}
        }
        delay = (delay * 2).min(MAX_RETRY);
    }
}

/// Resolves once the stream is stopped. The guard `wait_for` returns is dropped here, since it
/// may not be held across an await.
async fn stopping(stopped: &mut watch::Receiver<bool>) {
    let _ = stopped.wait_for(|stop| *stop).await;
}

/// Opens the socket and sends the hello naming this device; `None` when either fails.
async fn connect(client: &api_client::Client, url: &str) -> Option<Socket> {
    let connector = TLS.clone().map(Connector::Rustls);
    let opening = connect_async_tls_with_config(url, None, false, connector);
    let (mut socket, _) = timeout(CONNECT_TIMEOUT, opening).await.ok()?.ok()?;
    if let Some(hello) = client.hello() {
        timeout(
            CONNECT_TIMEOUT,
            socket.send(tungstenite::Message::text(hello)),
        )
        .await
        .ok()?
        .ok()?;
    }
    Some(socket)
}

/// Passes events on until the socket closes or goes quiet; `true` once stopped.
async fn forward(
    mut socket: Socket,
    listener: &dyn EventsListener,
    stopped: &mut watch::Receiver<bool>,
) -> bool {
    loop {
        let message = tokio::select! {
            () = stopping(stopped) => {
                // Tell the server right away, so the device shows offline now.
                let _ = timeout(Duration::from_secs(2), socket.close(None)).await;
                return true;
            }
            message = timeout(IDLE_TIMEOUT, socket.next()) => message,
        };
        match message {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                if let Some(event) = serde_json::from_str::<Event>(&text)
                    .ok()
                    .and_then(LiveEvent::from_event)
                {
                    listener.event(event);
                }
            }
            // Pings are answered by the socket itself; they only show the line is alive.
            Ok(Some(Ok(tungstenite::Message::Close(_)) | Err(_)) | None) | Err(_) => {
                return false;
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use protocol::{Node, NodeId, NodeKind};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;
    use tokio_tungstenite::accept_async;
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<LiveEvent>>,
        ended: Mutex<bool>,
    }

    impl EventsListener for Recorder {
        fn event(&self, event: LiveEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn session_ended(&self) {
            *self.ended.lock().unwrap() = true;
        }
    }

    fn node(parent: Option<NodeId>) -> Node {
        let now = time::OffsetDateTime::now_utc();
        Node {
            id: NodeId(Uuid::new_v4()),
            parent_id: parent,
            kind: NodeKind::File,
            name: "a.txt".to_owned(),
            size: 1,
            sha256: None,
            mime: "text/plain".to_owned(),
            version: 1,
            created_at: now,
            updated_at: now,
            edited_on: None,
        }
    }

    #[test]
    fn node_events_name_the_node_and_its_folder() {
        let parent = NodeId(Uuid::new_v4());
        let created = node(Some(parent));
        assert_eq!(
            LiveEvent::from_event(Event::NodeCreated {
                node: created.clone()
            }),
            Some(LiveEvent::NodeChanged {
                id: created.id.to_string(),
                parent_id: Some(parent.to_string()),
            })
        );
        assert_eq!(
            LiveEvent::from_event(Event::NodeDeleted {
                id: created.id,
                parent_id: None,
            }),
            Some(LiveEvent::NodeChanged {
                id: created.id.to_string(),
                parent_id: None,
            })
        );
    }

    async fn eventually(check: impl Fn() -> bool) -> bool {
        for _ in 0..100 {
            if check() {
                return true;
            }
            sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_signed_out_client_ends_the_session() {
        let recorder = Arc::new(Recorder::default());
        let (_stop, stopped) = watch::channel(false);
        run(
            api_client::Client::new("http://127.0.0.1:9"),
            recorder.clone(),
            stopped,
        )
        .await;
        assert!(*recorder.ended.lock().unwrap());
        assert!(recorder.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn events_follow_a_resync_and_stopping_closes_the_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let created = node(None);
        let sent = created.clone();
        let (closed_tx, mut closed) = mpsc::channel(1);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let event = serde_json::to_string(&Event::NodeCreated { node: sent }).unwrap();
            socket
                .send(tungstenite::Message::text(event))
                .await
                .unwrap();
            while let Some(Ok(message)) = socket.next().await {
                if message.is_close() {
                    break;
                }
            }
            closed_tx.send(()).await.unwrap();
        });
        let client = api_client::Client::new(format!("http://{addr}"));
        let recorder = Arc::new(Recorder::default());
        let (stop, mut stopped) = watch::channel(false);
        let socket = connect(&client, &client.events_url("t"))
            .await
            .expect("open");
        let forwarding = {
            let recorder = recorder.clone();
            tokio::spawn(async move { forward(socket, recorder.as_ref(), &mut stopped).await })
        };
        let seen = recorder.clone();
        assert!(eventually(|| !seen.events.lock().unwrap().is_empty()).await);
        assert_eq!(
            recorder.events.lock().unwrap()[0],
            LiveEvent::NodeChanged {
                id: created.id.to_string(),
                parent_id: None,
            }
        );
        stop.send_replace(true);
        assert!(forwarding.await.unwrap());
        assert!(timeout(Duration::from_secs(2), closed.recv()).await.is_ok());
    }

    #[tokio::test]
    async fn a_silent_server_counts_as_gone() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _socket = accept_async(stream).await.unwrap();
            std::future::pending::<()>().await;
        });
        let client = api_client::Client::new(format!("http://{addr}"));
        let socket = connect(&client, &client.events_url("t"))
            .await
            .expect("open");
        // Paused only now, so that connecting isn't timed out by the clock jumping ahead.
        tokio::time::pause();
        let (_stop, mut stopped) = watch::channel(false);
        let recorder = Recorder::default();
        assert!(!forward(socket, &recorder, &mut stopped).await);
    }
}
