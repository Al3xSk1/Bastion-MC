//! The client side of a WebSocket (RFC 6455), as much of it as talking to Firefox's remote
//! control needs: the opening handshake, masked frames out, unmasked frames in.
//!
//! What comes back was written by the browser, which renders pages from the internet: it is
//! read as hostile. The decoder buffers at most one message ([`MAX_MESSAGE`]), refuses
//! masked frames, oversized or fragmented control frames, reserved bits and opcodes, text
//! that is not UTF-8 and anything after a close.

use base64::Engine;
use sha1::{Digest, Sha1};

/// The largest message accepted (a screenshot, base64-encoded, is the largest legitimate one).
pub const MAX_MESSAGE: usize = 16 << 20;
/// The largest response head of the opening handshake.
pub const MAX_HANDSHAKE: usize = 8 * 1024;
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The request that opens the WebSocket at `path` on `host` (`127.0.0.1:9222`).
pub fn handshake_request(host: &str, path: &str, key: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: \
         {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
}

/// A fresh `Sec-WebSocket-Key`.
pub fn new_key() -> crate::Result<String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(crate::util::random_bytes::<16>()?))
}

fn accept_for(key: &str) -> String {
    let mut hash = Sha1::new();
    hash.update(key.as_bytes());
    hash.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hash.finalize())
}

/// Where the response head ends in `buf` (the index after `\r\n\r\n`), if it is complete.
pub fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Check the server's answer to [`handshake_request`]: `101`, the upgrade headers, and the
/// `Sec-WebSocket-Accept` that belongs to `key`.
pub fn check_handshake(head: &[u8], key: &str) -> Result<(), String> {
    let text = std::str::from_utf8(head).map_err(|_| "the handshake answer is not text".to_string())?;
    let mut lines = text.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let mut parts = status.splitn(3, ' ');
    if !matches!(parts.next(), Some("HTTP/1.1")) || parts.next() != Some("101") {
        return Err(format!(
            "the remote control refused the WebSocket: {:?}",
            crate::util::sanitize_line(status.as_bytes(), 80)
        ));
    }
    let (mut upgrade, mut connection, mut accept) = (false, false, None);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "upgrade" => upgrade = value.eq_ignore_ascii_case("websocket"),
            "connection" => connection = value.split(',').any(|v| v.trim().eq_ignore_ascii_case("upgrade")),
            "sec-websocket-accept" => accept = Some(value.to_string()),
            _ => {}
        }
    }
    if !upgrade || !connection {
        return Err("the handshake answer lacks the upgrade headers".into());
    }
    if accept.as_deref() != Some(accept_for(key).as_str()) {
        return Err("the handshake answer has the wrong Sec-WebSocket-Accept".into());
    }
    Ok(())
}

pub const OP_CONTINUATION: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

/// One frame from the client: FIN set, masked with `mask`.
pub fn encode(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | (opcode & 0x0f));
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= 0xffff => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// A text frame with a fresh mask.
pub fn text_frame(text: &str) -> crate::Result<Vec<u8>> {
    Ok(encode(OP_TEXT, text.as_bytes(), crate::util::random_bytes::<4>()?))
}

/// A complete message from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// The server is closing (status code, if one was sent).
    Close(Option<u16>),
}

/// Why the stream from the server was given up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Broken {
    Masked,
    ReservedBits,
    UnknownOpcode(u8),
    BadControlFrame,
    TooLarge,
    UnexpectedContinuation,
    InterleavedMessage,
    NotUtf8,
    AfterClose,
}

/// Turns the bytes from the server into [`Message`]s.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
    /// Opcode and payload of a fragmented message being assembled.
    partial: Option<(u8, Vec<u8>)>,
    closed: bool,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// Bytes waiting to be decoded (at most a frame header plus [`MAX_MESSAGE`]).
    pub fn buffered(&self) -> usize {
        self.buf.len() + self.partial.as_ref().map_or(0, |(_, p)| p.len())
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// The next complete message, `Ok(None)` if more bytes are needed.
    pub fn next_message(&mut self) -> Result<Option<Message>, Broken> {
        loop {
            if self.closed {
                return if self.buf.is_empty() { Ok(None) } else { Err(Broken::AfterClose) };
            }
            let Some((fin, opcode, start, len)) = self.header()? else { return Ok(None) };
            if self.buf.len() < start + len {
                return Ok(None);
            }
            let payload: Vec<u8> = self.buf[start..start + len].to_vec();
            self.buf.drain(..start + len);
            match opcode {
                OP_PING => return Ok(Some(Message::Ping(payload))),
                OP_PONG => return Ok(Some(Message::Pong(payload))),
                OP_CLOSE => {
                    self.closed = true;
                    if payload.len() == 1 {
                        return Err(Broken::BadControlFrame);
                    }
                    let code = (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
                    return Ok(Some(Message::Close(code)));
                }
                OP_CONTINUATION => {
                    let Some((first, mut assembled)) = self.partial.take() else {
                        return Err(Broken::UnexpectedContinuation);
                    };
                    if assembled.len() + payload.len() > MAX_MESSAGE {
                        return Err(Broken::TooLarge);
                    }
                    assembled.extend_from_slice(&payload);
                    if fin {
                        return finish(first, assembled).map(Some);
                    }
                    self.partial = Some((first, assembled));
                }
                OP_TEXT | OP_BINARY => {
                    if self.partial.is_some() {
                        return Err(Broken::InterleavedMessage);
                    }
                    if fin {
                        return finish(opcode, payload).map(Some);
                    }
                    self.partial = Some((opcode, payload));
                }
                other => return Err(Broken::UnknownOpcode(other)),
            }
        }
    }

    /// FIN, opcode, payload offset and payload length of the frame at the front, once its
    /// header is complete.
    fn header(&self) -> Result<Option<(bool, u8, usize, usize)>, Broken> {
        let b = &self.buf;
        if b.len() < 2 {
            return Ok(None);
        }
        let (fin, rsv, opcode) = (b[0] & 0x80 != 0, b[0] & 0x70, b[0] & 0x0f);
        if rsv != 0 {
            return Err(Broken::ReservedBits);
        }
        if !matches!(opcode, OP_CONTINUATION | OP_TEXT | OP_BINARY | OP_CLOSE | OP_PING | OP_PONG) {
            return Err(Broken::UnknownOpcode(opcode));
        }
        if b[1] & 0x80 != 0 {
            return Err(Broken::Masked);
        }
        let (len, start) = match b[1] & 0x7f {
            126 => {
                if b.len() < 4 {
                    return Ok(None);
                }
                (u64::from(u16::from_be_bytes([b[2], b[3]])), 4)
            }
            127 => {
                if b.len() < 10 {
                    return Ok(None);
                }
                let mut n = [0u8; 8];
                n.copy_from_slice(&b[2..10]);
                (u64::from_be_bytes(n), 10)
            }
            n => (u64::from(n), 2),
        };
        if opcode >= OP_CLOSE && (!fin || len > 125) {
            return Err(Broken::BadControlFrame);
        }
        if len > MAX_MESSAGE as u64 {
            return Err(Broken::TooLarge);
        }
        Ok(Some((fin, opcode, start, len as usize)))
    }
}

fn finish(opcode: u8, payload: Vec<u8>) -> Result<Message, Broken> {
    if opcode == OP_TEXT {
        String::from_utf8(payload).map(Message::Text).map_err(|_| Broken::NotUtf8)
    } else {
        Ok(Message::Binary(payload))
    }
}

/// Fuzzing entry (`fuzzing::websocket`): decode arbitrary bytes in arbitrary pieces; the
/// decoder must stay within its bounds and never panic.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) -> usize {
    let mut decoder = Decoder::new();
    let mut messages = 0;
    let step = data.first().map_or(1, |b| usize::from(*b % 7) + 1);
    for piece in data.chunks(step) {
        decoder.push(piece);
        loop {
            match decoder.next_message() {
                Ok(Some(message)) => {
                    messages += 1;
                    let len = match &message {
                        Message::Text(t) => t.len(),
                        Message::Binary(b) | Message::Ping(b) | Message::Pong(b) => b.len(),
                        Message::Close(_) => 0,
                    };
                    assert!(len <= MAX_MESSAGE);
                }
                Ok(None) => break,
                Err(_) => return messages,
            }
        }
        assert!(decoder.buffered() <= data.len());
    }
    let _ = check_handshake(data, "dGhlIHNhbXBsZSBub25jZQ==");
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame as a server sends it: unmasked.
    fn server_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![if fin { 0x80 } else { 0 } | opcode];
        match payload.len() {
            n if n < 126 => v.push(n as u8),
            n if n <= 0xffff => {
                v.push(126);
                v.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                v.push(127);
                v.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn the_handshake_is_checked_against_the_key() {
        // RFC 6455, section 1.3.
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        assert_eq!(accept_for(key), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        let good = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n";
        check_handshake(good.as_bytes(), key).unwrap();
        assert!(check_handshake(good.replace("101", "400").as_bytes(), key).is_err());
        assert!(check_handshake(good.replace("s3pP", "x3pP").as_bytes(), key).is_err());
        assert!(check_handshake(good.replace("Upgrade: websocket\r\n", "").as_bytes(), key).is_err());
        assert_eq!(head_end(good.as_bytes()), Some(good.len()));
        assert!(
            handshake_request("127.0.0.1:9222", "/session", key)
                .starts_with("GET /session HTTP/1.1\r\nHost: 127.0.0.1:9222\r\n")
        );
    }

    #[test]
    fn frames_out_are_masked_and_frames_in_are_decoded_in_any_pieces() {
        let frame = encode(OP_TEXT, b"hi", [1, 2, 3, 4]);
        assert_eq!(frame, [0x81, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'i' ^ 2]);
        assert_eq!(encode(OP_TEXT, &[0; 200], [0; 4])[1], 0x80 | 126);
        assert_eq!(encode(OP_TEXT, &[0; 70_000], [0; 4])[1], 0x80 | 127);

        let mut stream = server_frame(true, OP_TEXT, "{\"id\":1}".as_bytes());
        stream.extend(server_frame(false, OP_TEXT, b"{\"a\""));
        stream.extend(server_frame(true, OP_PING, b"p"));
        stream.extend(server_frame(true, OP_CONTINUATION, b":1}"));
        stream.extend(server_frame(true, OP_BINARY, &vec![9u8; 70_000]));
        stream.extend(server_frame(true, OP_CLOSE, &1000u16.to_be_bytes()));
        for step in [1, 3, 7, stream.len()] {
            let mut d = Decoder::new();
            let mut got = Vec::new();
            for piece in stream.chunks(step) {
                d.push(piece);
                while let Some(m) = d.next_message().unwrap() {
                    got.push(m);
                }
            }
            assert_eq!(
                got,
                [
                    Message::Text("{\"id\":1}".into()),
                    Message::Ping(b"p".to_vec()),
                    Message::Text("{\"a\":1}".into()),
                    Message::Binary(vec![9u8; 70_000]),
                    Message::Close(Some(1000)),
                ],
                "step {step}"
            );
        }
    }

    #[test]
    fn what_breaks_the_protocol_ends_the_stream() {
        let cases: Vec<(Vec<u8>, Broken)> = vec![
            (vec![0x81, 0x81, 0, 0, 0, 0, b'x'], Broken::Masked),
            (vec![0xC1, 0x00], Broken::ReservedBits),
            (vec![0x83, 0x00], Broken::UnknownOpcode(3)),
            (vec![0x09, 0x00], Broken::BadControlFrame),
            (server_frame(true, OP_PING, &[0; 126]), Broken::BadControlFrame),
            (server_frame(true, OP_CONTINUATION, b"x"), Broken::UnexpectedContinuation),
            (
                [server_frame(false, OP_TEXT, b"a"), server_frame(true, OP_TEXT, b"b")].concat(),
                Broken::InterleavedMessage,
            ),
            (server_frame(true, OP_TEXT, &[0xff, 0xfe]), Broken::NotUtf8),
            ([vec![0x82, 127], ((MAX_MESSAGE as u64) + 1).to_be_bytes().to_vec()].concat(), Broken::TooLarge),
            ([server_frame(true, OP_CLOSE, &[]), server_frame(true, OP_TEXT, b"x")].concat(), Broken::AfterClose),
        ];
        for (bytes, want) in cases {
            let mut d = Decoder::new();
            d.push(&bytes);
            let mut result = d.next_message();
            while let Ok(Some(_)) = result {
                result = d.next_message();
            }
            assert_eq!(result, Err(want.clone()), "{bytes:02x?}");
        }
        // Fragments that add up to more than a message may be are refused as they arrive.
        let mut d = Decoder::new();
        d.push(&server_frame(false, OP_BINARY, &vec![0; 65_536]));
        assert_eq!(d.next_message(), Ok(None));
        let mut big = vec![0x00, 127];
        big.extend_from_slice(&(MAX_MESSAGE as u64).to_be_bytes());
        d.push(&big);
        d.push(&vec![0; MAX_MESSAGE]);
        assert_eq!(d.next_message(), Err(Broken::TooLarge));
    }
}
