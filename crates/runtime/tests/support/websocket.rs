//! A tiny local WebSocket server (RFC 6455, server side) for adapter tests. No network.
//!
//! Written independently of the runtime's client codec, so a bug the two would share cannot
//! hide. Each connection goes to the test's handler as a [`Pending`] request, which the handler
//! upgrades with [`Pending::accept`] or answers as plain HTTP with [`Pending::respond`] (an
//! HTTP fallback can be served by the same server).

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::RecordedRequest;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub struct WsServer {
    pub base_url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: JoinHandle<()>,
}

impl WsServer {
    /// Serves every connection with `handler`, each on its own task.
    pub async fn start<F, Fut>(handler: F) -> Self
    where
        F: Fn(Pending) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let handler = Arc::new(handler);
        let counter = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let handler = Arc::clone(&handler);
                let recorded = Arc::clone(&recorded);
                let index = counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let Some((request, rest)) = read_head(&mut stream).await else {
                        return;
                    };
                    recorded.lock().expect("lock").push(request.clone());
                    handler(Pending {
                        request,
                        stream,
                        rest,
                        index,
                    })
                    .await;
                });
            }
        });
        Self {
            base_url: format!("http://{address}"),
            requests,
            task,
        }
    }

    /// Every request so far, upgrades and plain HTTP alike, in arrival order.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock").clone()
    }

    pub fn upgrades(&self) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|request| request.header("upgrade").is_some())
            .collect()
    }
}

impl Drop for WsServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A request waiting for its answer.
pub struct Pending {
    pub request: RecordedRequest,
    /// Which connection this is, from 0.
    pub index: usize,
    stream: TcpStream,
    rest: Vec<u8>,
}

impl Pending {
    pub fn is_upgrade(&self) -> bool {
        self.request.header("upgrade").is_some()
    }

    pub async fn accept(self) -> ServerSocket {
        let key = self
            .request
            .header("sec-websocket-key")
            .expect("a key")
            .to_owned();
        let accept = accept_for(&key);
        self.accept_with(&accept).await
    }

    /// Switches protocols with an accept key that does not answer the client's.
    pub async fn accept_wrongly(self) -> ServerSocket {
        self.accept_with(&accept_for("another key")).await
    }

    async fn accept_with(mut self, accept: &str) -> ServerSocket {
        let head = format!(
            "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
             sec-websocket-accept: {accept}\r\n\r\n"
        );
        self.stream.write_all(head.as_bytes()).await.expect("write");
        ServerSocket {
            stream: self.stream,
            buffer: self.rest,
        }
    }

    /// Answers with a plain HTTP response and closes the connection.
    pub async fn respond(mut self, status: u16, content_type: &str, body: &[u8]) {
        let head = format!(
            "HTTP/1.1 {status} Scripted\r\ncontent-type: {content_type}\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = self.stream.write_all(head.as_bytes()).await;
        let _ = self.stream.write_all(body).await;
        let _ = self.stream.shutdown().await;
    }
}

/// The server's end of an upgraded connection.
pub struct ServerSocket {
    stream: TcpStream,
    buffer: Vec<u8>,
}

impl ServerSocket {
    /// The next text message from the client; `None` once it closes or goes away. Pings and
    /// pongs are skipped; a close is answered.
    pub async fn recv(&mut self) -> Option<String> {
        loop {
            let (opcode, payload) = self.frame().await?;
            match opcode {
                0x1 => return Some(String::from_utf8(payload).expect("client text is UTF-8")),
                0x8 => {
                    let _ = self.write_frame(0x8, &payload).await;
                    return None;
                }
                _ => {}
            }
        }
    }

    /// Waits for the client's pong and returns its payload; `None` if the client goes away.
    pub async fn recv_pong(&mut self) -> Option<Vec<u8>> {
        loop {
            let (opcode, payload) = self.frame().await?;
            if opcode == 0xA {
                return Some(payload);
            }
        }
    }

    pub async fn send_text(&mut self, text: &str) {
        let _ = self.write_frame(0x1, text.as_bytes()).await;
    }

    pub async fn send_ping(&mut self, payload: &[u8]) {
        let _ = self.write_frame(0x9, payload).await;
    }

    pub async fn send_close(&mut self, code: u16, reason: &str) {
        let payload = [&code.to_be_bytes()[..], reason.as_bytes()].concat();
        let _ = self.write_frame(0x8, &payload).await;
    }

    /// Raw bytes, to break the protocol on purpose.
    pub async fn send_raw(&mut self, bytes: &[u8]) {
        let _ = self.stream.write_all(bytes).await;
    }

    /// Drops the TCP connection without a close frame.
    pub fn drop_connection(self) {}

    /// Reads one client frame; every client frame must be masked.
    async fn frame(&mut self) -> Option<(u8, Vec<u8>)> {
        let head = self.take(2).await?;
        let opcode = head[0] & 0x0F;
        assert!(head[1] & 0x80 != 0, "client frames are masked");
        let length = match head[1] & 0x7F {
            126 => u64::from(u16::from_be_bytes(self.take(2).await?.try_into().ok()?)),
            127 => u64::from_be_bytes(self.take(8).await?.try_into().ok()?),
            short => u64::from(short),
        };
        let mask = self.take(4).await?;
        let payload = self.take(usize::try_from(length).ok()?).await?;
        let unmasked = payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4])
            .collect();
        Some((opcode, unmasked))
    }

    async fn take(&mut self, count: usize) -> Option<Vec<u8>> {
        let mut chunk = [0_u8; 4096];
        while self.buffer.len() < count {
            let read = self.stream.read(&mut chunk).await.ok()?;
            if read == 0 {
                return None;
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
        Some(self.buffer.drain(..count).collect())
    }

    async fn write_frame(&mut self, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
        let mut frame = vec![0x80 | opcode];
        if payload.len() < 126 {
            frame.push(payload.len() as u8);
        } else if payload.len() <= usize::from(u16::MAX) {
            frame.push(126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).await
    }
}

fn accept_for(key: &str) -> String {
    let digest = ring::digest::digest(
        &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        format!("{key}{GUID}").as_bytes(),
    );
    base64(digest.as_ref())
}

pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for group in bytes.chunks(3) {
        let mut padded = [0_u8; 3];
        padded[..group.len()].copy_from_slice(group);
        let bits = u32::from_be_bytes([0, padded[0], padded[1], padded[2]]);
        for index in 0..4 {
            if index <= group.len() {
                out.push(TABLE[((bits >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Reads the request head and any body bytes already received after it.
async fn read_head(stream: &mut TcpStream) -> Option<(RecordedRequest, Vec<u8>)> {
    let mut data = Vec::new();
    let mut chunk = [0_u8; 4096];
    let end = loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..end]).to_string();
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
    let mut rest = data[end..].to_vec();
    let is_upgrade = headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("upgrade"));
    while !is_upgrade && rest.len() < length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        rest.extend_from_slice(&chunk[..read]);
    }
    let (body, rest) = if is_upgrade {
        (Vec::new(), rest)
    } else {
        (rest, Vec::new())
    };
    Some((
        RecordedRequest {
            path,
            headers,
            body,
        },
        rest,
    ))
}
