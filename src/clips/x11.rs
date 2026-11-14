//! Just enough X11 for the clip recorder to find the game's window: the connection setup
//! (with an MIT-MAGIC-COOKIE-1, if there is one), and a handful of requests — `QueryTree`,
//! `GetWindowAttributes`, `GetGeometry`, `InternAtom`, `GetProperty`, and the X-Resource
//! extension's `QueryClientIds` (which client process a window belongs to).
//!
//! It runs in the recorder's sandbox, not in the launcher: what the X server answers is
//! parsed there, behind that sandbox's walls. Parsed strictly all the same — every length is
//! checked against what was read, nothing is trusted to fit ([`fuzz`]).

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

/// A reply longer than this is not one the recorder asked for.
const MAX_REPLY: usize = 1 << 20;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X11Error {
    Io(String),
    /// The server refused the connection (with its reason).
    Refused(String),
    /// An X error for a request (code); `BadWindow` (3) means the window is gone.
    Error(u8),
    Malformed(&'static str),
}

impl std::fmt::Display for X11Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            X11Error::Io(e) => write!(f, "X11: {e}"),
            X11Error::Refused(why) => write!(f, "the X server refused the connection: {why}"),
            X11Error::Error(code) => write!(f, "X11 error {code}"),
            X11Error::Malformed(what) => write!(f, "X11: malformed {what}"),
        }
    }
}

type Result<T> = std::result::Result<T, X11Error>;

fn io(e: std::io::Error) -> X11Error {
    X11Error::Io(e.to_string())
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or(X11Error::Malformed("reply"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or(X11Error::Malformed("reply"))
}

fn pad4(n: usize) -> usize {
    (4 - n % 4) % 4
}

/// The connection setup request: little-endian, protocol 11.0, with an authorization if given.
pub fn setup_request(auth: Option<(&[u8], &[u8])>) -> Vec<u8> {
    let (name, data) = auth.unwrap_or((b"", b""));
    let mut out = vec![b'l', 0, 11, 0, 0, 0];
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(name);
    out.extend(std::iter::repeat_n(0, pad4(name.len())));
    out.extend_from_slice(data);
    out.extend(std::iter::repeat_n(0, pad4(data.len())));
    out
}

/// From a successful setup reply's body (everything after its first eight bytes): the first
/// screen's root window, and the resource id base and mask.
pub fn parse_setup(status: u8, body: &[u8]) -> Result<u32> {
    match status {
        1 => {}
        0 | 2 => {
            let reason = String::from_utf8_lossy(&body[..body.len().min(200)]);
            return Err(X11Error::Refused(reason.chars().filter(|c| !c.is_control()).collect()));
        }
        _ => return Err(X11Error::Malformed("setup reply")),
    }
    let vendor_len = usize::from(u16_at(body, 16)?);
    let screens = body.get(20).copied().ok_or(X11Error::Malformed("setup reply"))?;
    let formats = usize::from(body.get(21).copied().ok_or(X11Error::Malformed("setup reply"))?);
    if screens == 0 {
        return Err(X11Error::Malformed("setup reply without a screen"));
    }
    let first_screen = 32 + vendor_len + pad4(vendor_len) + 8 * formats;
    u32_at(body, first_screen)
}

/// What a window looks like to the recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub id: u32,
    pub width: u16,
    pub height: u16,
    pub viewable: bool,
    /// InputOutput (1); InputOnly windows (2) have no pixels.
    pub input_output: bool,
    pub override_redirect: bool,
}

/// A connection, used one request at a time.
pub struct X11 {
    stream: UnixStream,
    sequence: u16,
    pub root: u32,
    xres: Option<u8>,
}

impl X11 {
    /// Connect to the display socket at `socket`, with the cookie in `xauthority` (an
    /// Xauthority file reduced to one record, as the launcher writes it), if any.
    pub fn connect(socket: &Path, xauthority: Option<&Path>) -> Result<X11> {
        let mut stream = UnixStream::connect(socket).map_err(io)?;
        stream.set_read_timeout(Some(TIMEOUT)).map_err(io)?;
        stream.set_write_timeout(Some(TIMEOUT)).map_err(io)?;
        let cookie = xauthority.and_then(|p| std::fs::read(p).ok()).and_then(|raw| first_cookie(&raw));
        let request = match &cookie {
            Some(data) => setup_request(Some((b"MIT-MAGIC-COOKIE-1", data))),
            None => setup_request(None),
        };
        stream.write_all(&request).map_err(io)?;
        let mut head = [0u8; 8];
        stream.read_exact(&mut head).map_err(io)?;
        let words = usize::from(u16::from_le_bytes([head[6], head[7]]));
        let mut body = vec![0u8; words * 4];
        if body.len() > MAX_REPLY {
            return Err(X11Error::Malformed("setup reply"));
        }
        stream.read_exact(&mut body).map_err(io)?;
        let root = parse_setup(head[0], &body)?;
        let mut x = X11 { stream, sequence: 0, root, xres: None };
        x.xres = x.extension(b"X-Resource")?;
        if let Some(major) = x.xres {
            // QueryVersion first: QueryClientIds is version 1.2.
            let reply = x.request(&[major, 0, 2, 0, 1, 2, 0, 0])?;
            let (server_major, server_minor) = (u16_at(&reply, 8)?, u16_at(&reply, 10)?);
            if (server_major, server_minor) < (1, 2) {
                x.xres = None;
            }
        }
        Ok(x)
    }

    /// Send one request and wait for its reply (events in between are skipped).
    fn request(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        debug_assert!(
            request.len().is_multiple_of(4)
                && usize::from(u16::from_le_bytes([request[2], request[3]])) * 4 == request.len()
        );
        self.stream.write_all(request).map_err(io)?;
        self.sequence = self.sequence.wrapping_add(1);
        read_reply(&mut self.stream, self.sequence)
    }

    fn extension(&mut self, name: &[u8]) -> Result<Option<u8>> {
        let mut request = vec![98, 0, 0, 0];
        request.extend_from_slice(&(name.len() as u16).to_le_bytes());
        request.extend_from_slice(&[0, 0]);
        request.extend_from_slice(name);
        request.extend(std::iter::repeat_n(0, pad4(name.len())));
        let words = (request.len() / 4) as u16;
        request[2..4].copy_from_slice(&words.to_le_bytes());
        let reply = self.request(&request)?;
        Ok((reply.get(8) == Some(&1)).then(|| reply[9]))
    }

    /// The children of `window`, bottom to top.
    pub fn children(&mut self, window: u32) -> Result<Vec<u32>> {
        let mut request = vec![15, 0, 2, 0];
        request.extend_from_slice(&window.to_le_bytes());
        parse_query_tree(&self.request(&request)?)
    }

    pub fn window(&mut self, id: u32) -> Result<Window> {
        let mut request = vec![3, 0, 2, 0];
        request.extend_from_slice(&id.to_le_bytes());
        let attributes = self.request(&request)?;
        let mut request = vec![14, 0, 2, 0];
        request.extend_from_slice(&id.to_le_bytes());
        let geometry = self.request(&request)?;
        parse_window(id, &attributes, &geometry)
    }

    pub fn atom(&mut self, name: &[u8]) -> Result<u32> {
        let mut request = vec![16, 1, 0, 0];
        request.extend_from_slice(&(name.len() as u16).to_le_bytes());
        request.extend_from_slice(&[0, 0]);
        request.extend_from_slice(name);
        request.extend(std::iter::repeat_n(0, pad4(name.len())));
        let words = (request.len() / 4) as u16;
        request[2..4].copy_from_slice(&words.to_le_bytes());
        u32_at(&self.request(&request)?, 8)
    }

    /// A property's raw value (at most 1 KiB of it), or none.
    pub fn property(&mut self, window: u32, property: u32) -> Result<Option<(u8, Vec<u8>)>> {
        if property == 0 {
            return Ok(None);
        }
        let mut request = vec![20, 0, 6, 0];
        for word in [window, property, 0, 0, 256] {
            request.extend_from_slice(&word.to_le_bytes());
        }
        parse_property(&self.request(&request)?)
    }

    /// The process id of the client that owns `window`, as the server knows it (0 or none:
    /// it does not know — a server in another PID namespace, or no X-Resource extension).
    pub fn client_pid(&mut self, window: u32) -> Result<Option<u32>> {
        let Some(major) = self.xres else { return Ok(None) };
        let mut request = vec![major, 4, 4, 0];
        // One spec: the window's client, PID mask (2).
        for word in [1u32, window, 2] {
            request.extend_from_slice(&word.to_le_bytes());
        }
        parse_client_ids(&self.request(&request)?)
    }
}

/// Read until the reply (or error) with `sequence` arrives; events are skipped.
fn read_reply(stream: &mut impl Read, sequence: u16) -> Result<Vec<u8>> {
    loop {
        let mut head = [0u8; 32];
        stream.read_exact(&mut head).map_err(io)?;
        let seq = u16::from_le_bytes([head[2], head[3]]);
        match head[0] {
            0 => {
                if seq == sequence {
                    return Err(X11Error::Error(head[1]));
                }
            }
            1 => {
                let extra = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize * 4;
                if extra > MAX_REPLY {
                    return Err(X11Error::Malformed("reply length"));
                }
                let mut reply = head.to_vec();
                reply.resize(32 + extra, 0);
                stream.read_exact(&mut reply[32..]).map_err(io)?;
                if seq == sequence {
                    return Ok(reply);
                }
            }
            // A generic event carries a length of its own.
            35 | 0xa3 => {
                let extra = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize * 4;
                if extra > MAX_REPLY {
                    return Err(X11Error::Malformed("event length"));
                }
                let mut skip = vec![0u8; extra];
                stream.read_exact(&mut skip).map_err(io)?;
            }
            _ => {}
        }
    }
}

pub fn parse_query_tree(reply: &[u8]) -> Result<Vec<u32>> {
    let count = usize::from(u16_at(reply, 16)?);
    (0..count).map(|i| u32_at(reply, 32 + 4 * i)).collect()
}

pub fn parse_window(id: u32, attributes: &[u8], geometry: &[u8]) -> Result<Window> {
    if attributes.len() < 44 || geometry.len() < 32 {
        return Err(X11Error::Malformed("window reply"));
    }
    Ok(Window {
        id,
        width: u16_at(geometry, 16)?,
        height: u16_at(geometry, 18)?,
        viewable: attributes[26] == 2,
        input_output: u16_at(attributes, 12)? == 1,
        override_redirect: attributes[27] != 0,
    })
}

pub fn parse_property(reply: &[u8]) -> Result<Option<(u8, Vec<u8>)>> {
    let (format, kind) = (*reply.get(1).ok_or(X11Error::Malformed("property"))?, u32_at(reply, 8)?);
    if kind == 0 {
        return Ok(None);
    }
    let units = u32_at(reply, 16)? as usize;
    let bytes = match format {
        8 => units,
        16 => units.checked_mul(2).ok_or(X11Error::Malformed("property"))?,
        32 => units.checked_mul(4).ok_or(X11Error::Malformed("property"))?,
        _ => return Err(X11Error::Malformed("property format")),
    };
    let value = reply
        .get(32..32usize.checked_add(bytes).ok_or(X11Error::Malformed("property"))?)
        .ok_or(X11Error::Malformed("property"))?;
    Ok(Some((format, value.to_vec())))
}

pub fn parse_client_ids(reply: &[u8]) -> Result<Option<u32>> {
    let count = u32_at(reply, 8)? as usize;
    let mut at = 32;
    for _ in 0..count.min(64) {
        let (mask, length) = (u32_at(reply, at + 4)?, u32_at(reply, at + 8)? as usize);
        let value = reply.get(at + 12..at + 12 + length.min(MAX_REPLY)).ok_or(X11Error::Malformed("client ids"))?;
        if mask == 2 && length == 4 {
            let pid = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
            return Ok((pid != 0 && pid != u32::MAX).then_some(pid));
        }
        at += 12 + length + pad4(length);
    }
    Ok(None)
}

/// The MIT cookie of the first record of an Xauthority file (the launcher writes one).
pub fn first_cookie(raw: &[u8]) -> Option<Vec<u8>> {
    // The family (a plain u16), then four length-prefixed fields.
    let mut rest = raw.get(2..)?;
    let mut next = || -> Option<Vec<u8>> {
        let len = usize::from(u16::from_be_bytes([*rest.first()?, *rest.get(1)?]));
        let value = rest.get(2..2 + len)?.to_vec();
        rest = &rest[2 + len..];
        Some(value)
    };
    let (_address, _number, name, data) = (next()?, next()?, next()?, next()?);
    (name == b"MIT-MAGIC-COOKIE-1" && !data.is_empty() && data.len() <= 256).then_some(data)
}

/// The parsers on arbitrary bytes (as replies from a server).
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    let _ = parse_query_tree(data);
    let _ = parse_property(data);
    let _ = parse_client_ids(data);
    if data.len() > 8 {
        let _ = parse_setup(data[0], &data[8..]);
        let _ = parse_window(1, data, &data[data.len() / 2..]);
    }
    let _ = first_cookie(data);
    let mut cursor = data;
    let _ = read_reply(&mut cursor, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_request_carries_the_cookie_padded() {
        let r = setup_request(Some((b"MIT-MAGIC-COOKIE-1", &[7; 16])));
        assert_eq!(&r[..12], &[b'l', 0, 11, 0, 0, 0, 18, 0, 16, 0, 0, 0]);
        assert_eq!(r.len(), 12 + 20 + 16);
        assert_eq!(setup_request(None), vec![b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn replies_are_read_strictly() {
        // QueryTree with two children.
        let mut tree = vec![1, 0, 5, 0, 2, 0, 0, 0];
        tree.extend_from_slice(&[0; 8]); // root, parent
        tree.extend_from_slice(&2u16.to_le_bytes());
        tree.extend_from_slice(&[0; 14]);
        tree.extend_from_slice(&0x400037u32.to_le_bytes());
        tree.extend_from_slice(&0x40003du32.to_le_bytes());
        assert_eq!(parse_query_tree(&tree).unwrap(), [0x400037, 0x40003d]);
        tree.truncate(tree.len() - 2);
        assert!(parse_query_tree(&tree).is_err(), "a child that is not all there");
        // A property claiming more than the reply holds.
        let mut property = vec![1, 8, 0, 0, 0, 0, 0, 0];
        property.extend_from_slice(&31u32.to_le_bytes()); // type STRING
        property.extend_from_slice(&0u32.to_le_bytes());
        property.extend_from_slice(&1000u32.to_le_bytes());
        property.extend_from_slice(&[0; 12]);
        property.extend_from_slice(b"bastion\0");
        assert!(parse_property(&property).is_err());
        property[16..20].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(parse_property(&property).unwrap(), Some((8, b"bastion".to_vec())));
        // Client ids: one PID spec with a PID, then the unknown PID 0.
        let mut ids = vec![1, 0, 0, 0, 4, 0, 0, 0];
        ids.extend_from_slice(&1u32.to_le_bytes());
        ids.extend_from_slice(&[0; 20]);
        ids.extend_from_slice(&0x400000u32.to_le_bytes());
        ids.extend_from_slice(&2u32.to_le_bytes());
        ids.extend_from_slice(&4u32.to_le_bytes());
        ids.extend_from_slice(&4242u32.to_le_bytes());
        assert_eq!(parse_client_ids(&ids).unwrap(), Some(4242));
        let n = ids.len();
        ids[n - 4..].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse_client_ids(&ids).unwrap(), None, "PID 0: the server does not know");
    }

    #[test]
    fn events_before_the_reply_are_skipped_and_errors_reported() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&[22; 32]); // some event
        let mut generic = vec![35, 0, 0, 0, 1, 0, 0, 0];
        generic.resize(36, 0);
        stream.extend_from_slice(&generic);
        let mut error = vec![0, 3, 7, 0];
        error.resize(32, 0);
        stream.extend_from_slice(&error);
        let mut cursor = &stream[..];
        assert_eq!(read_reply(&mut cursor, 7), Err(X11Error::Error(3)));
    }

    #[test]
    fn the_cookie_of_the_launchers_xauthority_record_is_found() {
        let mut record = 0xffffu16.to_be_bytes().to_vec();
        for field in [&b""[..], b"", b"MIT-MAGIC-COOKIE-1", &[9; 16]] {
            record.extend_from_slice(&(field.len() as u16).to_be_bytes());
            record.extend_from_slice(field);
        }
        assert_eq!(first_cookie(&record), Some(vec![9; 16]));
        assert_eq!(first_cookie(&record[..record.len() - 1]), None);
        fuzz(&record);
    }
}
