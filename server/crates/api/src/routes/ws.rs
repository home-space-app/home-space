//! The events websocket. While it is open, the device on the other end counts as online; a
//! session that names no device takes the one the client names in its hello.
//!
//! A socket lives only as long as its session: once the session ends (logout, signing out its
//! device, a reused refresh token, a password change) the socket is closed rather than kept
//! delivering the user's events.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket, close_code},
    },
    http::HeaderMap,
    response::Response,
    routing::get,
};
use tokio::{
    sync::broadcast::{Receiver, error::RecvError},
    time::{Instant, interval_at, timeout},
};
use uuid::Uuid;

use auth::{AuthError, Grant};
use db::UserId;
use notifications::{Event, Hub, Presence};
use protocol::{ClientMessage, DeviceInfo, TicketRequest};

use crate::extract::ApiQuery;
use crate::{
    ApiError, AppState,
    browser::refuse_other_sites,
    extract::{AuthUser, Tenant, TicketQuery},
};

/// How often the server pings, so that a client gone silent is noticed.
const PING_INTERVAL: Duration = Duration::from_secs(15);
/// How long a client may stay silent, pongs included, before its socket is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
/// How long one message may take to go out. A client that stops reading would otherwise hold
/// the socket's loop, and with it the idle, expiry and session checks, for as long as it likes.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest message a client may send. The only one that means anything is the hello,
/// which names a device in a few hundred bytes.
const MAX_MESSAGE_BYTES: usize = 4096;
/// Sockets one user may have open at a time, over all devices and browser tabs. Each holds
/// a task and buffers on the server.
const MAX_SOCKETS_PER_USER: usize = 32;
/// Close reason sent when the socket's session has ended.
const SESSION_ENDED: &str = "session ended";
/// Close reason sent when the credentials the socket was opened with have expired; the client
/// reconnects with fresh ones.
const CREDENTIALS_EXPIRED: &str = "credentials expired";

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws", get(upgrade))
}

async fn upgrade(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<TicketQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // A page of another site must not open the user's events, however the socket signs in.
    refuse_other_sites(&headers, &tenant)?;
    // Listen before the session is checked, so that it cannot end unnoticed in between.
    let ended = tenant.auth.ended_sessions();
    let (grant, lifetime) = match query.ticket {
        Some(ticket) => (
            tenant
                .auth
                .verify_ticket(&ticket, &TicketRequest::Events)
                .await?,
            tenant.auth.access_ttl().unsigned_abs(),
        ),
        None => {
            let user = AuthUser::from_headers(&headers, &tenant).await?;
            let grant = Grant {
                user_id: user.id,
                session_id: user.session_id,
                device_id: user.device_id,
            };
            (grant, remaining_lifetime(user.token_exp))
        }
    };
    let events = state
        .hub
        .subscribe_within(grant.user_id, MAX_SOCKETS_PER_USER)
        .ok_or(ApiError::TooManyConnections)?;
    let feeds = Feeds { events, ended };
    let connection = Connection::new(state.hub, tenant, grant);
    Ok(ws
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| session(connection, socket, feeds, lifetime)))
}

fn remaining_lifetime(token_exp: i64) -> Duration {
    let expires_at = UNIX_EPOCH + Duration::from_secs(token_exp.unsigned_abs());
    expires_at
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO)
}

/// What a socket listens to besides its client.
struct Feeds {
    /// The user's events, which the socket forwards.
    events: Receiver<Event>,
    /// Users whose sessions just ended.
    ended: Receiver<UserId>,
}

/// What the socket does after one thing happened.
enum Step {
    Wait,
    Send(Message),
    /// Tells the client why, then closes.
    Close(u16, &'static str),
    /// Closes without a word, as when the client is gone.
    Stop,
}

/// Runs the socket until its client leaves or falls silent, or its session or credentials end.
async fn session(
    mut connection: Connection,
    mut socket: WebSocket,
    Feeds {
        mut events,
        mut ended,
    }: Feeds,
    lifetime: Duration,
) {
    let expiry = tokio::time::sleep(lifetime);
    tokio::pin!(expiry);
    let mut ping = interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    loop {
        // Biased, so that a session that ended is noticed before any further event is sent.
        let step = tokio::select! {
            biased;
            () = &mut expiry => Step::Close(close_code::AGAIN, CREDENTIALS_EXPIRED),
            notice = ended.recv() => connection.on_session_notice(notice).await,
            _ = ping.tick() => connection.on_ping().await,
            event = events.recv() => connection.forward(event).await,
            incoming = socket.recv() => connection.on_message(incoming).await,
        };
        match step {
            Step::Wait => {}
            Step::Send(message) => {
                let sent = timeout(SEND_TIMEOUT, socket.send(message)).await;
                if !matches!(sent, Ok(Ok(()))) {
                    break;
                }
            }
            Step::Close(code, reason) => {
                close(&mut socket, code, reason).await;
                break;
            }
            Step::Stop => break,
        }
    }
    connection.went_offline().await;
}

/// The socket's side of a session: whose it is, and the device it keeps online.
struct Connection {
    hub: Hub,
    tenant: Tenant,
    grant: Grant,
    /// The device and its presence, once the socket knows which device it is.
    online: Option<(Uuid, Presence)>,
    /// The device a hello named that could not be registered yet; tried again at each ping.
    unregistered: Option<DeviceInfo>,
    /// When the client was last heard from.
    heard: Instant,
}

impl Connection {
    fn new(hub: Hub, tenant: Tenant, grant: Grant) -> Self {
        let online = grant
            .device_id
            .map(|device| (device, hub.connect(grant.user_id, device)));
        Self {
            hub,
            tenant,
            grant,
            online,
            unregistered: None,
            heard: Instant::now(),
        }
    }

    /// One of the user's events as a message to the client; a client that fell behind is told
    /// to resync.
    async fn forward(&self, event: Result<Event, RecvError>) -> Step {
        let event = match event {
            Ok(event) => event,
            Err(RecvError::Lagged(_)) => Event::Resync,
            Err(RecvError::Closed) => return Step::Stop,
        };
        serde_json::to_string(&event)
            .ok()
            .map_or(Step::Wait, |text| Step::Send(Message::text(text)))
    }

    /// Someone's sessions ended: closes the socket if its own did.
    async fn on_session_notice(&self, notice: Result<UserId, RecvError>) -> Step {
        let recheck = match notice {
            Ok(user) => user == self.grant.user_id,
            Err(RecvError::Lagged(_)) => true,
            Err(RecvError::Closed) => false,
        };
        if recheck && !self.still_live().await {
            Step::Close(close_code::POLICY, SESSION_ENDED)
        } else {
            Step::Wait
        }
    }

    /// Pings the client, after checking it is still there and its session still lives.
    async fn on_ping(&mut self) -> Step {
        if self.heard.elapsed() >= IDLE_TIMEOUT {
            return Step::Stop;
        }
        // Also catches sessions ended where no notice reaches this server, such as a password
        // changed from the command line.
        if !self.still_live().await {
            return Step::Close(close_code::POLICY, SESSION_ENDED);
        }
        if self.online.is_none() {
            self.register().await;
        }
        Step::Send(Message::Ping(Default::default()))
    }

    /// The client spoke: it is alive, and a hello names its device and its client
    /// version. Anything other than a hello is ignored.
    async fn on_message(&mut self, incoming: Option<Result<Message, axum::Error>>) -> Step {
        let message = match incoming {
            Some(Ok(Message::Close(_)) | Err(_)) | None => return Step::Stop,
            Some(Ok(message)) => message,
        };
        self.heard = Instant::now();
        if let Message::Text(text) = &message
            && let Ok(ClientMessage::Hello { device }) = serde_json::from_str(text.as_str())
        {
            if self.online.is_none() {
                self.unregistered = Some(device);
                self.register().await;
            } else {
                self.record_client(&device).await;
            }
        }
        Step::Wait
    }

    /// Whether the socket's session still lives. A failed check counts as ended: the client
    /// reconnects, and is let back in only with a live session.
    async fn still_live(&self) -> bool {
        let live = self.tenant.auth.is_live(&self.grant).await;
        live.unwrap_or_else(|err| {
            tracing::warn!(error = %err, "checking the websocket's session failed");
            false
        })
    }

    /// Registers the device a client named in its hello, for a session that names none, and
    /// marks it online. A failure that may pass, such as the database being unreachable,
    /// keeps the device to be tried again, so a socket is not left offline for its whole life.
    async fn register(&mut self) {
        let Some(device) = self.unregistered.take() else {
            return;
        };
        match self.tenant.auth.session_device(&self.grant, &device).await {
            Ok(id) => {
                let presence = self.hub.connect(self.grant.user_id, id);
                self.online = Some((id, presence));
            }
            Err(err @ AuthError::InvalidInput(_)) => {
                tracing::warn!(error = %err, "the websocket's hello names an invalid device");
            }
            Err(err) => {
                tracing::warn!(error = %err, "registering the websocket's device failed, will retry");
                self.unregistered = Some(device);
            }
        }
    }

    /// Records the client version a hello names for the session's device, so a client
    /// updated without signing in again is shown with its new version. A failure only leaves
    /// the old version shown.
    async fn record_client(&self, device: &DeviceInfo) {
        if let Err(err) = self.tenant.auth.session_device(&self.grant, device).await {
            tracing::warn!(error = %err, "recording the websocket's client version failed");
        }
    }

    /// Records when the device was last seen, then marks it offline, so a device shown
    /// offline already has its last-seen time.
    async fn went_offline(self) {
        if let Some((device, presence)) = self.online {
            self.tenant.devices.seen(self.grant.user_id, device).await;
            drop(presence);
        }
    }
}

/// Sends a close frame, giving up after [`SEND_TIMEOUT`]: the socket closes either way.
async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let _ = timeout(SEND_TIMEOUT, socket.send(Message::Close(Some(frame)))).await;
}
