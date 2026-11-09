use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use futures::future::select;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::{
    LocalStorage, SetValue, StoredValue, WithValue, expect_context, on_cleanup, provide_context,
};
use tokio::sync::{Notify, oneshot};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{MessageEvent, VisibilityState, WebSocket};

use api_client::ApiError;
use protocol::{Event, TicketRequest};

use crate::session;

const MIN_RETRY_MS: u32 = 1_000;
const MAX_RETRY_MS: u32 = 30_000;
/// A page hidden this long gets a new socket when shown again: the old one may have died
/// unnoticed meanwhile, as when the laptop slept.
const HIDDEN_RECONNECT_MS: f64 = 45_000.0;

type Handler = Rc<dyn Fn(Event)>;

/// Handle to the live-events connection; stop it when the page goes away.
pub struct Connection {
    stopped: Rc<Cell<bool>>,
    socket: Rc<RefCell<Option<WebSocket>>>,
}

impl Connection {
    /// Closes the socket and ends reconnecting.
    pub fn stop(&self) {
        self.stopped.set(true);
        if let Some(socket) = self.socket.borrow().as_ref() {
            let _ = socket.close();
        }
    }
}

#[derive(Default)]
struct HubState {
    connection: RefCell<Option<Connection>>,
    subscribers: RefCell<Vec<(u64, Handler)>>,
    next_id: Cell<u64>,
}

/// One live-events connection shared by every page; pages come and go via
/// [`Hub::subscribe`] without reconnecting.
#[derive(Clone, Default)]
pub struct Hub(Rc<HubState>);

impl Hub {
    /// Opens the connection unless it is already open.
    pub fn start(&self) {
        if self.0.connection.borrow().is_some() {
            return;
        }
        let state = Rc::downgrade(&self.0);
        let connection = connect(move |event| {
            let Some(state) = state.upgrade() else {
                return;
            };
            let handlers: Vec<Handler> = state
                .subscribers
                .borrow()
                .iter()
                .map(|(_, handler)| Rc::clone(handler))
                .collect();
            for handler in handlers {
                handler(event.clone());
            }
        });
        *self.0.connection.borrow_mut() = Some(connection);
    }

    /// Closes the connection; subscribers stay registered.
    pub fn stop(&self) {
        if let Some(connection) = self.0.connection.borrow_mut().take() {
            connection.stop();
        }
    }

    /// Passes every event to `on_event` until the returned handle is dropped.
    pub fn subscribe(&self, on_event: impl Fn(Event) + 'static) -> Subscription {
        let id = self.0.next_id.get();
        self.0.next_id.set(id + 1);
        self.0
            .subscribers
            .borrow_mut()
            .push((id, Rc::new(on_event)));
        Subscription {
            state: Rc::downgrade(&self.0),
            id,
        }
    }
}

/// Shares one [`Hub`] with every component below the caller.
pub fn provide_hub() -> StoredValue<Hub, LocalStorage> {
    let hub = StoredValue::new_local(Hub::default());
    provide_context(hub);
    hub
}

/// Passes every live event to `on_event` until the calling component unmounts.
pub fn use_events(on_event: impl Fn(Event) + 'static) {
    let hub = expect_context::<StoredValue<Hub, LocalStorage>>();
    let subscription = StoredValue::new_local(Some(hub.with_value(|hub| hub.subscribe(on_event))));
    on_cleanup(move || subscription.set_value(None));
}

/// Keeps a handler registered with a [`Hub`]; dropping it unsubscribes.
pub struct Subscription {
    state: Weak<HubState>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state
                .subscribers
                .borrow_mut()
                .retain(|(id, _)| *id != self.id);
        }
    }
}

struct Open {
    closed: oneshot::Receiver<()>,
    /// Whether the socket ever opened; one refused at the upgrade never does.
    opened: Rc<Cell<bool>>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut()>,
    _on_open: Closure<dyn FnMut()>,
}

/// Keeps a websocket to the server open, reconnecting with backoff, and passes every
/// event to `on_event`. Once a reconnected socket opens it sends [`Event::Resync`], since
/// events may have been missed meanwhile.
pub fn connect(on_event: impl Fn(Event) + 'static) -> Connection {
    let connection = Connection {
        stopped: Rc::new(Cell::new(false)),
        socket: Rc::new(RefCell::new(None)),
    };
    let handler: Handler = Rc::new(on_event);
    wasm_bindgen_futures::spawn_local(run(
        Rc::clone(&connection.stopped),
        Rc::clone(&connection.socket),
        handler,
    ));
    connection
}

async fn run(stopped: Rc<Cell<bool>>, socket: Rc<RefCell<Option<WebSocket>>>, handler: Handler) {
    let client = session::client();
    let wake = Rc::new(Notify::new());
    let _watchdog = Watchdog::start(&socket, &wake);
    let mut delay = MIN_RETRY_MS;
    let mut reconnecting = false;
    while !stopped.get() {
        match client.ticket(&TicketRequest::Events).await {
            Ok(ticket) => {
                let url = client.events_url(&ticket.ticket);
                if let Some(open) = open(&url, client.hello(), &handler, &socket, reconnecting) {
                    let _ = open.closed.await;
                    // Only a socket that got through the upgrade counts: one refused there,
                    // such as by a proxy that doesn't pass websockets on, keeps backing off.
                    if open.opened.get() {
                        reconnecting = true;
                        delay = MIN_RETRY_MS;
                    }
                }
            }
            Err(ApiError::Unauthorized) => break,
            Err(_) => {}
        }
        if stopped.get() {
            break;
        }
        // The network coming back or the page being shown again cuts the wait short.
        let _ = select(TimeoutFuture::new(delay), Box::pin(wake.notified())).await;
        delay = (delay * 2).min(MAX_RETRY_MS);
    }
}

fn open(
    url: &str,
    hello: Option<String>,
    handler: &Handler,
    socket: &Rc<RefCell<Option<WebSocket>>>,
    resync: bool,
) -> Option<Open> {
    let ws = WebSocket::new(url).ok()?;
    let greeter = ws.clone();
    let opened = Rc::new(Cell::new(false));
    let on_open = {
        let opened = Rc::clone(&opened);
        let handler = Rc::clone(handler);
        Closure::<dyn FnMut()>::new(move || {
            opened.set(true);
            if let Some(hello) = &hello {
                let _ = greeter.send_with_str(hello);
            }
            if resync {
                handler(Event::Resync);
            }
        })
    };
    let (closed_tx, closed) = oneshot::channel();
    let closed_tx = RefCell::new(Some(closed_tx));
    let handler = Rc::clone(handler);
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |message: MessageEvent| {
        if let Some(text) = message.data().as_string()
            && let Ok(event) = serde_json::from_str::<Event>(&text)
        {
            handler(event);
        }
    });
    let on_close = Closure::<dyn FnMut()>::new(move || {
        if let Some(tx) = closed_tx.borrow_mut().take() {
            let _ = tx.send(());
        }
    });
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    *socket.borrow_mut() = Some(ws);
    Some(Open {
        closed,
        opened,
        _on_message: on_message,
        _on_close: on_close,
        _on_open: on_open,
    })
}

/// Reconnects at once when the network comes back, or when the page is shown after being
/// hidden a while. Browsers keep websocket pings from pages, so a socket that died with the
/// network, or while the laptop slept, can look open for minutes; these are the moments that
/// happens. Listens until dropped.
struct Watchdog {
    on_online: Closure<dyn FnMut()>,
    on_visibility: Closure<dyn FnMut()>,
}

impl Watchdog {
    fn start(socket: &Rc<RefCell<Option<WebSocket>>>, wake: &Rc<Notify>) -> Option<Self> {
        let window = web_sys::window()?;
        let document = window.document()?;
        let reconnect = {
            let socket = Rc::clone(socket);
            let wake = Rc::clone(wake);
            Rc::new(move || {
                // Closing ends the current socket's wait; waking ends a backoff's.
                if let Some(socket) = socket.borrow().as_ref() {
                    let _ = socket.close();
                }
                wake.notify_one();
            })
        };
        let on_online = {
            let reconnect = Rc::clone(&reconnect);
            Closure::<dyn FnMut()>::new(move || reconnect())
        };
        let hidden_since = Cell::new(None::<f64>);
        let watched = document.clone();
        let on_visibility = Closure::<dyn FnMut()>::new(move || {
            if watched.visibility_state() == VisibilityState::Hidden {
                hidden_since.set(Some(js_sys::Date::now()));
            } else if let Some(since) = hidden_since.take()
                && js_sys::Date::now() - since >= HIDDEN_RECONNECT_MS
            {
                reconnect();
            }
        });
        window
            .add_event_listener_with_callback("online", on_online.as_ref().unchecked_ref())
            .ok()?;
        document
            .add_event_listener_with_callback(
                "visibilitychange",
                on_visibility.as_ref().unchecked_ref(),
            )
            .ok()?;
        Some(Self {
            on_online,
            on_visibility,
        })
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let _ = window
            .remove_event_listener_with_callback("online", self.on_online.as_ref().unchecked_ref());
        if let Some(document) = window.document() {
            let _ = document.remove_event_listener_with_callback(
                "visibilitychange",
                self.on_visibility.as_ref().unchecked_ref(),
            );
        }
    }
}
