//! Test support: a tiny local HTTP server and fixture helpers. No network, no keys.
#![allow(
    dead_code,
    reason = "each test binary uses a different part of the support module"
)]
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub mod websocket;

pub fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative)
}

pub fn fixture(relative: &str) -> Vec<u8> {
    std::fs::read(fixture_path(relative)).expect("fixture exists")
}

/// One scripted HTTP response.
#[derive(Clone, Debug)]
pub struct Scripted {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub chunks: Vec<Vec<u8>>,
    pub chunk_delay: Duration,
    /// After the chunks, keep the connection open until the client goes away.
    pub hang: bool,
}

impl Scripted {
    /// A 200 SSE response that delivers `body` in `pieces` roughly equal chunks, so the client
    /// sees events split at arbitrary points.
    pub fn sse(body: &[u8], pieces: usize) -> Self {
        let size = body.len().div_ceil(pieces.max(1)).max(1);
        Self {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: body.chunks(size).map(<[u8]>::to_vec).collect(),
            chunk_delay: Duration::ZERO,
            hang: false,
        }
    }

    pub fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![body.as_bytes().to_vec()],
            chunk_delay: Duration::ZERO,
            hang: false,
        }
    }

    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn hanging(mut self) -> Self {
        self.hang = true;
        self
    }

    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.chunk_delay = delay;
        self
    }
}

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }
}

/// Serves scripted responses in order, one per connection, and records each request. Requests
/// past the end of the script get a 500. The accept loop is aborted when the server is dropped.
pub struct TestServer {
    pub base_url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    disconnected: Arc<(AtomicBool, Notify)>,
    task: JoinHandle<()>,
}

impl TestServer {
    pub async fn start(script: Vec<Scripted>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let disconnected = Arc::new((AtomicBool::new(false), Notify::new()));
        let task = tokio::spawn(accept_loop(
            listener,
            script,
            Arc::clone(&requests),
            Arc::clone(&disconnected),
        ));
        Self {
            base_url: format!("http://{address}"),
            requests,
            disconnected,
            task,
        }
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock").clone()
    }

    /// Resolves when a hanging response notices that the client closed the connection.
    pub async fn client_disconnected(&self) {
        loop {
            let notified = self.disconnected.1.notified();
            if self.disconnected.0.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn accept_loop(
    listener: TcpListener,
    script: Vec<Scripted>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    disconnected: Arc<(AtomicBool, Notify)>,
) {
    let mut script = script.into_iter();
    while let Ok((socket, _)) = listener.accept().await {
        let response = script
            .next()
            .unwrap_or_else(|| Scripted::status(500, r#"{"error":{"message":"no more script"}}"#));
        let requests = Arc::clone(&requests);
        let disconnected = Arc::clone(&disconnected);
        // Each connection is served on its own task, so a hanging response never blocks the
        // next; all of them stop with the runtime at the end of the test.
        tokio::spawn(async move {
            serve(socket, response, requests, disconnected).await;
        });
    }
}

async fn serve(
    mut socket: TcpStream,
    response: Scripted,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    disconnected: Arc<(AtomicBool, Notify)>,
) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    requests.lock().expect("lock").push(request);
    let mut head = format!("HTTP/1.1 {} Scripted\r\n", response.status);
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("transfer-encoding: chunked\r\nconnection: close\r\n\r\n");
    if socket.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    for chunk in &response.chunks {
        tokio::time::sleep(response.chunk_delay).await;
        let framed = [format!("{:x}\r\n", chunk.len()).as_bytes(), chunk, b"\r\n"].concat();
        if socket.write_all(&framed).await.is_err() {
            mark(&disconnected);
            return;
        }
    }
    if response.hang {
        let mut buffer = [0_u8; 64];
        while matches!(socket.read(&mut buffer).await, Ok(read) if read > 0) {}
        mark(&disconnected);
        return;
    }
    let _ = socket.write_all(b"0\r\n\r\n").await;
    let _ = socket.shutdown().await;
}

fn mark(disconnected: &(AtomicBool, Notify)) {
    disconnected.0.store(true, Ordering::Release);
    disconnected.1.notify_waiters();
}

async fn read_request(socket: &mut TcpStream) -> Option<RecordedRequest> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..read]);
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let path = lines.next()?.split(' ').nth(1)?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = data[header_end..].to_vec();
    while body.len() < length {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some(RecordedRequest {
        path,
        headers,
        body,
    })
}

/// A unique, empty directory under the system temp dir, removed on drop.
pub struct TempFolder {
    pub path: PathBuf,
}

impl TempFolder {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "boardai-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    pub fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, contents).expect("write file");
        path
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
