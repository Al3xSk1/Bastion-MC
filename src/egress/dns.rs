//! Minimal DNS wire-format handling. Two uses, both on untrusted input:
//!
//! * **Responder** (queries come from the sandboxed JVM): answers A queries for allowlisted
//!   names with synthetic loopback addresses and refuses everything else *locally*. A query is
//!   never forwarded upstream, so DNS cannot be used as a covert exfiltration channel
//!   (`<stolen-bytes>.attacker.example` goes nowhere).
//! * **SRV client** (answers come from the network): resolves `_minecraft._tcp.<server>` for
//!   allowlisted servers on the host side.
//!
//! Everything is bounds-checked; compression pointers are followed with a hop limit; names
//! are restricted to LDH (+ underscore) and lower-cased before any policy decision.

use crate::error::{Error, Result};

pub const TYPE_A: u16 = 1;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_SRV: u16 = 33;
const CLASS_IN: u16 = 1;
const MAX_NAME: usize = 253;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rcode {
    NoError = 0,
    FormErr = 1,
    ServFail = 2,
    NxDomain = 3,
    NotImp = 4,
    Refused = 5,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub id: u16,
    /// Lower-case, no trailing dot.
    pub name: String,
    pub qtype: u16,
    recursion_desired: bool,
    /// The question section exactly as received, echoed in the response.
    question: Vec<u8>,
}

fn u16_at(buf: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*buf.get(at)?, *buf.get(at + 1)?]))
}

fn label_byte_ok(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Read an uncompressed name starting at `at`; returns the name and the offset after it.
fn read_plain_name(buf: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    loop {
        let len = *buf.get(at)? as usize;
        at += 1;
        if len == 0 {
            return Some((name, at));
        }
        if len > 63 {
            return None; // compression pointer or reserved label type
        }
        let label = buf.get(at..at + len)?;
        if !label.iter().all(|b| label_byte_ok(*b)) {
            return None;
        }
        if !name.is_empty() {
            name.push('.');
        }
        name.extend(label.iter().map(|b| b.to_ascii_lowercase() as char));
        if name.len() > MAX_NAME {
            return None;
        }
        at += len;
    }
}

/// Parse a query from the sandbox. `Err` carries the id (when readable) and the rcode to send.
pub fn parse_query(packet: &[u8]) -> std::result::Result<Query, (Option<u16>, Rcode)> {
    let id = u16_at(packet, 0);
    let fail = |r| (id, r);
    if packet.len() < 12 || packet.len() > 512 {
        return Err(fail(Rcode::FormErr));
    }
    let flags = u16_at(packet, 2).ok_or(fail(Rcode::FormErr))?;
    if flags & 0x8000 != 0 {
        return Err((None, Rcode::FormErr)); // a response, not a query: never answer it
    }
    if (flags >> 11) & 0xF != 0 {
        return Err(fail(Rcode::NotImp));
    }
    if u16_at(packet, 4) != Some(1) {
        return Err(fail(Rcode::FormErr));
    }
    let (name, after) = read_plain_name(packet, 12).ok_or(fail(Rcode::FormErr))?;
    let qtype = u16_at(packet, after).ok_or(fail(Rcode::FormErr))?;
    let qclass = u16_at(packet, after + 2).ok_or(fail(Rcode::FormErr))?;
    if qclass != CLASS_IN {
        return Err(fail(Rcode::Refused));
    }
    Ok(Query {
        id: id.ok_or(fail(Rcode::FormErr))?,
        name,
        qtype,
        recursion_desired: flags & 0x0100 != 0,
        question: packet[12..after + 4].to_vec(),
    })
}

fn header(id: u16, rd: bool, rcode: Rcode, questions: u16, answers: u16) -> Vec<u8> {
    // QR=1, AA=1, RA=1.
    let flags: u16 = 0x8000 | 0x0400 | 0x0080 | if rd { 0x0100 } else { 0 } | rcode as u16;
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&questions.to_be_bytes());
    out.extend_from_slice(&answers.to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

/// A response with a single A record. TTL is short; the mapping is stable for the session.
pub fn answer_a(q: &Query, addr: std::net::Ipv4Addr) -> Vec<u8> {
    let mut out = header(q.id, q.recursion_desired, Rcode::NoError, 1, 1);
    out.extend_from_slice(&q.question);
    out.extend_from_slice(&[0xC0, 0x0C]); // pointer to the question name
    out.extend_from_slice(&TYPE_A.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out.extend_from_slice(&30u32.to_be_bytes());
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(&addr.octets());
    out
}

/// An answerless response (NOERROR/NODATA, REFUSED, …) echoing the question.
pub fn answer_empty(q: &Query, rcode: Rcode) -> Vec<u8> {
    let mut out = header(q.id, q.recursion_desired, rcode, 1, 0);
    out.extend_from_slice(&q.question);
    out
}

/// For packets that could not be parsed far enough to echo a question.
pub fn answer_error(id: u16, rcode: Rcode) -> Vec<u8> {
    header(id, false, rcode, 0, 0)
}

// ---------------------------------------------------------------- SRV client

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Srv {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

fn write_name(out: &mut Vec<u8>, name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(Error::Format("DNS name length".into()));
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 || !label.bytes().all(label_byte_ok) {
            return Err(Error::Format(format!("invalid DNS label in {name:?}")));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(())
}

pub fn build_srv_query(id: u16, name: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(32 + name.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    out.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
    write_name(&mut out, name)?;
    out.extend_from_slice(&TYPE_SRV.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(out)
}

/// Read a possibly compressed name. Returns the name (lower-case; `None` if it contains
/// bytes outside the accepted alphabet) and the offset just past it in the original stream.
fn read_name(buf: &[u8], start: usize) -> Option<(Option<String>, usize)> {
    let mut at = start;
    let mut after = None;
    let mut name = String::new();
    let mut valid = true;
    for _hop in 0..64 {
        let len = *buf.get(at)? as usize;
        if len & 0xC0 == 0xC0 {
            let target = ((len & 0x3F) << 8) | *buf.get(at + 1)? as usize;
            after.get_or_insert(at + 2);
            // Pointers must go backwards; this also rules out loops.
            if target >= at {
                return None;
            }
            at = target;
            continue;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        at += 1;
        if len == 0 {
            return Some((valid.then_some(name), after.unwrap_or(at)));
        }
        let label = buf.get(at..at + len)?;
        valid &= label.iter().all(|b| label_byte_ok(*b));
        if !name.is_empty() {
            name.push('.');
        }
        name.extend(label.iter().map(|b| b.to_ascii_lowercase() as char));
        if name.len() > MAX_NAME {
            return None;
        }
        at += len;
    }
    None
}

/// Parse the response to [`build_srv_query`]. `Ok(vec![])` means "no such record".
pub fn parse_srv_response(packet: &[u8], id: u16, qname: &str) -> Result<Vec<Srv>> {
    let bad = |m: &str| Error::Format(format!("DNS response: {m}"));
    if packet.len() < 12 {
        return Err(bad("too short"));
    }
    if u16_at(packet, 0) != Some(id) {
        return Err(bad("transaction id mismatch"));
    }
    let flags = u16_at(packet, 2).ok_or_else(|| bad("truncated header"))?;
    if flags & 0x8000 == 0 {
        return Err(bad("not a response"));
    }
    if flags & 0x0200 != 0 {
        return Err(bad("truncated (TC) response"));
    }
    match flags & 0xF {
        0 => {}
        3 => return Ok(Vec::new()),
        r => return Err(bad(&format!("rcode {r}"))),
    }
    if u16_at(packet, 4) != Some(1) {
        return Err(bad("unexpected question count"));
    }
    let ancount = u16_at(packet, 6).ok_or_else(|| bad("truncated header"))?;
    // The echoed question must be ours: a cheap check against off-path spoofing.
    let (echoed, mut at) = read_name(packet, 12).ok_or_else(|| bad("bad question"))?;
    if echoed.as_deref() != Some(&qname.to_ascii_lowercase()) || u16_at(packet, at) != Some(TYPE_SRV) {
        return Err(bad("question does not match the query"));
    }
    at += 4;
    let mut out = Vec::new();
    for _ in 0..ancount.min(64) {
        let (_, next) = read_name(packet, at).ok_or_else(|| bad("bad record name"))?;
        let rtype = u16_at(packet, next).ok_or_else(|| bad("truncated record"))?;
        let rdlen = u16_at(packet, next + 8).ok_or_else(|| bad("truncated record"))? as usize;
        let rdata = next + 10;
        if packet.len() < rdata + rdlen {
            return Err(bad("record overruns packet"));
        }
        if rtype == TYPE_SRV && rdlen >= 7 {
            let (target, _) = read_name(packet, rdata + 6).ok_or_else(|| bad("bad SRV target"))?;
            // "." as target means "service decidedly not available".
            if let Some(target) = target.filter(|t| !t.is_empty()) {
                out.push(Srv {
                    priority: u16_at(packet, rdata).unwrap_or(u16::MAX),
                    weight: u16_at(packet, rdata + 2).unwrap_or(0),
                    port: u16_at(packet, rdata + 4).unwrap_or(0),
                    target,
                });
            }
        }
        at = rdata + rdlen;
    }
    out.retain(|s| s.port != 0);
    // Lowest priority first, then heaviest weight: deterministic choice of `out[0]`.
    out.sort_by_key(|s| (s.priority, std::cmp::Reverse(s.weight)));
    Ok(out)
}

/// Name servers from the host's resolver configuration.
pub fn system_nameservers() -> Vec<std::net::IpAddr> {
    let text = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    let mut servers: Vec<std::net::IpAddr> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .filter_map(|rest| rest.split_whitespace().next()?.split('%').next()?.parse().ok())
        .take(3)
        .collect();
    if servers.is_empty() {
        servers.push(std::net::Ipv4Addr::LOCALHOST.into());
    }
    servers
}

/// One SRV lookup against the system resolvers. `Ok(None)`: no usable record.
pub async fn lookup_srv(qname: &str) -> Result<Option<Srv>> {
    use std::{net::SocketAddr, time::Duration};
    use tokio::net::UdpSocket;
    let mut last = Error::Net("no name servers".into());
    for server in system_nameservers() {
        let id = u16::from_be_bytes(crate::util::random_bytes::<2>()?);
        let query = build_srv_query(id, qname)?;
        let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse().unwrap() } else { "[::]:0".parse().unwrap() };
        let attempt = async {
            // A fresh socket per query: random source port, and connect() makes the kernel
            // discard datagrams from any other address.
            let sock = UdpSocket::bind(bind).await?;
            sock.connect(SocketAddr::new(server, 53)).await?;
            sock.send(&query).await?;
            let mut buf = [0u8; 1500];
            let n = sock.recv(&mut buf).await?;
            Ok::<Vec<u8>, std::io::Error>(buf[..n].to_vec())
        };
        match tokio::time::timeout(Duration::from_secs(3), attempt).await {
            Ok(Ok(reply)) => match parse_srv_response(&reply, id, qname) {
                Ok(records) => return Ok(records.into_iter().next()),
                Err(e) => last = e,
            },
            Ok(Err(e)) => last = Error::Net(format!("DNS query to {server}: {e}")),
            Err(_) => last = Error::Net(format!("DNS query to {server} timed out")),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
        let mut q = Vec::new();
        q.extend_from_slice(&id.to_be_bytes());
        q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
        for l in name.split('.') {
            q.push(l.len() as u8);
            q.extend_from_slice(l.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&[0, 1]);
        q
    }

    #[test]
    fn parses_and_answers_a_query() {
        let pkt = query(0xBEEF, "SessionServer.Mojang.com", TYPE_A);
        let q = parse_query(&pkt).unwrap();
        assert_eq!((q.id, q.name.as_str(), q.qtype), (0xBEEF, "sessionserver.mojang.com", TYPE_A));
        let resp = answer_a(&q, Ipv4Addr::new(127, 64, 0, 1));
        assert_eq!(&resp[..2], &[0xBE, 0xEF]);
        assert_eq!(resp[2] & 0x80, 0x80, "QR set");
        assert_eq!(resp[3] & 0x0F, 0, "NOERROR");
        assert_eq!(&resp[4..8], &[0, 1, 0, 1]);
        assert_eq!(&resp[resp.len() - 4..], &[127, 64, 0, 1]);
        // The question is echoed byte for byte (including the client's original case).
        assert_eq!(&resp[12..12 + pkt.len() - 12], &pkt[12..]);
        let refused = answer_empty(&q, Rcode::Refused);
        assert_eq!(refused[3] & 0x0F, 5);
        assert_eq!(&refused[6..8], &[0, 0]);
    }

    #[test]
    fn rejects_malformed_queries() {
        assert!(parse_query(&[]).is_err());
        assert!(parse_query(&[0; 11]).is_err());
        assert!(parse_query(&vec![0; 600]).is_err());
        // response bit set: never answered
        let mut resp = query(1, "a.example", TYPE_A);
        resp[2] |= 0x80;
        assert_eq!(parse_query(&resp), Err((None, Rcode::FormErr)));
        // two questions
        let mut two = query(1, "a.example", TYPE_A);
        two[5] = 2;
        assert!(parse_query(&two).is_err());
        // compression pointer in the question
        let mut ptr = query(1, "a.example", TYPE_A);
        ptr[12] = 0xC0;
        assert!(parse_query(&ptr).is_err());
        // label length running past the packet
        let mut over = query(1, "a.example", TYPE_A);
        over[12] = 60;
        assert!(parse_query(&over).is_err());
        // bytes outside the host-name alphabet (space, slash, NUL, high bytes)
        for evil in ["a b.example", "a/b.example", "a\0b.example", "é.example"] {
            let mut q = Vec::from(&query(1, "x", TYPE_A)[..12]);
            for l in evil.split('.') {
                q.push(l.len() as u8);
                q.extend_from_slice(l.as_bytes());
            }
            q.extend_from_slice(&[0, 0, 1, 0, 1]);
            assert!(parse_query(&q).is_err(), "{evil:?}");
        }
        // non-IN class, non-standard opcode
        let mut chaos = query(1, "a.example", TYPE_A);
        let n = chaos.len();
        chaos[n - 1] = 3;
        assert_eq!(parse_query(&chaos).unwrap_err().1, Rcode::Refused);
        let mut update = query(1, "a.example", TYPE_A);
        update[2] = 0x28;
        assert_eq!(parse_query(&update).unwrap_err().1, Rcode::NotImp);
    }

    /// Build a response the way a real server would: compression pointers back to the
    /// question name.
    fn srv_response(id: u16, qname: &str, records: &[(u16, u16, u16, &str)], flags: u16) -> Vec<u8> {
        let mut p = build_srv_query(id, qname).unwrap();
        p[2..4].copy_from_slice(&flags.to_be_bytes());
        p[6..8].copy_from_slice(&(records.len() as u16).to_be_bytes());
        for (prio, weight, port, target) in records {
            p.extend_from_slice(&[0xC0, 0x0C]);
            p.extend_from_slice(&TYPE_SRV.to_be_bytes());
            p.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
            let mut rdata = Vec::new();
            rdata.extend_from_slice(&prio.to_be_bytes());
            rdata.extend_from_slice(&weight.to_be_bytes());
            rdata.extend_from_slice(&port.to_be_bytes());
            if *target == "." {
                rdata.push(0);
            } else {
                write_name(&mut rdata, target).unwrap();
            }
            p.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            p.extend_from_slice(&rdata);
        }
        p
    }

    #[test]
    fn srv_parsing_and_selection() {
        let q = "_minecraft._tcp.play.example.com";
        let pkt = srv_response(
            7,
            q,
            &[(10, 5, 25570, "b.host.example"), (5, 1, 25577, "A.Host.Example"), (5, 9, 25578, "c.host.example")],
            0x8180,
        );
        let recs = parse_srv_response(&pkt, 7, q).unwrap();
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0], Srv { priority: 5, weight: 9, port: 25578, target: "c.host.example".into() });
        assert_eq!(recs[1].target, "a.host.example", "targets are lower-cased");
        // NXDOMAIN and "." targets mean "no record"
        assert!(parse_srv_response(&srv_response(7, q, &[], 0x8183), 7, q).unwrap().is_empty());
        assert!(parse_srv_response(&srv_response(7, q, &[(0, 0, 1, ".")], 0x8180), 7, q).unwrap().is_empty());
    }

    #[test]
    fn srv_parsing_rejects_spoofs_and_garbage() {
        let q = "_minecraft._tcp.play.example.com";
        let good = srv_response(7, q, &[(0, 0, 25565, "h.example")], 0x8180);
        assert!(parse_srv_response(&good, 8, q).is_err(), "wrong transaction id");
        assert!(parse_srv_response(&good, 7, "_minecraft._tcp.other.example").is_err(), "answer for another name");
        assert!(parse_srv_response(&srv_response(7, q, &[], 0x8380), 7, q).is_err(), "truncated flag");
        assert!(parse_srv_response(&srv_response(7, q, &[], 0x0100), 7, q).is_err(), "not a response");
        assert!(parse_srv_response(&srv_response(7, q, &[], 0x8182), 7, q).is_err(), "SERVFAIL");
        // every truncation of a valid packet must fail cleanly, never panic
        for cut in 0..good.len() {
            let _ = parse_srv_response(&good[..cut], 7, q);
        }
        // forward/self-referencing compression pointer in the answer name
        let mut looped = good.clone();
        let ans = build_srv_query(7, q).unwrap().len();
        looped[ans] = 0xC0;
        looped[ans + 1] = ans as u8;
        assert!(parse_srv_response(&looped, 7, q).is_err());
        // a target with characters outside the host alphabet is dropped, not returned
        let mut weird = srv_response(7, q, &[(0, 0, 25565, "h.example")], 0x8180);
        let n = weird.len();
        weird[n - 8] = b' '; // first character of the "example" label
        assert!(parse_srv_response(&weird, 7, q).unwrap().is_empty());
    }

    #[test]
    fn query_builder_validates_names() {
        assert!(build_srv_query(1, "_minecraft._tcp.example.com").is_ok());
        assert!(build_srv_query(1, "").is_err());
        assert!(build_srv_query(1, "a..b").is_err());
        assert!(build_srv_query(1, &format!("{}.com", "x".repeat(64))).is_err());
    }
}
