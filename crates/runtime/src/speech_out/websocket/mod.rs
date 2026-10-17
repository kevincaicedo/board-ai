//! A WebSocket client (RFC 6455) for the ElevenLabs realtime voice, over reqwest's HTTP/1.1
//! upgrade: the same TLS stack, proxy and crypto provider as every other provider call, and no
//! new crate (law 9). It speaks only what that needs: text messages out, text messages in,
//! pings answered by the owner, and the close handshake. Extensions and subprotocols are never
//! offered, so a server may not use them.
//!
//! [`WebSocket::next`] is cancel-safe: everything read is kept in the parser before the next
//! await, so it can sit in a `select!` next to other work. Writes are not, so the owner writes
//! only outside a `select!`.

mod frame;

use std::fmt;

use reqwest::StatusCode;
use reqwest::header::{
    CONNECTION, HeaderMap, HeaderValue, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY,
    SEC_WEBSOCKET_VERSION, UPGRADE,
};
use ring::rand::{SecureRandom, SystemRandom};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(crate) use frame::Incoming;
use frame::{Opcode, Parser, ProtocolError, close_payload, encode};

use super::base64;
use crate::llm::ProviderError;
use crate::llm::bounded_detail;
use crate::llm::http::error_from_response;

/// Appended to the key before hashing (RFC 6455 §4.2.2).
const ACCEPT_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Bytes asked of the socket per read.
const READ_CHUNK_BYTES: usize = 16 * 1024;

/// Close codes we send (§7.4.1).
pub(crate) const CLOSE_NORMAL: u16 = 1000;
pub(crate) const CLOSE_PROTOCOL_ERROR: u16 = 1002;

/// Why the connection can no longer be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WebSocketError {
    /// The socket failed or ended without a close frame.
    Io(String),
    /// The server broke the protocol or sent more than the limit.
    Protocol(&'static str),
}

impl fmt::Display for WebSocketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(detail) => write!(formatter, "WebSocket connection failed: {detail}"),
            Self::Protocol(detail) => write!(formatter, "WebSocket protocol error: {detail}"),
        }
    }
}

impl From<ProtocolError> for WebSocketError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error.0)
    }
}

/// One open connection.
pub(crate) struct WebSocket<S> {
    io: S,
    parser: Parser,
    read_buffer: Box<[u8]>,
    random: SystemRandom,
    close_sent: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin> WebSocket<S> {
    /// Wraps an upgraded connection. `message_max_bytes` bounds every frame and message.
    pub(crate) fn new(io: S, message_max_bytes: usize) -> Self {
        Self {
            io,
            parser: Parser::new(message_max_bytes),
            read_buffer: vec![0; READ_CHUNK_BYTES].into_boxed_slice(),
            random: SystemRandom::new(),
            close_sent: false,
        }
    }

    /// The next message. Cancel-safe (see the module comment).
    pub(crate) async fn next(&mut self) -> Result<Incoming, WebSocketError> {
        loop {
            if let Some(message) = self.parser.next_message()? {
                return Ok(message);
            }
            let read = self
                .io
                .read(&mut self.read_buffer)
                .await
                .map_err(|error| WebSocketError::Io(bounded_detail(&error.to_string())))?;
            if read == 0 {
                return Err(WebSocketError::Io(
                    "the server ended the connection without closing it".to_owned(),
                ));
            }
            self.parser.push(&self.read_buffer[..read]);
        }
    }

    pub(crate) async fn send_text(&mut self, text: &str) -> Result<(), WebSocketError> {
        self.send(Opcode::Text, text.as_bytes()).await
    }

    pub(crate) async fn send_pong(&mut self, payload: &[u8]) -> Result<(), WebSocketError> {
        self.send(Opcode::Pong, payload).await
    }

    /// Starts (or answers) the close handshake; sent at most once.
    pub(crate) async fn send_close(&mut self, code: u16) -> Result<(), WebSocketError> {
        if self.close_sent {
            return Ok(());
        }
        self.close_sent = true;
        self.send(Opcode::Close, &close_payload(code, "")).await
    }

    async fn send(&mut self, opcode: Opcode, payload: &[u8]) -> Result<(), WebSocketError> {
        let mut mask = [0_u8; 4];
        // §5.3 asks for an unpredictable mask; the system's generator does not fail on macOS.
        self.random
            .fill(&mut mask)
            .map_err(|_| WebSocketError::Io("no random bytes for the frame mask".to_owned()))?;
        let frame = encode(opcode, payload, mask);
        let io_error =
            |error: std::io::Error| WebSocketError::Io(bounded_detail(&error.to_string()));
        self.io.write_all(&frame).await.map_err(io_error)?;
        self.io.flush().await.map_err(io_error)
    }
}

/// Opens a WebSocket at `url` (`http` or `https`; reqwest upgrades it) with `headers` on the
/// handshake. A refused handshake keeps the provider's category (a 401 is `Auth`). The caller
/// bounds the time.
pub(crate) async fn connect(
    client: &reqwest::Client,
    url: &str,
    headers: HeaderMap,
    message_max_bytes: usize,
) -> Result<WebSocket<reqwest::Upgraded>, ProviderError> {
    let random = SystemRandom::new();
    let mut nonce = [0_u8; 16];
    random
        .fill(&mut nonce)
        .map_err(|_| ProviderError::Network {
            detail: "no random bytes for the WebSocket key".to_owned(),
        })?;
    let key = base64::encode(&nonce);
    let response = client
        .get(url)
        .headers(headers)
        .header(CONNECTION, HeaderValue::from_static("Upgrade"))
        .header(UPGRADE, HeaderValue::from_static("websocket"))
        .header(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"))
        .header(SEC_WEBSOCKET_KEY, key.as_str())
        .send()
        .await
        .map_err(|error| ProviderError::Network {
            detail: bounded_detail(&error.to_string()),
        })?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        if response.status().is_success() {
            return Err(ProviderError::BadOutput {
                detail: "the server answered without switching to WebSocket".to_owned(),
            });
        }
        return Err(error_from_response(response).await);
    }
    let accepted = response
        .headers()
        .get(SEC_WEBSOCKET_ACCEPT)
        .and_then(|value| value.to_str().ok());
    if accepted != Some(accept_for(&key).as_str()) {
        return Err(ProviderError::BadOutput {
            detail: "the server's WebSocket accept key is wrong".to_owned(),
        });
    }
    let upgraded = response
        .upgrade()
        .await
        .map_err(|error| ProviderError::Network {
            detail: bounded_detail(&error.to_string()),
        })?;
    Ok(WebSocket::new(upgraded, message_max_bytes))
}

/// The `Sec-WebSocket-Accept` a server must answer `key` with (§4.2.2).
pub(crate) fn accept_for(key: &str) -> String {
    let mut context = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    context.update(key.as_bytes());
    context.update(ACCEPT_GUID.as_bytes());
    base64::encode(context.finish().as_ref())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    use super::{CLOSE_NORMAL, Incoming, WebSocket, WebSocketError, accept_for};

    #[test]
    fn the_accept_key_matches_the_rfc_example() {
        assert_eq!(
            accept_for("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[tokio::test]
    async fn sends_masked_text_and_reads_split_server_messages() {
        let (client, mut server) = duplex(1024);
        let mut socket = WebSocket::new(client, 1024);
        socket.send_text("Hi").await.expect("sent");
        let mut sent = [0_u8; 8];
        server.read_exact(&mut sent).await.expect("read");
        assert_eq!(sent[0], 0x81);
        assert_eq!(sent[1], 0x80 | 2, "masked, two bytes");
        let unmasked: Vec<u8> = sent[6..]
            .iter()
            .zip(&sent[2..6])
            .map(|(byte, key)| byte ^ key)
            .collect();
        assert_eq!(unmasked, b"Hi");
        server.write_all(&[0x81, 0x03, b'a']).await.expect("write");
        let reader = tokio::spawn(async move {
            let message = socket.next().await;
            (socket, message)
        });
        tokio::task::yield_now().await;
        server.write_all(b"bc").await.expect("write");
        let (mut socket, message) = reader.await.expect("joined");
        assert_eq!(message, Ok(Incoming::Text("abc".into())));
        socket.send_close(CLOSE_NORMAL).await.expect("close");
        socket
            .send_close(CLOSE_NORMAL)
            .await
            .expect("closing twice is a no-op");
        drop(server);
        assert!(matches!(socket.next().await, Err(WebSocketError::Io(_))));
    }
}
