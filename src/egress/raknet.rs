//! RakNet's offline messages: what a Bedrock client and server exchange before a connection
//! exists — the server-list ping and pong, and the two-step "open connection" handshake in
//! which the client probes the path MTU.
//!
//! The datagram relay (`udp`) does not parse what a connection carries: once a flow exists its
//! datagrams go through as they are, like voice. It reads exactly one thing with this module:
//! **a flow to a Bedrock server opens only with a datagram that starts a RakNet exchange** — an
//! unconnected ping or an open connection request 1, well formed, with RakNet's magic. Any other
//! first datagram is refused. That keeps the relay a RakNet relay: code in the game cannot open
//! a flow to an allowlisted server's port with some other protocol, or reuse a flow that ended
//! by sending into the middle of a conversation. (What it does not do: check that the rest of
//! the flow is RakNet. A flow that was opened carries whatever the game sends, within the
//! relay's size and rate limits — the server is one the user listed, and the game can put
//! arbitrary bytes inside RakNet anyway.)
//!
//! The encoders are what a RakNet peer sends. The launcher never sends them; the tests' stand-in
//! server answers with them, and the harness checks that what the parser accepts survives a
//! round trip.
//!
//! Facts, from go-raknet at the commit gophertunnel pins (0d1fd09, read, not run): protocol
//! version 11; MTU probes of 1492, 1200 and 576 bytes (`dial.go`, `mtuSizes`), where a probe of
//! size M is an open connection request 1 padded to M − 28 bytes of UDP payload (the IPv4 and
//! UDP headers; `message/open_connection_request_1.go`); a client accepts a server's MTU up to
//! 1500 (`dial.go`, `discoverMTU`). Everything here is bounded by the datagram it reads.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

/// The sixteen bytes every offline message carries ("OFFLINE_MESSAGE_DATA_ID").
pub const MAGIC: [u8; 16] =
    [0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78];

/// The RakNet protocol version Bedrock speaks.
pub const PROTOCOL: u8 = 11;

/// IPv4 and UDP headers: an MTU of M bytes leaves M − 28 bytes for a datagram.
pub const HEADERS: usize = 28;

/// RakNet's largest MTU probe (`MAXIMUM_MTU_SIZE`) and the smallest MTU go-raknet accepts.
pub const MAX_MTU: u16 = 1492;
pub const MIN_MTU: u16 = 400;

/// The largest datagram of an MTU discovery: a probe of [`MAX_MTU`].
pub const MAX_PROBE: usize = MAX_MTU as usize - HEADERS;

/// The largest datagram the relay carries for Bedrock, in either direction: what fits a 1500-byte
/// Ethernet packet. Larger than anything a RakNet peer sends after an MTU discovery that started
/// at [`MAX_MTU`] (1464 bytes); a server that negotiates an MTU of 1500 — which go-raknet's
/// client accepts — sends up to this.
pub const MAX_DATAGRAM: usize = 1500 - HEADERS;

const ID_UNCONNECTED_PING: u8 = 0x01;
const ID_UNCONNECTED_PING_OPEN: u8 = 0x02;
const ID_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
const ID_OPEN_CONNECTION_REPLY_1: u8 = 0x06;
const ID_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;
const ID_OPEN_CONNECTION_REPLY_2: u8 = 0x08;
const ID_INCOMPATIBLE_PROTOCOL: u8 = 0x19;
const ID_UNCONNECTED_PONG: u8 = 0x1c;

/// The largest server description a pong may carry: whatever fits the largest datagram.
pub const MAX_PONG_DATA: usize = MAX_DATAGRAM - 35;

/// One offline message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offline {
    /// The server-list ping. `open_connections`: the variant that only servers with free slots
    /// answer (`0x02`).
    UnconnectedPing {
        open_connections: bool,
        time: u64,
        client_guid: u64,
    },
    /// The server's answer: its description (`MCPE;<motd>;<protocol>;<version>;…`), as bytes.
    UnconnectedPong {
        time: u64,
        server_guid: u64,
        data: Vec<u8>,
    },
    /// The first handshake step: padded to the MTU being probed.
    OpenConnectionRequest1 {
        protocol: u8,
        mtu: u16,
    },
    OpenConnectionReply1 {
        server_guid: u64,
        cookie: Option<u32>,
        mtu: u16,
    },
    /// The second step. `cookie` is present when the server asked for one in its reply 1.
    OpenConnectionRequest2 {
        cookie: Option<u32>,
        server: SocketAddr,
        mtu: u16,
        client_guid: u64,
    },
    OpenConnectionReply2 {
        server_guid: u64,
        client: SocketAddr,
        mtu: u16,
        security: bool,
    },
    IncompatibleProtocol {
        protocol: u8,
        server_guid: u64,
    },
}

/// Why a datagram is not an offline message this module reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malformed {
    Empty,
    /// Not one of the message ids above (a connected datagram, or something else).
    NotOffline(u8),
    BadMagic,
    BadLength,
    BadAddress,
    BadMtu,
}

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Malformed::Empty => f.write_str("an empty datagram"),
            Malformed::NotOffline(id) => write!(f, "not a RakNet offline message (first byte {id:#04x})"),
            Malformed::BadMagic => f.write_str("a RakNet message id without RakNet's magic bytes"),
            Malformed::BadLength => f.write_str("a RakNet offline message of the wrong length"),
            Malformed::BadAddress => f.write_str("a RakNet address of an unknown family"),
            Malformed::BadMtu => f.write_str("an open connection request for an MTU RakNet does not probe"),
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        if self.0.len() < n {
            return Err(Malformed::BadLength);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Malformed> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, Malformed> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, Malformed> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes(b.try_into().expect("eight bytes")))
    }
    fn magic(&mut self) -> Result<(), Malformed> {
        if self.take(16)? == MAGIC { Ok(()) } else { Err(Malformed::BadMagic) }
    }
    /// RakNet's system address: family 4 (the four octets inverted, then the port) or family 6
    /// (the platform's `sockaddr_in6` after the family byte: family, port, flow information,
    /// address, scope).
    fn address(&mut self) -> Result<SocketAddr, Malformed> {
        match self.u8()? {
            4 => {
                let b = self.take(4)?;
                let ip = Ipv4Addr::new(!b[0], !b[1], !b[2], !b[3]);
                Ok(SocketAddr::V4(SocketAddrV4::new(ip, self.u16()?)))
            }
            6 => {
                self.take(2)?; // the platform's AF_INET6, little-endian
                let port = self.u16()?;
                let flow = self.u32()?;
                let octets: [u8; 16] = self.take(16)?.try_into().expect("sixteen bytes");
                let scope = self.u32()?;
                Ok(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(octets), port, flow, scope)))
            }
            _ => Err(Malformed::BadAddress),
        }
    }
    fn end(&self) -> Result<(), Malformed> {
        if self.0.is_empty() { Ok(()) } else { Err(Malformed::BadLength) }
    }
}

/// Length of an encoded address of the family named by `family`.
fn address_len(family: u8) -> Option<usize> {
    match family {
        4 => Some(7),
        6 => Some(29),
        _ => None,
    }
}

fn put_address(out: &mut Vec<u8>, addr: &SocketAddr) {
    match addr {
        SocketAddr::V4(v4) => {
            out.push(4);
            out.extend(v4.ip().octets().iter().map(|b| !b));
            out.extend_from_slice(&v4.port().to_be_bytes());
        }
        SocketAddr::V6(v6) => {
            out.push(6);
            out.extend_from_slice(&23u16.to_le_bytes()); // what go-raknet writes (Windows' AF_INET6)
            out.extend_from_slice(&v6.port().to_be_bytes());
            out.extend_from_slice(&v6.flowinfo().to_be_bytes());
            out.extend_from_slice(&v6.ip().octets());
            out.extend_from_slice(&v6.scope_id().to_be_bytes());
        }
    }
}

/// Read one offline message. Strict: exact lengths, RakNet's magic, known address families, and
/// for an open connection request 1 a datagram whose size is an MTU RakNet can probe.
pub fn parse(datagram: &[u8]) -> Result<Offline, Malformed> {
    let (&id, rest) = datagram.split_first().ok_or(Malformed::Empty)?;
    let mut r = Reader(rest);
    let message = match id {
        ID_UNCONNECTED_PING | ID_UNCONNECTED_PING_OPEN => {
            let time = r.u64()?;
            r.magic()?;
            let client_guid = r.u64()?;
            r.end()?;
            Offline::UnconnectedPing { open_connections: id == ID_UNCONNECTED_PING_OPEN, time, client_guid }
        }
        ID_UNCONNECTED_PONG => {
            let time = r.u64()?;
            let server_guid = r.u64()?;
            r.magic()?;
            let len = usize::from(r.u16()?);
            if len > MAX_PONG_DATA {
                return Err(Malformed::BadLength);
            }
            let data = r.take(len)?.to_vec();
            r.end()?;
            Offline::UnconnectedPong { time, server_guid, data }
        }
        ID_OPEN_CONNECTION_REQUEST_1 => {
            r.magic()?;
            let protocol = r.u8()?;
            // The rest is padding up to the probed size; RakNet pads with zeros.
            if !r.0.iter().all(|b| *b == 0) {
                return Err(Malformed::BadLength);
            }
            if datagram.len() > MAX_PROBE || datagram.len() + HEADERS < usize::from(MIN_MTU) {
                return Err(Malformed::BadMtu);
            }
            let mtu = u16::try_from(datagram.len() + HEADERS).map_err(|_| Malformed::BadMtu)?;
            Offline::OpenConnectionRequest1 { protocol, mtu }
        }
        ID_OPEN_CONNECTION_REPLY_1 => {
            r.magic()?;
            let server_guid = r.u64()?;
            let cookie = match r.u8()? {
                0 => None,
                1 => Some(r.u32()?),
                _ => return Err(Malformed::BadLength),
            };
            let mtu = r.u16()?;
            r.end()?;
            Offline::OpenConnectionReply1 { server_guid, cookie, mtu }
        }
        ID_OPEN_CONNECTION_REQUEST_2 => {
            // Whether a cookie (and the client's "no challenge" byte) precedes the address
            // depends on the server's reply 1; the lengths tell the two forms apart.
            let after_magic = rest.len().checked_sub(16).ok_or(Malformed::BadLength)?;
            r.magic()?;
            let plain = r.0.first().and_then(|f| address_len(*f)).map(|a| a + 10);
            let cookie = if plain == Some(after_magic) {
                None
            } else {
                let cookie = r.u32()?;
                if r.u8()? != 0 {
                    return Err(Malformed::BadLength);
                }
                Some(cookie)
            };
            let server = r.address()?;
            let mtu = r.u16()?;
            let client_guid = r.u64()?;
            r.end()?;
            Offline::OpenConnectionRequest2 { cookie, server, mtu, client_guid }
        }
        ID_OPEN_CONNECTION_REPLY_2 => {
            r.magic()?;
            let server_guid = r.u64()?;
            let client = r.address()?;
            let mtu = r.u16()?;
            let security = match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(Malformed::BadLength),
            };
            r.end()?;
            Offline::OpenConnectionReply2 { server_guid, client, mtu, security }
        }
        ID_INCOMPATIBLE_PROTOCOL => {
            let protocol = r.u8()?;
            r.magic()?;
            let server_guid = r.u64()?;
            r.end()?;
            Offline::IncompatibleProtocol { protocol, server_guid }
        }
        other => return Err(Malformed::NotOffline(other)),
    };
    Ok(message)
}

impl Offline {
    /// The datagram a RakNet peer sends for this message.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        match self {
            Offline::UnconnectedPing { open_connections, time, client_guid } => {
                out.push(if *open_connections { ID_UNCONNECTED_PING_OPEN } else { ID_UNCONNECTED_PING });
                out.extend_from_slice(&time.to_be_bytes());
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&client_guid.to_be_bytes());
            }
            Offline::UnconnectedPong { time, server_guid, data } => {
                out.push(ID_UNCONNECTED_PONG);
                out.extend_from_slice(&time.to_be_bytes());
                out.extend_from_slice(&server_guid.to_be_bytes());
                out.extend_from_slice(&MAGIC);
                let data = &data[..data.len().min(MAX_PONG_DATA)];
                out.extend_from_slice(&(data.len() as u16).to_be_bytes());
                out.extend_from_slice(data);
            }
            Offline::OpenConnectionRequest1 { protocol, mtu } => {
                out.push(ID_OPEN_CONNECTION_REQUEST_1);
                out.extend_from_slice(&MAGIC);
                out.push(*protocol);
                let size = usize::from(*mtu).saturating_sub(HEADERS).max(out.len());
                out.resize(size, 0);
            }
            Offline::OpenConnectionReply1 { server_guid, cookie, mtu } => {
                out.push(ID_OPEN_CONNECTION_REPLY_1);
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&server_guid.to_be_bytes());
                match cookie {
                    Some(cookie) => {
                        out.push(1);
                        out.extend_from_slice(&cookie.to_be_bytes());
                    }
                    None => out.push(0),
                }
                out.extend_from_slice(&mtu.to_be_bytes());
            }
            Offline::OpenConnectionRequest2 { cookie, server, mtu, client_guid } => {
                out.push(ID_OPEN_CONNECTION_REQUEST_2);
                out.extend_from_slice(&MAGIC);
                if let Some(cookie) = cookie {
                    out.extend_from_slice(&cookie.to_be_bytes());
                    out.push(0);
                }
                put_address(&mut out, server);
                out.extend_from_slice(&mtu.to_be_bytes());
                out.extend_from_slice(&client_guid.to_be_bytes());
            }
            Offline::OpenConnectionReply2 { server_guid, client, mtu, security } => {
                out.push(ID_OPEN_CONNECTION_REPLY_2);
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&server_guid.to_be_bytes());
                put_address(&mut out, client);
                out.extend_from_slice(&mtu.to_be_bytes());
                out.push(u8::from(*security));
            }
            Offline::IncompatibleProtocol { protocol, server_guid } => {
                out.push(ID_INCOMPATIBLE_PROTOCOL);
                out.push(*protocol);
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&server_guid.to_be_bytes());
            }
        }
        out
    }

    /// What this is, for one log line.
    pub fn describe(&self) -> String {
        match self {
            Offline::UnconnectedPing { .. } => "RakNet ping (server list)".into(),
            Offline::UnconnectedPong { .. } => "RakNet pong".into(),
            Offline::OpenConnectionRequest1 { mtu, protocol } => {
                format!("RakNet open connection request 1 (MTU {mtu}, protocol {protocol})")
            }
            Offline::OpenConnectionReply1 { .. } => "RakNet open connection reply 1".into(),
            Offline::OpenConnectionRequest2 { .. } => "RakNet open connection request 2".into(),
            Offline::OpenConnectionReply2 { .. } => "RakNet open connection reply 2".into(),
            Offline::IncompatibleProtocol { .. } => "RakNet incompatible protocol".into(),
        }
    }
}

/// May this datagram, sent by the game, open a flow? Only the two messages a RakNet client
/// starts with: a ping, or an open connection request 1.
pub fn opens_flow(datagram: &[u8]) -> bool {
    matches!(parse(datagram), Ok(Offline::UnconnectedPing { .. } | Offline::OpenConnectionRequest1 { .. }))
}

/// For `fuzzing::raknet`: whatever the bytes, what parses is bounded, survives a round trip
/// through the encoder unchanged in meaning, and only a ping or a request 1 opens a flow.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    match parse(data) {
        Ok(message) => {
            let again = message.encode();
            assert_eq!(parse(&again).as_ref(), Ok(&message), "a message does not survive its own encoding");
            // Encodings are canonical except for an IPv6 address's family field.
            if !matches!(
                message,
                Offline::OpenConnectionRequest2 { server: SocketAddr::V6(_), .. }
                    | Offline::OpenConnectionReply2 { client: SocketAddr::V6(_), .. }
            ) {
                assert_eq!(again, data, "the encoding is not the datagram that was read");
            }
            if opens_flow(data) {
                assert!(data.len() <= MAX_PROBE, "a {}-byte datagram opened a flow", data.len());
                assert_eq!(&data[1 + usize::from(data[0] <= 2) * 8..][..16], &MAGIC);
            }
            if let Offline::UnconnectedPong { data: motd, .. } = &message {
                assert!(motd.len() <= MAX_PONG_DATA);
            }
            assert!(!message.describe().is_empty());
        }
        Err(_) => assert!(!opens_flow(data)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_ping_is_exactly_thirty_three_bytes_with_the_magic() {
        let ping = Offline::UnconnectedPing { open_connections: false, time: 0x0102_0304_0506_0708, client_guid: 42 };
        let bytes = ping.encode();
        assert_eq!(bytes.len(), 33);
        assert_eq!(bytes[0], 0x01);
        assert_eq!(&bytes[9..25], &MAGIC);
        assert_eq!(parse(&bytes), Ok(ping));
        assert!(opens_flow(&bytes));
        // One byte more or less, or a damaged magic, is no ping.
        assert_eq!(parse(&bytes[..32]), Err(Malformed::BadLength));
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(parse(&longer), Err(Malformed::BadLength));
        let mut damaged = bytes.clone();
        damaged[12] ^= 1;
        assert_eq!(parse(&damaged), Err(Malformed::BadMagic));
        assert!(!opens_flow(&damaged) && !opens_flow(&longer));
    }

    #[test]
    fn a_request_one_is_padded_to_each_probed_mtu_and_nothing_larger_opens_a_flow() {
        // go-raknet's probes: 1492, 1200, 576 — datagrams of the MTU less 28 bytes.
        for mtu in [1492u16, 1200, 576, MIN_MTU] {
            let bytes = Offline::OpenConnectionRequest1 { protocol: PROTOCOL, mtu }.encode();
            assert_eq!(bytes.len(), usize::from(mtu) - HEADERS);
            assert_eq!(parse(&bytes), Ok(Offline::OpenConnectionRequest1 { protocol: PROTOCOL, mtu }));
            assert!(opens_flow(&bytes), "MTU {mtu}");
        }
        assert_eq!(MAX_PROBE, 1464);
        let mut too_big = Offline::OpenConnectionRequest1 { protocol: PROTOCOL, mtu: MAX_MTU }.encode();
        too_big.push(0);
        assert_eq!(parse(&too_big), Err(Malformed::BadMtu));
        let too_small = Offline::OpenConnectionRequest1 { protocol: PROTOCOL, mtu: MIN_MTU - 1 }.encode();
        assert_eq!(parse(&too_small), Err(Malformed::BadMtu));
        // Padding that is not zeros is something else wearing RakNet's header.
        let mut stuffed = Offline::OpenConnectionRequest1 { protocol: PROTOCOL, mtu: 576 }.encode();
        stuffed[100] = b'x';
        assert!(parse(&stuffed).is_err() && !opens_flow(&stuffed));
    }

    #[test]
    fn the_rest_of_the_handshake_round_trips_with_and_without_a_cookie() {
        let server = v4("127.64.0.5:19132");
        for message in [
            Offline::UnconnectedPong { time: 7, server_guid: 9, data: b"MCPE;stand-in;800;1.26.62;0;10".to_vec() },
            Offline::OpenConnectionReply1 { server_guid: 9, cookie: Some(0xdead_beef), mtu: 1400 },
            Offline::OpenConnectionReply1 { server_guid: 9, cookie: None, mtu: 1400 },
            Offline::OpenConnectionRequest2 { cookie: Some(0xdead_beef), server, mtu: 1400, client_guid: 1 << 63 },
            Offline::OpenConnectionRequest2 { cookie: None, server, mtu: 1400, client_guid: 1 << 63 },
            Offline::OpenConnectionReply2 { server_guid: 9, client: v4("127.0.0.1:40000"), mtu: 1400, security: false },
            Offline::IncompatibleProtocol { protocol: 10, server_guid: 9 },
            Offline::UnconnectedPing { open_connections: true, time: 1, client_guid: 2 },
        ] {
            let bytes = message.encode();
            assert_eq!(parse(&bytes), Ok(message.clone()), "{message:?}");
            assert_eq!(opens_flow(&bytes), matches!(message, Offline::UnconnectedPing { .. }), "{message:?}");
        }
        // An IPv4 address is written inverted, as RakNet does.
        let bytes = Offline::OpenConnectionRequest2 { cookie: None, server, mtu: 1400, client_guid: 1 }.encode();
        assert_eq!(&bytes[17..22], &[4, !127, !64, !0, !5]);
        // An IPv6 address round-trips too.
        let v6: SocketAddr = "[2001:db8::7]:19133".parse().unwrap();
        let message = Offline::OpenConnectionReply2 { server_guid: 1, client: v6, mtu: 1200, security: true };
        assert_eq!(parse(&message.encode()), Ok(message));
    }

    #[test]
    fn connected_datagrams_and_garbage_are_not_offline_messages() {
        assert_eq!(parse(&[]), Err(Malformed::Empty));
        // A connected RakNet datagram (frame set) starts with 0x80..0x8f.
        assert_eq!(parse(&[0x84, 0, 0, 0]), Err(Malformed::NotOffline(0x84)));
        assert_eq!(parse(b"GET / HTTP/1.1\r\n"), Err(Malformed::NotOffline(b'G')));
        let mut wrong_family =
            Offline::OpenConnectionRequest2 { cookie: None, server: v4("1.2.3.4:5"), mtu: 1400, client_guid: 1 }
                .encode();
        wrong_family[17] = 5;
        assert!(parse(&wrong_family).is_err());
        let mut long_pong = Offline::UnconnectedPong { time: 1, server_guid: 2, data: b"abc".to_vec() }.encode();
        long_pong[33] = 0xff; // a length beyond the datagram
        assert_eq!(parse(&long_pong), Err(Malformed::BadLength));
        for text in [Malformed::Empty, Malformed::NotOffline(0x84), Malformed::BadMtu] {
            assert!(!text.to_string().is_empty());
        }
    }

    #[test]
    fn the_harness_holds_for_every_message_and_its_prefixes() {
        let samples = [
            Offline::UnconnectedPing { open_connections: false, time: 1, client_guid: 2 }.encode(),
            Offline::OpenConnectionRequest1 { protocol: PROTOCOL, mtu: 576 }.encode(),
            Offline::OpenConnectionRequest2 {
                cookie: Some(5),
                server: v4("127.64.0.9:19132"),
                mtu: 576,
                client_guid: 3,
            }
            .encode(),
            Offline::UnconnectedPong { time: 1, server_guid: 2, data: vec![b'm'; 40] }.encode(),
        ];
        for sample in samples {
            for cut in 0..=sample.len() {
                fuzz(&sample[..cut]);
            }
        }
        assert_eq!(MAX_DATAGRAM, 1472);
    }
}
