//! One function per parser that reads bytes this program does not control.
//!
//! Each takes arbitrary bytes, runs the parser, and checks the properties that must hold
//! whatever the input is — above all that it returns at all (no panic, no hang), and for
//! the sanitisers that their output is stable: sanitising it again changes nothing.
//!
//! Two callers: `tests/robustness.rs`, which feeds them generated and mutated inputs on
//! every test run with the stable toolchain, and the `cargo fuzz` targets in `fuzz/`, which
//! do the same with coverage guidance on a nightly toolchain. Nothing here is used by the
//! launcher itself.
#![doc(hidden)]

use crate::{
    auth::minecraft,
    egress::{broker, dns, sni, udp},
    persist::{self, Kind},
    profile::zipdir as zip_directory,
    sandbox::{display, wayland},
};

/// The settings sanitisers (`options.txt`, `servers.dat`, other NBT, text, JSON): what
/// they produce must itself be accepted, and reproduce itself.
pub fn persist(data: &[u8]) {
    for kind in [Kind::Options, Kind::Text, Kind::Servers, Kind::Nbt, Kind::Json] {
        if let Ok((once, _)) = persist::sanitize(kind, data) {
            let (twice, report) =
                persist::sanitize(kind, &once).unwrap_or_else(|e| panic!("{kind:?}: own output refused: {e}"));
            assert_eq!(once, twice, "{kind:?}: sanitising is not idempotent");
            assert_eq!(report.dropped, 0, "{kind:?}: the sanitised form still contained something to drop");
            if kind == Kind::Json {
                assert!(once.iter().all(|b| (0x20..0x7f).contains(b)), "JSON came out with more than printable ASCII");
            }
        }
    }
}

/// The TLS ClientHello reader of the egress relay.
pub fn sni(data: &[u8]) {
    match sni::parse_client_hello(data) {
        sni::Hello::Complete(Some(name)) => {
            assert!(!name.is_empty() && name.len() <= 253, "{name:?}");
            assert!(name.bytes().all(|b| b.is_ascii_graphic()), "an SNI name with unprintable bytes: {name:?}");
        }
        sni::Hello::Complete(None) | sni::Hello::Incomplete | sni::Hello::Invalid => {}
    }
    // More bytes after a complete record never change the verdict about that record.
    if let sni::Hello::Complete(first) = sni::parse_client_hello(data) {
        let mut longer = data.to_vec();
        longer.extend_from_slice(&[0x17, 3, 3, 0, 1, 0]);
        assert_eq!(sni::parse_client_hello(&longer), sni::Hello::Complete(first));
    }
}

/// The DNS responder: queries from the sandbox, and SRV answers from the resolver.
pub fn dns(data: &[u8]) {
    match dns::parse_query(data) {
        Ok(query) => {
            assert!(query.name.len() <= 253 && query.name.bytes().all(|b| b.is_ascii_graphic()), "{:?}", query.name);
            // Every answer the responder can build for it is a well-formed packet.
            for packet in [
                dns::answer_a(&query, std::net::Ipv4Addr::new(127, 64, 0, 1)),
                dns::answer_empty(&query, dns::Rcode::NoError),
            ] {
                assert!(packet.len() >= 12 && packet.len() <= 512, "{} bytes", packet.len());
                assert_eq!(&packet[..2], &query.id.to_be_bytes());
            }
        }
        Err((id, rcode)) => {
            if let Some(id) = id {
                assert_eq!(dns::answer_error(id, rcode).len(), 12);
            }
        }
    }
    if let Ok(records) = dns::parse_srv_response(data, 0x1234, "_minecraft._tcp.play.example.org") {
        for srv in records {
            assert!(srv.target.len() <= 253, "{:?}", srv.target);
        }
    }
}

/// The broker's request classification and its join-body reader.
pub fn broker(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let canonical = broker::path_is_canonical(&text);
    if canonical {
        assert!(text.starts_with('/') && !text.contains("..") && !text.contains("//") && !text.contains('%'));
    }
    let _ = broker::parse_endpoint(&text);
    broker::fuzz_requests(data);
}

/// The Xauthority reducer: nothing or exactly one wildcard record.
pub fn xauthority(data: &[u8]) {
    for (display, host) in [(0u32, "host"), (7, ""), (u32::MAX, "a.b")] {
        let out = display::filter_xauthority(data, display, host);
        if !out.is_empty() {
            assert_eq!(&out[..2], &[0xff, 0xff], "not a wildcard record");
            assert_eq!(&out[2..6], &[0, 0, 0, 0], "an address or a display number was kept");
            assert!(out.len() <= data.len() + 8);
            // Its own output is a file it reduces to itself.
            assert_eq!(display::filter_xauthority(&out, display, host), out);
        }
    }
}

/// The Wayland event reader (registry announcements from a compositor).
pub fn wayland(data: &[u8]) {
    if let Ok(globals) = wayland::read_events_from(data) {
        assert!(globals.len() <= 512);
    }
}

/// Mojang's profile reply, and skin files.
pub fn profile(data: &[u8]) {
    if let Ok(profile) = minecraft::parse_profile(data) {
        assert!(profile.id.len() == 32 && profile.name.len() <= 25);
        for texture in profile.skins.iter().map(|s| &s.texture).chain(profile.capes.iter().map(|c| &c.texture)) {
            assert!(texture.bytes().all(|b| b.is_ascii_hexdigit()), "{texture:?}");
            assert!(minecraft::texture_url(texture).starts_with("https://textures.minecraft.net/texture/"));
        }
    }
    if let Ok(clean) = minecraft::sanitize_skin_png(data) {
        // A file of the launcher's own making: bounded whatever the input was, and its own
        // fixed point.
        assert!(clean.len() <= minecraft::MAX_SKIN_BYTES);
        assert_eq!(minecraft::sanitize_skin_png(&clean).expect("own output"), clean);
    }
}

/// The strict zip directory reader behind `pack import`: what it accepts has unique, safe
/// names, and another zip implementation that can read the same bytes lists the same ones.
pub fn zipdir(data: &[u8]) {
    zip_directory::fuzz(data);
}

/// The datagram relay's decisions (`egress::udp`). Not a parser — the relay never looks
/// into a datagram — but the same kind of exposure: which datagrams arrive, from which
/// source to which destination, how large, how fast and in what order relative to game
/// connections opening and closing is all the game's choice. Whatever the script, a
/// datagram is relayed only to the address of a game connection that is open at that
/// moment, under a rule that names the port, within the size, rate and flow limits.
pub fn udp(data: &[u8]) {
    udp::fuzz_table(data);
}

/// The companion channel: what code inside the game writes to the launcher's socket. The
/// same log lines and counts must result however the bytes are cut into reads, the reader
/// never holds more than one bounded line, and nothing it would log contains a control
/// character.
pub fn companion(data: &[u8]) {
    crate::companion::channel::fuzz(data);
}

/// A harness: arbitrary bytes in, a panic if a property does not hold.
pub type Harness = fn(&[u8]);

/// Every harness, by name (the names of the fuzz targets).
pub const ALL: &[(&str, Harness)] = &[
    ("persist", persist),
    ("sni", sni),
    ("dns", dns),
    ("broker", broker),
    ("xauthority", xauthority),
    ("wayland", wayland),
    ("profile", profile),
    ("zipdir", zipdir),
    ("udp", udp),
    ("companion", companion),
];
