use std::{
    hash::{Hash, Hasher},
    time::Duration,
};

use iced::{
    Subscription,
    futures::{SinkExt, Stream, StreamExt, channel::mpsc::Sender},
};
use tokio::time::{Instant, timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite};

use api_client::{ApiError, Client};
use protocol::{Event, TicketRequest};

const MIN_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);
const BUFFER: usize = 64;
/// How long connecting, TLS and the upgrade may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The server pings every 15 seconds, so this long without a frame means the connection is
/// dead even though no error came, as after the laptop slept or changed networks.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
/// A connection that lasted this long counts as working, and the next retry starts quickly again.
const STABLE_AFTER: Duration = Duration::from_secs(20);

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct Connection(Client);

impl Hash for Connection {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.url("").hash(state);
    }
}

/// Live events from the server of `client`, reconnecting with backoff.
///
/// Every connection after the first attempt is preceded by [`Event::Resync`], since events
/// may have been missed meanwhile. The stream ends once the session is no longer valid.
pub fn subscription(client: &Client) -> Subscription<Event> {
    Subscription::run_with(Connection(client.clone()), |connection| {
        listen(connection.0.clone())
    })
}

fn listen(client: Client) -> impl Stream<Item = Event> {
    iced::stream::channel(BUFFER, async move |mut output: Sender<Event>| {
        let mut delay = MIN_RETRY;
        let mut reconnecting = false;
        loop {
            match client.ticket(&TicketRequest::Events).await {
                Ok(ticket) => {
                    let opening = connect_async(client.events_url(&ticket.ticket));
                    if let Ok(Ok((mut socket, _))) = timeout(CONNECT_TIMEOUT, opening).await
                        && greet(&client, &mut socket).await
                    {
                        let opened = Instant::now();
                        let resynced = !reconnecting || output.send(Event::Resync).await.is_ok();
                        if !resynced || !forward(socket, &mut output).await {
                            break;
                        }
                        // A socket closed right after opening keeps backing off.
                        if opened.elapsed() >= STABLE_AFTER {
                            delay = MIN_RETRY;
                        }
                    }
                }
                Err(ApiError::Unauthorized) => break,
                Err(_) => {}
            }
            reconnecting = true;
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_RETRY);
        }
    })
}

/// Sends the hello naming this device; `false` when the socket is already gone.
async fn greet(client: &Client, socket: &mut Socket) -> bool {
    match client.hello() {
        Some(hello) => matches!(
            timeout(
                CONNECT_TIMEOUT,
                socket.send(tungstenite::Message::text(hello))
            )
            .await,
            Ok(Ok(()))
        ),
        None => true,
    }
}

/// Passes events on until the socket closes or goes quiet; `false` once nobody listens anymore.
async fn forward(mut socket: Socket, output: &mut Sender<Event>) -> bool {
    // Any frame, pings included, shows the connection is alive.
    while let Ok(Some(Ok(message))) = timeout(IDLE_TIMEOUT, socket.next()).await {
        if let tungstenite::Message::Text(text) = message
            && let Ok(event) = serde_json::from_str::<Event>(&text)
            && output.send(event).await.is_err()
        {
            return false;
        }
    }
    true
}
