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
        Query, State, WebSocketUpgrade,
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
use notifications::{Event, Presence};
use protocol::{ClientMessage, DeviceInfo, TicketRequest};

use crate::{
    ApiError, AppState,
    extract::{AuthUser, TicketQuery},
};

/// How often the server pings, so that a client gone silent is noticed.
const PING_INTERVAL: Duration = Duration::from_secs(15);
/// How long a client may stay silent, pongs included, before its socket is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
/// How long one message may take to go out. A client that stops reading would otherwise hold
/// the socket's loop, and with it the idle, expiry and session checks, for as long as it likes.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
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
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // Listen before the session is checked, so that it cannot end unnoticed in between.
    let ended = state.auth.ended_sessions();
    let (grant, lifetime) = match query.ticket {
        Some(ticket) => (
            state
                .auth
                .verify_ticket(&ticket, &TicketRequest::Events)
                .await?,
            state.auth.access_ttl().unsigned_abs(),
        ),
        None => {
            let user = AuthUser::from_headers(&headers, &state).await?;
            let grant = Grant {
                user_id: user.id,
                session_id: user.session_id,
                device_id: user.device_id,
            };
            (grant, remaining_lifetime(user.token_exp))
        }
    };
    let events = state.hub.subscribe(grant.user_id);
    let feeds = Feeds { events, ended };
    Ok(ws.on_upgrade(move |socket| session(state, grant, socket, feeds, lifetime)))
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

async fn session(
    state: AppState,
    grant: Grant,
    mut socket: WebSocket,
    Feeds {
        mut events,
        mut ended,
    }: Feeds,
    lifetime: Duration,
) {
    let mut online = grant
        .device_id
        .map(|device| (device, state.hub.connect(grant.user_id, device)));
    // The device a hello named that could not be registered yet; tried again at each ping.
    let mut unregistered = None;
    let expiry = tokio::time::sleep(lifetime);
    tokio::pin!(expiry);
    let mut ping = interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    let mut heard = Instant::now();
    loop {
        // Biased, so that a session that ended is noticed before any further event is sent.
        let outgoing = tokio::select! {
            biased;
            () = &mut expiry => {
                close(&mut socket, close_code::AGAIN, CREDENTIALS_EXPIRED).await;
                break;
            }
            notice = ended.recv() => {
                let recheck = match notice {
                    Ok(user) => user == grant.user_id,
                    Err(RecvError::Lagged(_)) => true,
                    Err(RecvError::Closed) => false,
                };
                if recheck && !still_live(&state, &grant).await {
                    close_ended(&mut socket).await;
                    break;
                }
                None
            }
            _ = ping.tick() => {
                if heard.elapsed() >= IDLE_TIMEOUT {
                    break;
                }
                // Also catches sessions ended where no notice reaches this server, such as a
                // password changed from the command line.
                if !still_live(&state, &grant).await {
                    close_ended(&mut socket).await;
                    break;
                }
                if online.is_none() {
                    online = register(&state, &grant, &mut unregistered).await;
                }
                Some(Message::Ping(Default::default()))
            }
            event = events.recv() => match event {
                Ok(event) => serde_json::to_string(&event).ok().map(Message::text),
                Err(RecvError::Lagged(_)) => {
                    serde_json::to_string(&Event::Resync).ok().map(Message::text)
                }
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(message)) => {
                    heard = Instant::now();
                    if online.is_none()
                        && let Message::Text(text) = &message
                        && let Ok(ClientMessage::Hello { device }) =
                            serde_json::from_str(text.as_str())
                    {
                        unregistered = Some(device);
                        online = register(&state, &grant, &mut unregistered).await;
                    }
                    None
                }
            },
        };
        if let Some(message) = outgoing
            && !matches!(
                timeout(SEND_TIMEOUT, socket.send(message)).await,
                Ok(Ok(()))
            )
        {
            break;
        }
    }
    if let Some((device, presence)) = online {
        went_offline(&state, &grant, device, presence).await;
    }
}

/// Whether the socket's session still lives. A failed check counts as ended: the client
/// reconnects, and is let back in only with a live session.
async fn still_live(state: &AppState, grant: &Grant) -> bool {
    state.auth.is_live(grant).await.unwrap_or_else(|err| {
        tracing::warn!(error = %err, "checking the websocket's session failed");
        false
    })
}

/// Tells the client that its session has ended before the socket closes.
async fn close_ended(socket: &mut WebSocket) {
    close(socket, close_code::POLICY, SESSION_ENDED).await;
}

/// Sends a close frame, giving up after [`SEND_TIMEOUT`]: the socket closes either way.
async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let _ = timeout(SEND_TIMEOUT, socket.send(Message::Close(Some(frame)))).await;
}

/// Registers the device a client named in its hello, for a session that names none, and marks
/// it online. Anything other than a hello is ignored: the socket still delivers events. A
/// failure that may pass, such as the database being unreachable, leaves the device in
/// `unregistered` to be tried again, so a socket is not left offline for its whole life.
async fn register(
    state: &AppState,
    grant: &Grant,
    unregistered: &mut Option<DeviceInfo>,
) -> Option<(Uuid, Presence)> {
    let device = unregistered.take()?;
    match state.auth.session_device(grant, &device).await {
        Ok(device) => Some((device, state.hub.connect(grant.user_id, device))),
        Err(err @ AuthError::InvalidInput(_)) => {
            tracing::warn!(error = %err, "the websocket's hello names an invalid device");
            None
        }
        Err(err) => {
            tracing::warn!(error = %err, "registering the websocket's device failed, will retry");
            *unregistered = Some(device);
            None
        }
    }
}

/// Records when `device` was last seen, then marks it offline, so a device shown offline
/// already has its last-seen time.
async fn went_offline(state: &AppState, grant: &Grant, device: Uuid, presence: Presence) {
    if let Err(err) = state.db.touch_device(grant.user_id, device).await {
        tracing::warn!(error = %err, "recording when a device was last seen failed");
    }
    drop(presence);
}
