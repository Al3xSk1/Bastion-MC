//! Generated and mutated inputs for every parser that reads bytes the launcher does not
//! control (`bastion_mc::fuzzing`). This is the part of fuzzing that runs everywhere, on
//! the stable toolchain, on every `cargo test`: no coverage guidance, but valid inputs for
//! each format as starting points and a few thousand mutations of each.
//!
//! Deterministic by default. For a longer or a different run:
//!   BASTION_FUZZ_ITERATIONS=200000 BASTION_FUZZ_SEED=42 cargo test --release --test robustness
//! Coverage-guided fuzzing of the same functions: see `fuzz/README.md`.

use bastion_mc::fuzzing;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

/// Bytes that are interesting to parsers of length-prefixed formats.
const MAGIC: [u8; 12] = [0x00, 0x01, 0x7f, 0x80, 0xff, 0xfe, 0x0a, 0x22, 0x5c, 0x2f, 0x3a, 0x7b];

fn mutate(rng: &mut Rng, seeds: &[Vec<u8>]) -> Vec<u8> {
    let mut data = seeds[rng.below(seeds.len())].clone();
    for _ in 0..1 + rng.below(4) {
        match rng.below(10) {
            0 if !data.is_empty() => {
                let at = rng.below(data.len());
                data[at] ^= 1 << rng.below(8);
            }
            1 if !data.is_empty() => {
                let at = rng.below(data.len());
                data[at] = MAGIC[rng.below(MAGIC.len())];
            }
            2 if !data.is_empty() => data.truncate(rng.below(data.len())),
            3 => {
                let at = rng.below(data.len() + 1);
                let extra: Vec<u8> = (0..1 + rng.below(16)).map(|_| rng.byte()).collect();
                data.splice(at..at, extra);
            }
            4 if data.len() > 1 => {
                // Repeat a piece somewhere else.
                let from = rng.below(data.len());
                let len = 1 + rng.below((data.len() - from).min(64));
                let piece = data[from..from + len].to_vec();
                let at = rng.below(data.len() + 1);
                data.splice(at..at, piece);
            }
            5 if data.len() > 1 => {
                let from = rng.below(data.len());
                let len = 1 + rng.below((data.len() - from).min(32));
                data.drain(from..from + len);
            }
            6 => {
                // Head of one valid input, tail of another.
                let other = &seeds[rng.below(seeds.len())];
                let cut = rng.below(data.len() + 1);
                data.truncate(cut);
                data.extend_from_slice(&other[rng.below(other.len() + 1)..]);
            }
            7 if data.len() >= 2 => {
                // A big-endian length field set to an extreme.
                let at = rng.below(data.len() - 1);
                let value: u16 = [0, 1, 0x7fff, 0x8000, 0xffff, data.len() as u16][rng.below(6)];
                data[at..at + 2].copy_from_slice(&value.to_be_bytes());
            }
            8 if data.len() >= 4 => {
                let at = rng.below(data.len() - 3);
                let value: u32 = [0, 1, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, data.len() as u32][rng.below(6)];
                let bytes = if rng.below(2) == 0 { value.to_be_bytes() } else { value.to_ne_bytes() };
                data[at..at + 4].copy_from_slice(&bytes);
            }
            _ => {
                // Nothing of the format at all.
                data = (0..rng.below(300)).map(|_| rng.byte()).collect();
            }
        }
    }
    data
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------ valid inputs

fn nbt_string(tag_name: &str, value: &str) -> Vec<u8> {
    let mut v = vec![8];
    v.extend_from_slice(&(tag_name.len() as u16).to_be_bytes());
    v.extend_from_slice(tag_name.as_bytes());
    v.extend_from_slice(&(value.len() as u16).to_be_bytes());
    v.extend_from_slice(value.as_bytes());
    v
}

fn servers_dat() -> Vec<u8> {
    // TAG_Compound "" { TAG_List "servers" of TAG_Compound { name, ip } }
    let mut v = vec![10, 0, 0, 9, 0, 7];
    v.extend_from_slice(b"servers");
    v.push(10);
    v.extend_from_slice(&2u32.to_be_bytes());
    for (name, ip) in [("Home", "play.example.org"), ("LAN", "192.0.2.7:25570")] {
        v.extend(nbt_string("name", name));
        v.extend(nbt_string("ip", ip));
        v.push(0);
    }
    v.push(0);
    v
}

fn client_hello(host: &str) -> Vec<u8> {
    let mut sni = vec![0];
    sni.extend_from_slice(&(host.len() as u16).to_be_bytes());
    sni.extend_from_slice(host.as_bytes());
    let mut extension = vec![0, 0];
    extension.extend_from_slice(&((sni.len() + 2) as u16).to_be_bytes());
    extension.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    extension.extend(sni);
    let mut body = vec![3, 3];
    body.extend_from_slice(&[0x11; 32]); // random
    body.push(0); // session id
    body.extend_from_slice(&[0, 2, 0x13, 0x01]); // cipher suites
    body.extend_from_slice(&[1, 0]); // compression
    body.extend_from_slice(&(extension.len() as u16).to_be_bytes());
    body.extend(extension);
    let mut handshake = vec![1, 0];
    handshake.extend_from_slice(&(body.len() as u16).to_be_bytes());
    handshake.extend(body);
    let mut record = vec![22, 3, 1];
    record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
    record.extend(handshake);
    record
}

fn dns_query(name: &str, qtype: u16) -> Vec<u8> {
    let mut v = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.split('.') {
        v.push(label.len() as u8);
        v.extend_from_slice(label.as_bytes());
    }
    v.push(0);
    v.extend_from_slice(&qtype.to_be_bytes());
    v.extend_from_slice(&[0, 1]);
    v
}

fn xauth_record(family: u16, fields: [&[u8]; 4]) -> Vec<u8> {
    let mut v = family.to_be_bytes().to_vec();
    for f in fields {
        v.extend_from_slice(&(f.len() as u16).to_be_bytes());
        v.extend_from_slice(f);
    }
    v
}

fn wayland_event(object: u32, opcode: u16, args: &[u8]) -> Vec<u8> {
    let mut v = object.to_ne_bytes().to_vec();
    v.extend_from_slice(&((((8 + args.len()) as u32) << 16) | u32::from(opcode)).to_ne_bytes());
    v.extend_from_slice(args);
    v
}

fn wayland_global(name: u32, interface: &str, version: u32) -> Vec<u8> {
    let mut args = name.to_ne_bytes().to_vec();
    args.extend_from_slice(&((interface.len() + 1) as u32).to_ne_bytes());
    args.extend_from_slice(interface.as_bytes());
    args.resize(args.len() + 1 + (4 - (interface.len() + 1) % 4) % 4, 0);
    args.extend_from_slice(&version.to_ne_bytes());
    wayland_event(2, 0, &args)
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = 0xffff_ffffu32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    let mut v = (data.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(data);
    v.extend_from_slice(&(!crc).to_be_bytes());
    v
}

/// A skin as an image editor writes it: `height` rows of 64 pixels of `bytes_per_pixel`
/// bytes (colour type 6 = RGBA, 3 = indexed), each row with a filter byte, compressed.
fn skin_png(height: u32, colour: u8) -> Vec<u8> {
    use std::io::Write;
    let bytes_per_pixel = if colour == 3 { 1 } else { 4 };
    let mut rows = Vec::new();
    for y in 0..height {
        // Every row filter there is — for RGBA, where any byte is a colour value. An indexed
        // image is left unfiltered, so that its bytes stay within its four-colour palette.
        rows.push(if colour == 3 { 0 } else { (y % 5) as u8 });
        rows.extend((0..64 * bytes_per_pixel).map(|x| ((x * 7 + y * 13) % 4) as u8));
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(&rows).unwrap();
    let pixels = encoder.finish().unwrap();

    let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut header = 64u32.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, colour, 0, 0, 0]);
    v.extend(png_chunk(b"IHDR", &header));
    v.extend(png_chunk(b"tEXt", b"Software\0editor"));
    if colour == 3 {
        v.extend(png_chunk(b"PLTE", &[0, 0, 0, 255, 255, 255, 200, 30, 30, 30, 200, 30]));
        v.extend(png_chunk(b"tRNS", &[0, 255]));
    }
    v.extend(png_chunk(b"IDAT", &pixels));
    v.extend(png_chunk(b"IEND", b""));
    v
}

/// A zip archive as an ordinary writer produces it (stored or deflated members).
fn zip_archive(entries: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
    use std::io::Write;
    let method = if deflate { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
    let options = zip::write::SimpleFileOptions::default().compression_method(method);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn seeds(harness: &str) -> Vec<Vec<u8>> {
    let text = |s: &str| s.as_bytes().to_vec();
    match harness {
        "persist" => vec![
            text(
                "version:4671\nlang:en_us\nfov:0.0\nsoundCategory_master:0.5\nkey_key.attack:key.mouse.left\nresourcePacks:[\"vanilla\",\"file/a.zip\"]\n",
            ),
            text("# a comment\nplain text file\nwith lines\n"),
            servers_dat(),
            vec![10, 0, 0, 0],
            text("{\"a\":[1,2,{\"b\":null}],\"c\":\"\\u00e9\",\"d\":1.5e3,\"e\":true}"),
            text("[]"),
        ],
        "sni" => vec![client_hello("sessionserver.mojang.com"), client_hello("a.b"), client_hello("")],
        "dns" => vec![
            dns_query("api.minecraftservices.com", 1),
            dns_query("_minecraft._tcp.play.example.org", 33),
            dns_query("x", 28),
            {
                // A response with one SRV record for the name the harness asks about.
                let mut v = dns_query("_minecraft._tcp.play.example.org", 33);
                v[2] = 0x81;
                v[3] = 0x80;
                v[7] = 1;
                v.extend_from_slice(&[0xc0, 0x0c, 0, 33, 0, 1, 0, 0, 0, 60, 0, 22, 0, 1, 0, 1, 0x63, 0xdd]);
                v.extend_from_slice(b"\x02mc\x07example\x03org\x00");
                v
            },
        ],
        "broker" => vec![
            text("/session/minecraft/join"),
            text("/player/certificates"),
            text("/minecraft/profile/skins"),
            text("/minecraft/profile/capes/active"),
            text("POST api.minecraftservices.com/some/new/endpoint"),
            text(
                "{\"accessToken\":\"bastion-placeholder-fuzz\",\"selectedProfile\":\"069a79f444e94726a5befca90e38aaf5\",\"serverId\":\"-1a2b3c4d5e6f\"}",
            ),
            text(
                "{\"serverId\":\"abc\",\"accessToken\":\"bastion-placeholder-fuzz\",\"selectedProfile\":\"069a79f444e94726a5befca90e38aaf5\"}",
            ),
        ],
        "xauthority" => vec![xauth_record(256, [b"host", b"0", b"MIT-MAGIC-COOKIE-1", b"0123456789abcdef"]), {
            let mut v = xauth_record(0, [b"\x7f\x00\x00\x01", b"0", b"MIT-MAGIC-COOKIE-1", b"stale-cookie-000"]);
            v.extend(xauth_record(256, [b"a.b", b"4294967295", b"MIT-MAGIC-COOKIE-1", b"0123456789abcdef"]));
            v.extend(xauth_record(0xffff, [b"", b"7", b"MIT-MAGIC-COOKIE-1", b"fedcba9876543210"]));
            v
        }],
        "wayland" => vec![
            {
                let mut v = wayland_global(1, "wl_compositor", 6);
                v.extend(wayland_global(2, "wp_security_context_manager_v1", 1));
                v.extend(wayland_global(3, "xdg_wm_base", 7));
                v.extend(wayland_event(3, 0, &7u32.to_ne_bytes())); // wl_callback.done
                v
            },
            {
                // wl_display.error(object, code, message)
                let mut args = 2u32.to_ne_bytes().to_vec();
                args.extend_from_slice(&1u32.to_ne_bytes());
                args.extend_from_slice(&8u32.to_ne_bytes());
                args.extend_from_slice(b"invalid\0");
                wayland_event(1, 0, &args)
            },
        ],
        "profile" => vec![
            text(
                "{\"id\":\"069a79f444e94726a5befca90e38aaf5\",\"name\":\"Notch\",\"skins\":[{\"id\":\"6a6e65e5-76dd-4c3c-a625-162924514568\",\"state\":\"ACTIVE\",\"url\":\"http://textures.minecraft.net/texture/1a4af718455d4aab\",\"variant\":\"SLIM\"}],\"capes\":[{\"id\":\"5af20372-79e0-4e1f-80f8-6bd8e3135995\",\"state\":\"INACTIVE\",\"url\":\"https://textures.minecraft.net/texture/2340c0e03dd24a11\",\"alias\":\"Migrator\"}]}",
            ),
            skin_png(64, 6),
            skin_png(32, 3),
        ],
        "zipdir" => vec![
            zip_archive(&[("pack.mcmeta", b"{\"pack\":{}}"), ("assets/minecraft/lang/en_us.json", b"{}")], false),
            zip_archive(
                &[("shaders/final.fsh", b"void main(){}"), ("shaders/", b""), ("readme \u{e9}.txt", b"x")],
                true,
            ),
            zip_archive(&[("a", b"")], false),
        ],
        other => panic!("no seeds for {other}"),
    }
}

fn iterations() -> usize {
    std::env::var("BASTION_FUZZ_ITERATIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(4000)
}

fn seed() -> u64 {
    std::env::var("BASTION_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x5eed_0bad_cafe_f00d)
}

fn run(name: &str) {
    let harness = fuzzing::ALL.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no harness {name}")).1;
    let seeds = seeds(name);
    // The valid inputs themselves, and every prefix of them.
    for input in &seeds {
        harness(input);
        for cut in 0..input.len().min(600) {
            harness(&input[..cut]);
        }
    }
    let mut rng = Rng(seed() ^ name.bytes().fold(0u64, |h, b| h.wrapping_mul(131).wrapping_add(u64::from(b))) | 1);
    for round in 0..iterations() {
        let input = mutate(&mut rng, &seeds);
        let outcome = std::panic::catch_unwind(|| harness(&input));
        if let Err(panic) = outcome {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            panic!(
                "harness {name} failed in round {round} (seed {}) on this {}-byte input:\n{}\n{message}",
                seed(),
                input.len(),
                hex(&input)
            );
        }
    }
}

#[test]
fn every_harness_has_a_test_here() {
    let tested = ["persist", "sni", "dns", "broker", "xauthority", "wayland", "profile", "zipdir"];
    let mut names: Vec<&str> = fuzzing::ALL.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    let mut expected = tested.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected, "a harness was added without a robustness test (or the other way round)");
}

#[test]
fn settings_sanitisers() {
    run("persist");
}

#[test]
fn tls_client_hello_reader() {
    run("sni");
}

#[test]
fn dns_responder() {
    run("dns");
}

#[test]
fn credential_broker_requests() {
    run("broker");
}

#[test]
fn xauthority_reducer() {
    run("xauthority");
}

#[test]
fn wayland_event_reader() {
    run("wayland");
}

#[test]
fn profile_reply_and_skin_files() {
    run("profile");
}

#[test]
fn zip_directory_reader() {
    run("zipdir");
}

/// Mutation only explores a parser if it starts from inputs the parser accepts.
#[test]
fn the_starting_points_are_accepted_by_their_parsers() {
    use bastion_mc::{
        auth::minecraft,
        egress::{dns, sni},
        persist::{self, Kind},
        sandbox::{display, wayland},
    };
    let (out, report) = persist::sanitize(Kind::Servers, &servers_dat()).expect("servers.dat");
    assert_eq!((report.kept, report.dropped), (2, 0));
    assert!(!out.is_empty());
    assert!(persist::sanitize(Kind::Options, &seeds("persist")[0]).unwrap().1.kept >= 5);
    assert!(persist::sanitize(Kind::Json, &seeds("persist")[4]).is_ok());
    assert_eq!(
        sni::parse_client_hello(&client_hello("sessionserver.mojang.com")),
        sni::Hello::Complete(Some("sessionserver.mojang.com".into()))
    );
    assert_eq!(dns::parse_query(&dns_query("api.minecraftservices.com", 1)).unwrap().name, "api.minecraftservices.com");
    let srv =
        dns::parse_srv_response(&seeds("dns")[3], 0x1234, "_minecraft._tcp.play.example.org").expect("SRV answer");
    assert_eq!((srv[0].port, srv[0].target.as_str()), (25565, "mc.example.org"));
    assert!(!display::filter_xauthority(&seeds("xauthority")[0], 0, "host").is_empty());
    assert!(!display::filter_xauthority(&seeds("xauthority")[1], 7, "").is_empty());
    let globals = wayland::read_events_from(&seeds("wayland")[0]).expect("registry");
    assert_eq!(globals.len(), 3);
    assert!(wayland::read_events_from(&seeds("wayland")[1]).unwrap_err().to_string().contains("protocol error 1"));
    for skin in [skin_png(64, 6), skin_png(32, 3)] {
        let clean = minecraft::sanitize_skin_png(&skin).expect("skin");
        assert!(skin.windows(4).any(|w| w == b"tEXt") && !clean.windows(4).any(|w| w == b"tEXt"));
    }
    let limits =
        bastion_mc::profile::zipdir::Limits { max_entries: 1000, max_directory_bytes: 1 << 20, max_name_bytes: 512 };
    for (archive, members) in seeds("zipdir").iter().zip([2, 3, 1]) {
        let listed = bastion_mc::profile::zipdir::read(archive.as_slice(), archive.len() as u64, &limits);
        assert_eq!(listed.expect("a zip archive").len(), members);
    }
}
