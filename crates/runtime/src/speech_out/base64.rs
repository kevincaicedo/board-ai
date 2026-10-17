//! Standard base64 (RFC 4648 §4), both ways: ElevenLabs sends audio in it, and a WebSocket
//! handshake key is written in it. Two short functions instead of a dependency (law 9).

const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with optional padding; `None` for anything else.
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let trimmed = text.trim_end_matches('=');
    if text.len() - trimmed.len() > 2 || trimmed.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    for group in trimmed.as_bytes().chunks(4) {
        let mut bits: u32 = 0;
        for byte in group {
            bits = (bits << 6) | value(*byte)?;
        }
        bits <<= 6 * (4 - group.len() as u32);
        let bytes = bits.to_be_bytes();
        out.extend_from_slice(&bytes[1..group.len()]);
    }
    Some(out)
}

/// Standard base64 with padding.
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let mut padded = [0_u8; 3];
        padded[..group.len()].copy_from_slice(group);
        let bits = u32::from_be_bytes([0, padded[0], padded[1], padded[2]]);
        for index in 0..4 {
            if index <= group.len() {
                let sextet = (bits >> (18 - 6 * index)) & 63;
                out.push(char::from(TABLE[sextet as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn decodes_with_and_without_padding() {
        assert_eq!(decode("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(decode("aGVsbG8"), Some(b"hello".to_vec()));
        assert_eq!(decode("AAEC/w=="), Some(vec![0, 1, 2, 255]));
        assert_eq!(decode(""), Some(Vec::new()));
        assert_eq!(decode("a"), None);
        assert_eq!(decode("ab!c"), None);
        assert_eq!(decode("ab==="), None);
    }

    #[test]
    fn encodes_the_rfc_vectors_and_round_trips() {
        let vectors = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in vectors {
            assert_eq!(encode(plain.as_bytes()), encoded);
            assert_eq!(decode(encoded), Some(plain.as_bytes().to_vec()));
        }
        let every_byte: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&every_byte)), Some(every_byte));
    }
}
