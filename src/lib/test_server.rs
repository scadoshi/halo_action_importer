//! A local HTTP server for tests: answers one request with a canned response and hands back what was asked.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// What the server was asked.
#[derive(Debug)]
pub(crate) struct Request {
    /// The request line, such as `POST /auth/token HTTP/1.1`.
    pub(crate) line: String,
}

/// Serves one JSON request on a free local port. Returns the base URL, with a
/// trailing slash, and a handle that resolves to the request.
pub(crate) async fn serve_once(status: u16, body: &'static str) -> (String, JoinHandle<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        answer(&mut socket, status, body).await
    });
    (base_url, handle)
}

/// How long a test waits for the server to see the request it expects.
const PATIENCE: Duration = Duration::from_secs(10);

/// Waits for the server behind `handle` to finish. Panics instead of hanging
/// when the request it expects never comes.
pub(crate) async fn served<T>(handle: JoinHandle<T>) -> T {
    tokio::time::timeout(PATIENCE, handle)
        .await
        .expect("the server did not get the request it expected")
        .unwrap()
}

/// Reads one request from `socket` and writes a JSON reply.
async fn answer(socket: &mut TcpStream, status: u16, body: &str) -> Request {
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    let body_start = loop {
        let n = socket.read(&mut buf).await.unwrap();
        assert!(n > 0, "the client closed the connection mid-request");
        request.extend_from_slice(&buf[..n]);
        if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&request[..body_start]).into_owned();
    let content_length: usize = head
        .to_lowercase()
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map_or(0, |len| len.trim().parse().unwrap());
    while request.len() < body_start + content_length {
        let n = socket.read(&mut buf).await.unwrap();
        assert!(n > 0, "the client closed the connection mid-body");
        request.extend_from_slice(&buf[..n]);
    }
    let response = format!(
        "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await.unwrap();
    Request {
        line: head.lines().next().unwrap_or_default().to_owned(),
    }
}
