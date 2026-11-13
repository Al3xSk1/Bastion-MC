//! Extract the server name from a TLS ClientHello, without terminating TLS.
//!
//! The relay only forwards a connection to an allowlisted HTTPS host if the ClientHello names
//! *that* host. Otherwise code in the sandbox could resolve an allowed name, connect to the
//! (shared CDN) address behind it, and ask the CDN for a different site by SNI.

use crate::config::normalize_host;

#[derive(Debug, PartialEq, Eq)]
pub enum Hello {
    /// More bytes are needed to see the whole first record.
    Incomplete,
    /// A complete ClientHello; the SNI host name if one was sent.
    Complete(Option<String>),
    /// Not a TLS ClientHello we are willing to forward.
    Invalid,
}

/// Largest first record we buffer: a TLS record payload is at most 2^14 bytes.
pub const MAX_HELLO: usize = 5 + 16384;

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }
    fn u8(&mut self) -> Option<usize> {
        self.take(1).map(|b| b[0] as usize)
    }
    fn u16(&mut self) -> Option<usize> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
    }
    fn u24(&mut self) -> Option<usize> {
        self.take(3).map(|b| (b[0] as usize) << 16 | (b[1] as usize) << 8 | b[2] as usize)
    }
}

pub fn parse_client_hello(buf: &[u8]) -> Hello {
    if buf.len() < 5 {
        return Hello::Incomplete;
    }
    // Record: handshake(22), legacy version 3.x, length.
    if buf[0] != 22 || buf[1] != 3 {
        return Hello::Invalid;
    }
    let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if record_len == 0 || record_len > 16384 {
        return Hello::Invalid;
    }
    if buf.len() < 5 + record_len {
        return Hello::Incomplete;
    }
    match parse_handshake(&buf[5..5 + record_len]) {
        Some(sni) => Hello::Complete(sni),
        None => Hello::Invalid,
    }
}

fn parse_handshake(record: &[u8]) -> Option<Option<String>> {
    let mut c = Cursor(record);
    if c.u8()? != 1 {
        return None; // not a ClientHello
    }
    let len = c.u24()?;
    // A ClientHello fragmented over several records is not forwarded (no client we relay
    // for does that); this also means the whole message has been inspected.
    let mut body = Cursor(c.take(len)?);
    body.take(2 + 32)?; // legacy_version, random
    let n = body.u8()?;
    body.take(n)?; // session id
    let n = body.u16()?;
    body.take(n)?; // cipher suites
    let n = body.u8()?;
    body.take(n)?; // compression methods
    if body.0.is_empty() {
        return Some(None); // no extensions at all
    }
    let n = body.u16()?;
    let mut exts = Cursor(body.take(n)?);
    let mut found: Option<String> = None;
    while !exts.0.is_empty() {
        let ext_type = exts.u16()?;
        let n = exts.u16()?;
        let data = exts.take(n)?;
        if ext_type != 0 {
            continue;
        }
        if found.is_some() {
            return None; // duplicate server_name extension
        }
        let mut sn = Cursor(data);
        let n = sn.u16()?;
        let mut list = Cursor(sn.take(n)?);
        let name_type = list.u8()?;
        let n = list.u16()?;
        let name = list.take(n)?;
        if name_type != 0 || !list.0.is_empty() {
            return None; // only a single host_name entry is defined
        }
        found = Some(normalize_host(std::str::from_utf8(name).ok()?).ok()?);
    }
    Some(found)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a syntactically valid ClientHello record with the given extensions.
    pub(crate) fn client_hello(extensions: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut ext = Vec::new();
        for (t, d) in extensions {
            ext.extend_from_slice(&t.to_be_bytes());
            ext.extend_from_slice(&(d.len() as u16).to_be_bytes());
            ext.extend_from_slice(d);
        }
        let mut body = vec![3, 3];
        body.extend_from_slice(&[0x11; 32]);
        body.push(0); // session id
        body.extend_from_slice(&[0, 2, 0x13, 0x01]); // one cipher suite
        body.extend_from_slice(&[1, 0]); // null compression
        body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
        body.extend_from_slice(&ext);
        let mut hs = vec![1, 0, (body.len() >> 8) as u8, body.len() as u8];
        hs.extend_from_slice(&body);
        let mut rec = vec![22, 3, 1, (hs.len() >> 8) as u8, hs.len() as u8];
        rec.extend_from_slice(&hs);
        rec
    }

    pub(crate) fn sni_ext(host: &str) -> (u16, Vec<u8>) {
        let mut d = Vec::new();
        d.extend_from_slice(&((host.len() + 3) as u16).to_be_bytes());
        d.push(0);
        d.extend_from_slice(&(host.len() as u16).to_be_bytes());
        d.extend_from_slice(host.as_bytes());
        (0, d)
    }

    #[test]
    fn extracts_server_name() {
        let hello = client_hello(&[(43, vec![2, 3, 4]), sni_ext("SessionServer.Mojang.com"), (10, vec![0, 2, 0, 29])]);
        assert_eq!(parse_client_hello(&hello), Hello::Complete(Some("sessionserver.mojang.com".into())));
        assert_eq!(parse_client_hello(&client_hello(&[(43, vec![2, 3, 4])])), Hello::Complete(None));
    }

    #[test]
    fn incomplete_then_complete() {
        let hello = client_hello(&[sni_ext("textures.minecraft.net")]);
        for cut in 0..hello.len() {
            assert_eq!(parse_client_hello(&hello[..cut]), Hello::Incomplete, "cut at {cut}");
        }
        assert!(matches!(parse_client_hello(&hello), Hello::Complete(Some(_))));
        // trailing bytes (start of the next record) are fine
        let mut more = hello.clone();
        more.extend_from_slice(&[23, 3, 3, 0, 1, 0]);
        assert!(matches!(parse_client_hello(&more), Hello::Complete(Some(_))));
    }

    #[test]
    fn rejects_non_tls_and_malformed_hellos() {
        assert_eq!(parse_client_hello(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"), Hello::Invalid);
        assert_eq!(parse_client_hello(&[22, 3, 1, 0, 0]), Hello::Invalid);
        assert_eq!(parse_client_hello(&[22, 3, 1, 0xff, 0xff]), Hello::Invalid);
        // ServerHello instead of ClientHello
        let mut sh = client_hello(&[sni_ext("a.example")]);
        sh[5] = 2;
        assert_eq!(parse_client_hello(&sh), Hello::Invalid);
        // two server_name extensions
        assert_eq!(parse_client_hello(&client_hello(&[sni_ext("a.example"), sni_ext("b.example")])), Hello::Invalid);
        // a name that is not a host name
        assert_eq!(parse_client_hello(&client_hello(&[sni_ext("a b")])), Hello::Invalid);
        assert_eq!(parse_client_hello(&client_hello(&[sni_ext("evil.example/../x")])), Hello::Invalid);
        // inner lengths that overrun: flip every length-ish byte and require no panic
        let good = client_hello(&[sni_ext("a.example")]);
        for i in 5..good.len() {
            let mut m = good.clone();
            m[i] = m[i].wrapping_add(0x80);
            let _ = parse_client_hello(&m);
        }
    }
}
