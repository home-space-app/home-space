mod common;

use std::{future::Future, net::SocketAddr, time::Duration};

use axum::http::{Method, StatusCode};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use uuid::Uuid;

use common::{PASSWORD, SHORT_WAIT, TestApp, setup};

const DEVICES: &str = "/api/v1/devices";
const POLL: Duration = Duration::from_millis(20);
const POLLS: usize = 100;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(app: &TestApp) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app.router.clone()).into_future());
    addr
}

async fn sign_in_from(app: &TestApp, username: &str, device: Uuid) -> String {
    sign_in_with_refresh(app, username, device).await.0
}

/// Signs in on `device`, returning the access and the refresh token.
async fn sign_in_with_refresh(app: &TestApp, username: &str, device: Uuid) -> (String, String) {
    app.state.auth.create_user(username, PASSWORD).await.ok();
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/login",
            None,
            json!({
                "username": username,
                "password": PASSWORD,
                "device": { "id": device, "name": "Laptop", "platform": "desktop-linux" },
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let pair = response.json();
    let token = |name: &str| pair[name].as_str().expect("token").to_owned();
    (token("access_token"), token("refresh_token"))
}

async fn hello(socket: &mut Socket, device: Uuid) {
    let hello = json!({
        "type": "hello",
        "device": { "id": device, "name": "Laptop", "platform": "desktop-linux" },
    });
    socket
        .send(Message::text(hello.to_string()))
        .await
        .expect("hello");
}

async fn devices(app: &TestApp, token: &str) -> Vec<Value> {
    let response = app.get(DEVICES, token).await;
    assert_eq!(response.status, StatusCode::OK);
    response.json().as_array().expect("devices").clone()
}

async fn online(app: &TestApp, token: &str, device: Uuid) -> Option<bool> {
    devices(app, token)
        .await
        .iter()
        .find(|listed| listed["id"] == json!(device))
        .and_then(|listed| listed["online"].as_bool())
}

async fn last_seen(app: &TestApp, token: &str) -> OffsetDateTime {
    let listed = devices(app, token).await;
    let seen = listed[0]["last_seen_at"].as_str().expect("last seen");
    OffsetDateTime::parse(seen, &Rfc3339).expect("rfc3339")
}

/// Waits for `check` to hold, since the server handles a socket's open and close on its own.
async fn eventually<F: Future<Output = bool>>(mut check: impl FnMut() -> F) -> bool {
    for _ in 0..POLLS {
        if check().await {
            return true;
        }
        tokio::time::sleep(POLL).await;
    }
    false
}

async fn next_text(socket: &mut Socket) -> Value {
    let message = tokio::time::timeout(SHORT_WAIT * 10, socket.next())
        .await
        .expect("a message")
        .expect("stream open")
        .expect("message");
    let Message::Text(text) = message else {
        panic!("expected text message, got {message:?}");
    };
    serde_json::from_str(text.as_str()).expect("json")
}

async fn connect(addr: SocketAddr, token: &str) -> Socket {
    let mut request = format!("ws://{addr}/api/v1/ws")
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("header"),
    );
    connect_async(request).await.expect("connect").0
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_receives_node_created_and_others_do_not(pool: PgPool) {
    let app = setup(pool).await;
    let alice = app.user_token("alice").await;
    let bob = app.user_token("bob").await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app.router.clone()).into_future());

    let mut alice_ws = connect(addr, &alice).await;
    let mut bob_ws = connect(addr, &bob).await;
    app.upload(&alice, "name=a.txt", "hello").await;

    let message = tokio::time::timeout(SHORT_WAIT * 10, alice_ws.next())
        .await
        .expect("alice should receive an event")
        .expect("stream open")
        .expect("message");
    let Message::Text(text) = message else {
        panic!("expected text message, got {message:?}");
    };
    let event: serde_json::Value = serde_json::from_str(text.as_str()).expect("json");
    assert_eq!(event["type"], "node_created");
    assert_eq!(event["node"]["name"], "a.txt");
    assert!(
        tokio::time::timeout(SHORT_WAIT, bob_ws.next())
            .await
            .is_err(),
        "bob must not receive alice's events"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn websocket_requires_authentication(pool: PgPool) {
    let app = setup(pool).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app.router.clone()).into_future());

    let request = format!("ws://{addr}/api/v1/ws")
        .into_client_request()
        .expect("request");

    assert!(connect_async(request).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_session_without_a_device_registers_the_one_its_hello_names(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let addr = serve(&app).await;
    let device = Uuid::now_v7();

    let mut socket = connect(addr, &token).await;
    hello(&mut socket, device).await;

    assert!(eventually(|| async { online(&app, &token, device).await == Some(true) }).await);
    let created = app.upload(&token, "name=a.txt", "hello").await;
    assert_eq!(created.json()["edited_on"], json!(device));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_hello_whose_device_could_not_be_registered_is_retried_at_the_next_ping(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let token = app.user_token("alice").await;
    let addr = serve(&app).await;
    let device = Uuid::now_v7();
    sqlx::raw_sql(
        "CREATE FUNCTION refuse_devices() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'database unavailable'; END $$; \
         CREATE TRIGGER refuse_devices BEFORE INSERT ON devices \
         FOR EACH ROW EXECUTE FUNCTION refuse_devices();",
    )
    .execute(&pool)
    .await
    .expect("refuse devices");

    let mut socket = connect(addr, &token).await;
    hello(&mut socket, device).await;
    tokio::time::sleep(SHORT_WAIT).await;
    assert_eq!(online(&app, &token, device).await, None);

    sqlx::raw_sql("DROP TRIGGER refuse_devices ON devices")
        .execute(&pool)
        .await
        .expect("accept devices");
    // The socket answers pings meanwhile, as a client would; the next one registers it.
    let registered = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            tokio::select! {
                _ = socket.next() => {}
                () = tokio::time::sleep(POLL) => {
                    if online(&app, &token, device).await == Some(true) {
                        break;
                    }
                }
            }
        }
    });
    assert!(registered.await.is_ok(), "the device should come online");
}

#[sqlx::test(migrations = "../../migrations")]
async fn closing_the_socket_takes_the_device_offline(pool: PgPool) {
    let app = setup(pool).await;
    let device = Uuid::now_v7();
    let token = sign_in_from(&app, "alice", device).await;
    let addr = serve(&app).await;
    let mut socket = connect(addr, &token).await;
    assert!(eventually(|| async { online(&app, &token, device).await == Some(true) }).await);
    let seen_online = last_seen(&app, &token).await;

    socket.close(None).await.expect("close");

    assert!(eventually(|| async { online(&app, &token, device).await == Some(false) }).await);
    assert!(last_seen(&app, &token).await > seen_online);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_device_stays_online_until_its_last_socket_closes(pool: PgPool) {
    let app = setup(pool).await;
    let device = Uuid::now_v7();
    let token = sign_in_from(&app, "alice", device).await;
    let addr = serve(&app).await;
    let mut first = connect(addr, &token).await;
    let mut second = connect(addr, &token).await;
    assert!(eventually(|| async { online(&app, &token, device).await == Some(true) }).await);

    first.close(None).await.expect("close");
    tokio::time::sleep(SHORT_WAIT).await;
    assert_eq!(online(&app, &token, device).await, Some(true));

    second.close(None).await.expect("close");
    assert!(eventually(|| async { online(&app, &token, device).await == Some(false) }).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_session_with_a_device_ignores_the_hello(pool: PgPool) {
    let app = setup(pool).await;
    let device = Uuid::now_v7();
    let token = sign_in_from(&app, "alice", device).await;
    let addr = serve(&app).await;

    let mut socket = connect(addr, &token).await;
    hello(&mut socket, Uuid::now_v7()).await;
    tokio::time::sleep(SHORT_WAIT).await;

    let listed = devices(&app, &token).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], json!(device));
    assert_eq!(listed[0]["online"], json!(true));
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_sockets_of_the_user_hear_a_device_come_online(pool: PgPool) {
    let app = setup(pool).await;
    let token = app.user_token("alice").await;
    let addr = serve(&app).await;
    let mut browser = connect(addr, &token).await;

    let mut laptop = connect(addr, &token).await;
    hello(&mut laptop, Uuid::now_v7()).await;

    assert_eq!(next_text(&mut browser).await["type"], "devices_changed");
}

/// Reads `socket` until the server closes it, failing on any file event on the way: once its
/// session has ended, the socket must not deliver the user's files.
async fn closes_for_ended_session(socket: &mut Socket) {
    loop {
        let message = tokio::time::timeout(SHORT_WAIT * 10, socket.next())
            .await
            .expect("the socket should close");
        match message {
            Some(Ok(Message::Close(frame))) => {
                let frame = frame.expect("a close frame");
                assert_eq!(frame.reason.as_str(), "session ended");
                return;
            }
            Some(Ok(Message::Text(text))) => {
                let event: Value = serde_json::from_str(text.as_str()).expect("json");
                assert_eq!(event["type"], "devices_changed", "leaked {event}");
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return,
        }
    }
}

async fn logout(app: &TestApp, refresh: &str) {
    let response = app
        .json(
            Method::POST,
            "/api/v1/auth/logout",
            None,
            json!({ "refresh_token": refresh }),
        )
        .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn logging_out_closes_the_sessions_socket_only(pool: PgPool) {
    let app = setup(pool).await;
    let (laptop, laptop_refresh) = sign_in_with_refresh(&app, "alice", Uuid::now_v7()).await;
    let phone = sign_in_from(&app, "alice", Uuid::now_v7()).await;
    let addr = serve(&app).await;
    let mut laptop_ws = connect(addr, &laptop).await;
    let mut phone_ws = connect(addr, &phone).await;

    logout(&app, &laptop_refresh).await;
    app.upload(&phone, "name=private.txt", "secret").await;

    closes_for_ended_session(&mut laptop_ws).await;
    loop {
        let event = next_text(&mut phone_ws).await;
        if event["type"] == "node_created" {
            break;
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn signing_out_a_device_closes_its_socket(pool: PgPool) {
    let app = setup(pool).await;
    let laptop_device = Uuid::now_v7();
    let laptop = sign_in_from(&app, "alice", laptop_device).await;
    let phone = sign_in_from(&app, "alice", Uuid::now_v7()).await;
    let addr = serve(&app).await;
    let mut laptop_ws = connect(addr, &laptop).await;

    let response = app
        .send(
            Method::DELETE,
            &format!("{DEVICES}/{laptop_device}"),
            Some(&phone),
            axum::body::Body::empty(),
            None,
        )
        .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    app.upload(&phone, "name=private.txt", "secret").await;

    closes_for_ended_session(&mut laptop_ws).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn reusing_a_refresh_token_closes_the_sessions_socket(pool: PgPool) {
    let app = setup(pool).await;
    let (laptop, refresh) = sign_in_with_refresh(&app, "alice", Uuid::now_v7()).await;
    let phone = sign_in_from(&app, "alice", Uuid::now_v7()).await;
    let addr = serve(&app).await;
    let mut laptop_ws = connect(addr, &laptop).await;
    let refresh_with = |token: String| {
        let app = &app;
        async move {
            app.json(
                Method::POST,
                "/api/v1/auth/refresh",
                None,
                json!({ "refresh_token": token }),
            )
            .await
            .status
        }
    };

    assert_eq!(refresh_with(refresh.clone()).await, StatusCode::OK);
    assert_eq!(refresh_with(refresh).await, StatusCode::UNAUTHORIZED);
    app.upload(&phone, "name=private.txt", "secret").await;

    closes_for_ended_session(&mut laptop_ws).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_the_password_closes_every_socket_of_the_user(pool: PgPool) {
    let app = setup(pool).await;
    let device = Uuid::now_v7();
    let token = sign_in_from(&app, "alice", device).await;
    let addr = serve(&app).await;
    let mut socket = connect(addr, &token).await;
    let user = app
        .state
        .db
        .find_user_by_username("alice")
        .await
        .expect("db")
        .expect("alice");

    app.state
        .auth
        .set_password(user.id, "another-password")
        .await
        .expect("set password");

    closes_for_ended_session(&mut socket).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_socket_opened_with_a_ticket_closes_when_its_session_ends(pool: PgPool) {
    let app = setup(pool).await;
    let (token, refresh) = sign_in_with_refresh(&app, "alice", Uuid::now_v7()).await;
    let phone = sign_in_from(&app, "alice", Uuid::now_v7()).await;
    let addr = serve(&app).await;
    let ticket = app
        .json(
            Method::POST,
            "/api/v1/tickets",
            Some(&token),
            json!({ "scope": "events" }),
        )
        .await
        .json()["ticket"]
        .as_str()
        .expect("ticket")
        .to_owned();
    let request = format!("ws://{addr}/api/v1/ws?ticket={ticket}")
        .into_client_request()
        .expect("request");
    let mut socket = connect_async(request).await.expect("connect").0;

    logout(&app, &refresh).await;
    app.upload(&phone, "name=private.txt", "secret").await;

    closes_for_ended_session(&mut socket).await;
}
