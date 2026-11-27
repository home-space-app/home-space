//! `home-space://` links (`client_core::app_link`): the web client hands a folder over to the
//! app with one, for what only the app can open. The app registers the scheme with the system
//! (the `.desktop` file on Linux, the bundle on macOS, the user's registry on Windows), takes a
//! link from its arguments, and a second start with a link hands it to the app already running.
//!
//! The running app listens on a loopback port. The port and a random token are in a file of the
//! app's data folder, readable only by the user; a link comes in only with the token.

use std::{
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};

use iced::{
    Subscription,
    futures::{SinkExt, Stream, channel::mpsc},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

pub use client_core::app_link::AppLink;

use crate::session;

const HANDOFF_FILE: &str = "app-link";
const TOKEN_LEN: usize = 16;
/// How long a second start waits for the running app.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest line a hand-over may send: the token and a link.
const MAX_LINE: u64 = 4096;
const TAKEN: &str = "ok";

/// The link the app was started with, if any.
pub fn from_args() -> Option<AppLink> {
    std::env::args()
        .skip(1)
        .find_map(|arg| AppLink::parse(&arg))
}

/// Hands `link` to the app already running; whether it took it.
pub fn hand_over(link: &AppLink) -> bool {
    let Some((port, token)) = read_handoff() else {
        return false;
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, HANDOFF_TIMEOUT) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(HANDOFF_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDOFF_TIMEOUT));
    if writeln!(stream, "{token} {}", link.to_url()).is_err() {
        return false;
    }
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).is_ok() && answer.trim() == TAKEN
}

/// The links later starts hand over, while the app runs.
pub fn listen() -> Subscription<AppLink> {
    Subscription::run(listening)
}

fn listening() -> impl Stream<Item = AppLink> {
    iced::stream::channel(8, async move |mut output: mpsc::Sender<AppLink>| {
        let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await else {
            return;
        };
        let Ok(address) = listener.local_addr() else {
            return;
        };
        let mut bytes = [0u8; TOKEN_LEN];
        rand::fill(&mut bytes);
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        if write_handoff(address.port(), &token).is_err() {
            return;
        }
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let read = tokio::time::timeout(HANDOFF_TIMEOUT, take(stream, &token)).await;
            if let Ok(Some(link)) = read
                && output.send(link).await.is_err()
            {
                return;
            }
        }
    })
}

/// Reads a hand-over from `stream`: the token, then the link, which it confirms.
async fn take(stream: tokio::net::TcpStream, token: &str) -> Option<AppLink> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    tokio::io::BufReader::new(read.take(MAX_LINE))
        .read_line(&mut line)
        .await
        .ok()?;
    let (sent, link) = line.trim().split_once(' ')?;
    if sent != token {
        return None;
    }
    let link = AppLink::parse(link)?;
    write
        .write_all(format!("{TAKEN}\n").as_bytes())
        .await
        .ok()?;
    Some(link)
}

fn handoff_path() -> PathBuf {
    session::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(HANDOFF_FILE)
}

fn read_handoff() -> Option<(u16, String)> {
    let text = std::fs::read_to_string(handoff_path()).ok()?;
    let (port, token) = text.trim().split_once(' ')?;
    Some((port.parse().ok()?, token.to_owned()))
}

fn write_handoff(port: u16, token: &str) -> std::io::Result<()> {
    let path = handoff_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = private_file(&path)?;
    writeln!(file, "{port} {token}")
}

#[cfg(unix)]
fn private_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn private_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::File::create(path)
}

/// Registers the scheme for this user, pointing at this executable. Linux and macOS take it from
/// the `.desktop` file and the bundle instead.
#[cfg(windows)]
pub fn register() {
    use std::process::Command;

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let key = format!("HKCU\\Software\\Classes\\{}", client_core::app_link::SCHEME);
    let command = format!("\"{}\" \"%1\"", exe.display());
    let entries: [(&str, Option<&str>, &str); 3] = [
        ("", None, "URL:Home Space"),
        ("", Some("URL Protocol"), ""),
        ("\\shell\\open\\command", None, &command),
    ];
    for (sub, name, value) in entries {
        let mut reg = Command::new("reg");
        reg.args(["add", &format!("{key}{sub}"), "/f", "/d", value]);
        match name {
            Some(name) => reg.args(["/v", name]),
            None => reg.arg("/ve"),
        };
        let _ = reg.output();
    }
}

#[cfg(not(windows))]
pub fn register() {}

#[cfg(test)]
mod tests {
    use protocol::NodeId;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn a_hand_over_is_taken_only_with_the_token() {
        let link =
            AppLink::folder("https://files.example.com", NodeId(Uuid::from_u128(5))).expect("link");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bound");
        let port = listener.local_addr().expect("address").port();
        let sent = link.to_url();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connected");
            writeln!(stream, "secret {sent}").expect("sent");
            let mut answer = String::new();
            BufReader::new(stream)
                .read_line(&mut answer)
                .expect("answer");
            answer
        });
        let (stream, _) = listener.accept().await.expect("accepted");
        assert_eq!(take(stream, "secret").await, Some(link.clone()));
        assert_eq!(client.join().expect("joined").trim(), TAKEN);

        let wrong = link.to_url();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connected");
            writeln!(stream, "guess {wrong}").expect("sent");
        });
        let (stream, _) = listener.accept().await.expect("accepted");
        assert_eq!(take(stream, "secret").await, None);
        client.join().expect("joined");
    }
}
