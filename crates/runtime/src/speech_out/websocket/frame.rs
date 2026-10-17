//! WebSocket frames (RFC 6455 §5), client side: what a server sends is parsed into messages,
//! and what we send is framed and masked.
//!
//! The parser is a bounded state machine over a byte buffer, so a message split anywhere across
//! reads parses the same (tested). Every rule a client must enforce is enforced: server frames
//! are never masked, reserved bits are zero (no extension was negotiated), control frames are
//! short and unfragmented, continuation frames follow a started message, text is UTF-8, and no
//! frame or message exceeds the limit given at construction.

use std::fmt;

/// Frame opcodes (§5.2). Anything else is a protocol error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl Opcode {
    fn parse(bits: u8) -> Option<Self> {
        match bits {
            0x0 => Some(Self::Continuation),
            0x1 => Some(Self::Text),
            0x2 => Some(Self::Binary),
            0x8 => Some(Self::Close),
            0x9 => Some(Self::Ping),
            0xA => Some(Self::Pong),
            _ => None,
        }
    }

    const fn bits(self) -> u8 {
        match self {
            Self::Continuation => 0x0,
            Self::Text => 0x1,
            Self::Binary => 0x2,
            Self::Close => 0x8,
            Self::Ping => 0x9,
            Self::Pong => 0xA,
        }
    }

    const fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

/// A whole message from the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Incoming {
    Text(String),
    Binary(Vec<u8>),
    /// Must be answered with a pong carrying the same payload.
    Ping(Vec<u8>),
    Pong,
    /// The server is closing; `code` is absent when the frame carried none.
    Close {
        code: Option<u16>,
        reason: String,
    },
}

/// The server broke the protocol; the connection must be dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProtocolError(pub(crate) &'static str);

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "WebSocket protocol error: {}", self.0)
    }
}

/// Control frames carry at most this much (§5.5).
const CONTROL_PAYLOAD_MAX_BYTES: usize = 125;

/// Bytes received but not yet parsed, and the message being reassembled from fragments.
pub(crate) struct Parser {
    buffer: Vec<u8>,
    message_max_bytes: usize,
    /// A fragmented data message in progress: its opcode and the payload so far.
    partial: Option<(Opcode, Vec<u8>)>,
}

/// One frame's header, once enough bytes have arrived to read it.
struct Header {
    fin: bool,
    opcode: Opcode,
    header_bytes: usize,
    payload_bytes: usize,
}

impl Parser {
    /// `message_max_bytes` bounds a frame's payload and a reassembled message alike.
    pub(crate) fn new(message_max_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            message_max_bytes,
            partial: None,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next complete message, `None` until more bytes arrive. Each pass consumes one frame,
    /// so the loop ends with the buffer.
    pub(crate) fn next_message(&mut self) -> Result<Option<Incoming>, ProtocolError> {
        loop {
            let Some(header) = self.header()? else {
                return Ok(None);
            };
            let end = header.header_bytes + header.payload_bytes;
            if self.buffer.len() < end {
                return Ok(None);
            }
            let payload = self.buffer[header.header_bytes..end].to_vec();
            self.buffer.drain(..end);
            if let Some(message) = self.frame(header, payload)? {
                return Ok(Some(message));
            }
        }
    }

    fn header(&self) -> Result<Option<Header>, ProtocolError> {
        let [first, second, ..] = self.buffer[..] else {
            return Ok(None);
        };
        if first & 0x70 != 0 {
            return Err(ProtocolError("a reserved bit is set"));
        }
        let opcode = Opcode::parse(first & 0x0F).ok_or(ProtocolError("an unknown opcode"))?;
        if second & 0x80 != 0 {
            return Err(ProtocolError("the server masked a frame"));
        }
        let (length_bytes, payload_bytes) = match second & 0x7F {
            126 => (2, None),
            127 => (8, None),
            short => (0, Some(u64::from(short))),
        };
        let header_bytes = 2 + length_bytes;
        let payload_bytes = match payload_bytes {
            Some(length) => length,
            None => {
                let Some(field) = self.buffer.get(2..header_bytes) else {
                    return Ok(None);
                };
                field
                    .iter()
                    .fold(0_u64, |length, byte| (length << 8) | u64::from(*byte))
            }
        };
        let fin = first & 0x80 != 0;
        if opcode.is_control() && (!fin || payload_bytes > CONTROL_PAYLOAD_MAX_BYTES as u64) {
            return Err(ProtocolError("a control frame is fragmented or too long"));
        }
        let payload_bytes = usize::try_from(payload_bytes)
            .ok()
            .filter(|length| *length <= self.message_max_bytes)
            .ok_or(ProtocolError("a frame is larger than its limit"))?;
        Ok(Some(Header {
            fin,
            opcode,
            header_bytes,
            payload_bytes,
        }))
    }

    /// Applies one frame; a message when it completes one.
    fn frame(
        &mut self,
        header: Header,
        payload: Vec<u8>,
    ) -> Result<Option<Incoming>, ProtocolError> {
        match header.opcode {
            Opcode::Ping => Ok(Some(Incoming::Ping(payload))),
            Opcode::Pong => Ok(Some(Incoming::Pong)),
            Opcode::Close => close(&payload).map(Some),
            Opcode::Text | Opcode::Binary => {
                if self.partial.is_some() {
                    return Err(ProtocolError(
                        "a new message started inside a fragmented one",
                    ));
                }
                if header.fin {
                    return message(header.opcode, payload).map(Some);
                }
                self.partial = Some((header.opcode, payload));
                Ok(None)
            }
            Opcode::Continuation => {
                let Some((opcode, mut so_far)) = self.partial.take() else {
                    return Err(ProtocolError("a continuation frame with no message"));
                };
                if so_far.len() + payload.len() > self.message_max_bytes {
                    return Err(ProtocolError("a message is larger than its limit"));
                }
                so_far.extend_from_slice(&payload);
                if header.fin {
                    return message(opcode, so_far).map(Some);
                }
                self.partial = Some((opcode, so_far));
                Ok(None)
            }
        }
    }
}

fn message(opcode: Opcode, payload: Vec<u8>) -> Result<Incoming, ProtocolError> {
    match opcode {
        Opcode::Text => String::from_utf8(payload)
            .map(Incoming::Text)
            .map_err(|_| ProtocolError("a text message is not UTF-8")),
        Opcode::Binary => Ok(Incoming::Binary(payload)),
        Opcode::Continuation | Opcode::Close | Opcode::Ping | Opcode::Pong => {
            Err(ProtocolError("a control frame cannot start a message"))
        }
    }
}

fn close(payload: &[u8]) -> Result<Incoming, ProtocolError> {
    match payload {
        [] => Ok(Incoming::Close {
            code: None,
            reason: String::new(),
        }),
        [_] => Err(ProtocolError("a close frame with half a status code")),
        [high, low, reason @ ..] => {
            let reason = std::str::from_utf8(reason)
                .map_err(|_| ProtocolError("a close reason is not UTF-8"))?;
            Ok(Incoming::Close {
                code: Some(u16::from_be_bytes([*high, *low])),
                reason: reason.to_owned(),
            })
        }
    }
}

/// One unfragmented client frame, masked with `mask` (§5.3: every client frame is masked).
pub(crate) fn encode(opcode: Opcode, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode.bits());
    let length = payload.len();
    if length < 126 {
        frame.push(0x80 | length as u8);
    } else if let Ok(short) = u16::try_from(length) {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&short.to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(length as u64).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .zip(mask.iter().cycle())
            .map(|(byte, key)| byte ^ key),
    );
    frame
}

/// A close frame's payload: the status code, then a reason of at most what fits.
pub(crate) fn close_payload(code: u16, reason: &str) -> Vec<u8> {
    let mut payload = code.to_be_bytes().to_vec();
    let room = CONTROL_PAYLOAD_MAX_BYTES - payload.len();
    let mut end = reason.len().min(room);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    payload.extend_from_slice(&reason.as_bytes()[..end]);
    payload
}

#[cfg(test)]
mod tests {
    use super::{Incoming, Opcode, Parser, ProtocolError, close_payload, encode};

    /// A server frame (unmasked), for the parser.
    fn server_frame(first: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![first];
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
        frame
    }

    fn parse_all(bytes: &[u8], limit: usize) -> Result<Vec<Incoming>, ProtocolError> {
        let mut parser = Parser::new(limit);
        parser.push(bytes);
        let mut messages = Vec::new();
        while let Some(message) = parser.next_message()? {
            messages.push(message);
        }
        Ok(messages)
    }

    #[test]
    fn the_rfc_examples_parse_and_encode() {
        // RFC 6455 §5.7: a single-frame unmasked text message, and the masked one.
        let unmasked = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        assert_eq!(
            parse_all(&unmasked, 1024),
            Ok(vec![Incoming::Text("Hello".into())])
        );
        let masked = [
            0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        assert_eq!(
            encode(Opcode::Text, b"Hello", [0x37, 0xfa, 0x21, 0x3d]),
            masked
        );
        // A fragmented text message, then a ping in the middle of another.
        let fragmented = [
            0x01, 0x03, 0x48, 0x65, 0x6c, 0x80, 0x02, 0x6c, 0x6f, 0x89, 0x05, 0x48, 0x65, 0x6c,
            0x6c, 0x6f,
        ];
        assert_eq!(
            parse_all(&fragmented, 1024),
            Ok(vec![
                Incoming::Text("Hello".into()),
                Incoming::Ping(b"Hello".to_vec())
            ])
        );
    }

    #[test]
    fn messages_split_anywhere_parse_the_same() {
        let long = "x".repeat(70_000);
        let bytes = [
            server_frame(0x81, b"{\"audio\":\"AAAA\"}"),
            server_frame(0x01, b"frag"),
            server_frame(0x89, b"ping"),
            server_frame(0x80, b"mented"),
            server_frame(0x81, long.as_bytes()),
            server_frame(0x81, &"é".repeat(100).into_bytes()),
            server_frame(0x88, &close_payload(1008, "quota")),
        ]
        .concat();
        let whole = parse_all(&bytes, 100_000).expect("parses");
        assert_eq!(whole.len(), 6);
        assert_eq!(whole[2], Incoming::Text("fragmented".into()));
        assert_eq!(
            whole[5],
            Incoming::Close {
                code: Some(1008),
                reason: "quota".into()
            }
        );
        for split in (1..bytes.len()).step_by(997).chain(1..40) {
            let mut parser = Parser::new(100_000);
            let mut messages = Vec::new();
            for part in [&bytes[..split], &bytes[split..]] {
                parser.push(part);
                while let Some(message) = parser.next_message().expect("parses") {
                    messages.push(message);
                }
            }
            assert_eq!(messages, whole, "split at {split}");
        }
    }

    #[test]
    fn protocol_violations_are_errors() {
        let cases: [(&[u8], &str); 8] = [
            (&[0x81, 0x81, 1, 2, 3, 4, 0x41], "the server masked a frame"),
            (&[0xC1, 0x00], "a reserved bit is set"),
            (&[0x83, 0x00], "an unknown opcode"),
            (&[0x09, 0x00], "a control frame is fragmented or too long"),
            (&[0x80, 0x00], "a continuation frame with no message"),
            (&[0x81, 0x02, 0xff, 0xfe], "a text message is not UTF-8"),
            (&[0x88, 0x01, 0x03], "a close frame with half a status code"),
            (
                &[0x01, 0x01, 0x41, 0x81, 0x01, 0x41],
                "a new message started inside a fragmented one",
            ),
        ];
        for (bytes, expected) in cases {
            assert_eq!(parse_all(bytes, 1024), Err(ProtocolError(expected)));
        }
    }

    #[test]
    fn frames_and_messages_past_the_limit_are_refused_before_buffering() {
        let header_only = [0x81, 127, 0, 0, 0, 1, 0, 0, 0, 0];
        assert_eq!(
            parse_all(&header_only, 1024),
            Err(ProtocolError("a frame is larger than its limit"))
        );
        let pieces = [
            server_frame(0x01, &[b'a'; 600]),
            server_frame(0x80, &[b'a'; 600]),
        ]
        .concat();
        assert_eq!(
            parse_all(&pieces, 1024),
            Err(ProtocolError("a message is larger than its limit"))
        );
    }

    #[test]
    fn encoded_lengths_use_the_three_forms_and_close_reasons_fit() {
        let mask = [1, 2, 3, 4];
        assert_eq!(encode(Opcode::Text, &[0; 125], mask)[1], 0x80 | 125);
        let medium = encode(Opcode::Text, &[0; 126], mask);
        assert_eq!(&medium[1..4], &[0x80 | 126, 0, 126]);
        let large = encode(Opcode::Binary, &[0; 70_000], mask);
        assert_eq!(large[1], 0x80 | 127);
        assert_eq!(large.len(), 2 + 8 + 4 + 70_000);
        let payload = close_payload(1000, &"é".repeat(100));
        assert!(payload.len() <= 125);
        assert!(std::str::from_utf8(&payload[2..]).is_ok());
    }
}
