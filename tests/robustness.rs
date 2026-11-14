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

/// A class file whose constant pool names its own class, `java.net.Socket`,
/// `Runtime.exec` and the string "sun.misc.Unsafe".
fn class_file() -> Vec<u8> {
    let utf8 = |text: &str| {
        let mut entry = vec![1];
        entry.extend_from_slice(&(text.len() as u16).to_be_bytes());
        entry.extend_from_slice(text.as_bytes());
        entry
    };
    let pool: Vec<Vec<u8>> = vec![
        utf8("mod/Main"),          // 1
        vec![7, 0, 1],             // 2: class mod/Main
        utf8("java/net/Socket"),   // 3
        vec![7, 0, 3],             // 4
        utf8("java/lang/Runtime"), // 5
        vec![7, 0, 5],             // 6
        utf8("exec"),              // 7
        utf8("()V"),               // 8
        vec![12, 0, 7, 0, 8],      // 9: name and type
        vec![10, 0, 6, 0, 9],      // 10: Runtime.exec
        utf8("sun.misc.Unsafe"),   // 11
        vec![8, 0, 11],            // 12: a string
    ];
    let mut out = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52];
    out.extend_from_slice(&((pool.len() + 1) as u16).to_be_bytes());
    out.extend(pool.into_iter().flatten());
    out.extend_from_slice(&[0, 0x21, 0, 2]); // access flags, this class
    out.extend_from_slice(&[0; 10]); // super class, interfaces, fields, methods, attributes
    out
}

/// A jar with the given entries, deflated or stored: a zip archive like any other.
fn jar(entries: &[(&str, &[u8])], deflated: bool) -> Vec<u8> {
    zip_archive(entries, deflated)
}

const MOD_JSON: &[u8] =
    br#"{"schemaVersion":1,"id":"example-mod","version":"1.0","depends":{"fabricloader":">=0.19"}}"#;

/// A mod jar as Fabric mods are built: metadata, a class, a native library, and a jar inside.
fn mod_jar(deflated: bool) -> Vec<u8> {
    let inner = jar(&[("lib/Helper.class", &class_file()), ("natives/libhelper.so", b"\x7fELF")], deflated);
    jar(
        &[
            ("fabric.mod.json", MOD_JSON),
            ("mod/Main.class", &class_file()),
            ("META-INF/jars/helper-1.2.jar", &inner),
            ("assets/example/icon.png", &[1, 2, 3]),
        ],
        deflated,
    )
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
        "udp" => {
            // Scripts for the datagram relay's decision table, four bytes an event (see
            // `egress::udp::fuzz_table`): 1 = a game connection opens (rule, address),
            // 2 = one closes, 0 = time passes, 3 = a datagram from the server, 4 = idle
            // flows end, anything else = a datagram from the game (source, destination,
            // port and size).
            let script = |events: &[[u8; 4]]| events.as_flattened().to_vec();
            vec![
                // A session: connect, talk both ways, fall silent, close.
                script(&[[1, 0, 0, 0], [5, 0, 0, 0], [5, 0, 0, 3], [3, 0, 10, 0], [0, 100, 0, 0], [5, 0, 0, 6]]),
                // The limits: a burst on one flow, an oversize datagram, more sockets than flows.
                script(&[
                    [1, 0, 0, 0],
                    [5, 0, 0, 0],
                    [5, 0, 0, 0],
                    [5, 0, 0, 0],
                    [5, 0, 0, 0],
                    [5, 0, 0, 0],
                    [5, 0, 0, 0],
                    [5, 0, 0, 51],
                    [5, 1, 0, 0],
                    [5, 2, 0, 0],
                    [5, 3, 0, 0],
                    [3, 0, 60, 0],
                    [3, 1, 5, 0],
                ]),
                // Rules: private addresses with and without the flag, a literal address, the
                // other server's port, a server without a UDP rule, a service name, nobody.
                script(&[
                    [1, 1, 1, 0],
                    [5, 0, 1, 0],
                    [5, 1, 1, 1],
                    [1, 0, 1, 0],
                    [5, 2, 0, 0],
                    [1, 3, 0, 0],
                    [5, 3, 4, 0],
                    [5, 4, 0, 1],
                    [1, 2, 0, 0],
                    [5, 5, 2, 0],
                    [5, 0, 3, 0],
                    [5, 0, 5, 0],
                    [5, 0, 6, 0],
                ]),
                // Endings: silence, the idle check, the connection closing, and what comes after.
                script(&[
                    [1, 0, 2, 0],
                    [5, 0, 0, 0],
                    [0, 255, 0, 0],
                    [0, 255, 0, 0],
                    [4, 0, 0, 0],
                    [5, 0, 0, 0],
                    [1, 0, 0, 0],
                    [5, 1, 0, 0],
                    [2, 0, 0, 0],
                    [5, 0, 0, 0],
                    [2, 0, 0, 0],
                    [5, 1, 0, 0],
                ]),
            ]
        }
        "companion" => vec![
            text("hello 1 0.1.0 zoom,hud,fullbright\n"),
            text("hello 1 0.1.0 -\ndiag zoom disabled: lookup failed (NoSuchMethodException: Camera.calculateFov)\n"),
            text("diag hud disabled: the HUD hook failed (ClassCastException)\r\ndiag fullbright off\n\n"),
            {
                // A line far beyond the limit, then an ordinary one.
                let mut v = vec![b'A'; 3000];
                v.extend_from_slice(b"\nhello 1 0.1.0 zoom\n");
                v
            },
            text("diag zoom \u{1b}[2J\u{1b}]0;title\u{7}\u{202e}text\n").repeat(40),
        ],
        "phone" => {
            use bastion_mc::companion::channel::{KIND_COMMAND, KIND_DIAG, frame};
            let mut many = Vec::new();
            for i in 0..40 {
                many.extend(frame(KIND_COMMAND, format!("spotify play {i}").as_bytes()));
            }
            vec![
                frame(KIND_COMMAND, b"spotify play"),
                [frame(KIND_COMMAND, b"spotify search -- Daft Punk"), frame(KIND_DIAG, b"diag zoom disabled: no")]
                    .concat(),
                [frame(KIND_COMMAND, b"browser open discord"), frame(KIND_COMMAND, b"spotify wipe")].concat(),
                frame(KIND_COMMAND, b"spotify play \xff\n\x1b[2J"),
                // A kind the mod never sends, and a length over the limit, each mid-stream.
                [frame(KIND_COMMAND, b"spotify play"), frame(0x81, b"spotify\0{}")].concat(),
                [0, 0, 0x10, 0, KIND_COMMAND].to_vec(),
                [0, 0, 0, 0, KIND_COMMAND].to_vec(),
                many,
            ]
        }
        "registry" => {
            // A build, a project, a list of builds and a hash lookup, shaped like Modrinth's answers.
            let sha512 = "090e8b47b413523b8682c3953beb6d99e5a92473a8d0580bdbf6a057352aadb8ad8b7a2d08f36c99c14ed3294923102c45724c88b24d697a05b729925ce03de3";
            let version = format!(
                "{{\"game_versions\":[\"26.2\",\"26.3\"],\"loaders\":[\"fabric\"],\"id\":\"z1FVDMnq\",\"project_id\":\"YlKdE5VK\",\"version_number\":\"0.4.22+188-26.x\",\"changelog\":null,\"date_published\":\"2026-10-22T19:11:11.566967Z\",\"downloads\":7,\"version_type\":\"alpha\",\"status\":\"listed\",\"files\":[{{\"hashes\":{{\"sha1\":\"602797ada2a9bcc4aaecbfdd2f89e4c9397787cc\",\"sha512\":\"{sha512}\"}},\"url\":\"https://cdn.modrinth.com/data/YlKdE5VK/versions/z1FVDMnq/ViaFabric-0.4.22%2B188-26.x.jar\",\"filename\":\"ViaFabric-0.4.22+188-26.x.jar\",\"primary\":true,\"size\":5916758,\"file_type\":null}}],\"dependencies\":[{{\"version_id\":null,\"project_id\":\"P7dR8mSH\",\"file_name\":null,\"dependency_type\":\"required\"}},{{\"version_id\":\"Zns5SSRA\",\"project_id\":null,\"file_name\":\"x.jar\",\"dependency_type\":\"optional\"}}]}}"
            );
            vec![
                text(&version),
                text(
                    "{\"id\":\"YlKdE5VK\",\"slug\":\"viafabric\",\"project_type\":\"mod\",\"title\":\"ViaFabric\",\"status\":\"approved\",\"license\":{\"id\":\"GPL-3.0-only\",\"name\":\"GNU General Public License v3.0 only\",\"url\":null},\"versions\":[\"z1FVDMnq\"]}",
                ),
                text(&format!("[{version},{version}]")),
                text(&format!("{{\"{sha512}\":{version}}}")),
                text("2026-10-22T19:11:11.566967Z"),
                text("ViaFabric-0.4.22%2B188-26.x.jar"),
                text("https://cdn.modrinth.com/data/AABBCCDD/versions/z1FVDMnq/a.jar"),
            ]
        }
        // Stored as well as deflated: in a stored archive a mutation lands in the entry it
        // hits; in a deflated one it mostly breaks the stream.
        "jar" => vec![
            mod_jar(false),
            mod_jar(true),
            class_file(),
            MOD_JSON.to_vec(),
            text(r#"{"schema_version":1,"quilt_loader":{"id":"example_mod","version":"1.0"}}"#),
        ],
        "realms" => vec![
            text("sid=token:bastion-placeholder-fuzz:069a79f444e94726a5befca90e38aaf5;user=Steve;version=26.3"),
            text("sid=token:bastion-placeholder-fuzz:069a79f4-44e9-4726-a5be-fca90e38aaf5; user=Alex; version=1.20.1"),
            text(
                r#"{"address":"203.0.113.9:31234","resourcePackUrl":null,"resourcePackHash":null,"sessionRegionData":{"regionName":"EU"}}"#,
            ),
            text(r#"{"servers":[{"id":1,"name":"Our \u001b[31mRealm","state":"OPEN"},{"id":2,"name":"Other"}]}"#),
            text("/worlds/v1/123456/join/pc"),
            text("/invites/accept/a1b2-c3"),
            text("/api/v1.0/invite/codes/use/AbC-123"),
            text("[2001:db8::1]:25566"),
            {
                let mut request = vec![1u8, 4, 203, 0, 113, 9];
                request.extend_from_slice(&[0; 12]);
                request.extend_from_slice(&31234u16.to_be_bytes());
                request
            },
        ],
        "sync" => {
            let options = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/options/1.20.1.txt")).unwrap();
            let copy = r#"{"format":"bastion-mc/sync/v1","settings":{"fov":{"value":"0.25","data_version":5023,"changed":1,"instance":"fab"},"cutoutLeaves":{"value":"false","data_version":5023,"changed":2,"instance":"fab"},"weatherRadius":{"value":"5","data_version":5023,"changed":2,"instance":"fab"},"vignette":{"value":"false","data_version":5023,"changed":2,"instance":"fab"},"improvedTransparency":{"value":"false","data_version":5023,"changed":2,"instance":"fab"},"key_key.jump":{"value":"key.keyboard.j","data_version":4189,"changed":3,"instance":"v"},"menuBackgroundBlurriness":{"value":"3","data_version":5023,"changed":4,"instance":"fab"}}}"#;
            let mut both = options.clone();
            both.push(0);
            both.extend_from_slice(copy.as_bytes());
            let modern = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/options/26.3.txt")).unwrap();
            let mut modern_both = modern.clone();
            modern_both.push(0);
            modern_both.extend_from_slice(copy.as_bytes());
            vec![both, modern_both, options, text(copy)]
        }
        "dbus" => {
            use bastion_mc::spotify::dbus::{Kind, Message, Value};
            let mut call = Message::call(
                ":1.7",
                "/org/mpris/MediaPlayer2",
                "org.freedesktop.DBus.Properties",
                "GetAll",
                vec![Value::Str("org.mpris.MediaPlayer2.Player".into())],
            );
            call.serial = 3;
            let metadata = Value::string_dict(vec![
                ("mpris:trackid".into(), Value::Path("/com/spotify/track/4uLU6hMCjMI75M1A2tKUQC".into())),
                ("mpris:length".into(), Value::U64(213_000_000)),
                (
                    "xesam:artist".into(),
                    Value::Array { element: "s".into(), items: vec![Value::Str("Daft Punk".into())] },
                ),
                ("xesam:title".into(), Value::Str("Harder, Better".into())),
            ]);
            let mut signal = Message {
                kind: Kind::Signal,
                flags: 0,
                serial: 9,
                path: Some("/org/mpris/MediaPlayer2".into()),
                interface: Some("org.freedesktop.DBus.Properties".into()),
                member: Some("PropertiesChanged".into()),
                error_name: None,
                reply_serial: None,
                destination: None,
                sender: Some(":1.42".into()),
                body: vec![
                    Value::Str("org.mpris.MediaPlayer2.Player".into()),
                    Value::string_dict(vec![("Metadata".into(), metadata), ("Volume".into(), Value::Double(0.5))]),
                    Value::Array { element: "s".into(), items: vec![] },
                ],
            };
            let mut error = signal.clone();
            error.kind = Kind::Error;
            error.error_name = Some("org.freedesktop.DBus.Error.ServiceUnknown".into());
            error.reply_serial = Some(3);
            error.body = vec![Value::Str("no such name".into())];
            signal.serial = 10;
            vec![call.encode().unwrap(), signal.encode().unwrap(), error.encode().unwrap()]
        }
        "spotify" => vec![
            text(
                r#"{"device":{"id":"5fbb3ba6aa454b5534c4ba43a8c7e8e45a63ad0e","is_active":true,"name":"Kitchen","volume_percent":59},"shuffle_state":false,"repeat_state":"context","progress_ms":44272,"item":{"album":{"images":[{"url":"https://i.scdn.co/image/ab67616d00001e024ce8b4e42588bf18182a1ad2","width":300}],"name":"Discovery"},"artists":[{"name":"Daft Punk"}],"duration_ms":224693,"name":"Harder","type":"track","uri":"spotify:track:5W3cjX2J3tjhG8zb6u0qHn"},"is_playing":true}"#,
            ),
            text(
                r#"{"devices":[{"id":"abc123","is_active":true,"name":"Phone","type":"Smartphone","volume_percent":50}]}"#,
            ),
            text(
                r#"{"tracks":{"items":[{"uri":"spotify:track:5W3cjX2J3tjhG8zb6u0qHn","name":"Harder","artists":[{"name":"Daft Punk"}],"album":{"name":"Discovery"}}]},"playlists":{"items":[null,{"uri":"spotify:playlist:37i9dQZF1DXcBWIGoYBM5M","name":"Mix","owner":{"display_name":"Spotify"}}]}}"#,
            ),
            text(
                r#"{"access_token":"BQD-access","token_type":"Bearer","scope":"user-read-playback-state","expires_in":3600,"refresh_token":"AQD-refresh"}"#,
            ),
            text(
                r#"{"error":{"status":403,"message":"Player command failed: Premium required","reason":"PREMIUM_REQUIRED"}}"#,
            ),
            text(r#"{"error":"invalid_grant","error_description":"Refresh token revoked"}"#),
            text("GET /callback?code=AQC-code&state=GOOD-STATE HTTP/1.1\r\nHost: 127.0.0.1:43117"),
            text("GET /callback?error=access_denied&state=GOOD-STATE HTTP/1.1"),
        ],
        "keeper" => vec![
            vec![0, 0, 0, 14, 16, 0, 9, b'B', b'Q', b'D', b'-', b't', b'o', b'k', b'e', b'n'],
            [vec![1, 3, 0, 0, 1, 247, 0, 4], b"busy".to_vec()].concat(),
            vec![1, 4, 0, 0, 0, 30, 0, 0],
        ],
        "picture" => vec![bastion_mc::util::b64_decode(TINY_JPEG).unwrap()],
        "clips" => {
            let mut packets = Vec::new();
            for (pid, cc) in [(0u16, 0u8), (4096, 0), (256, 0), (256, 1), (0x1fff, 0)] {
                let mut p = vec![0xffu8; 188];
                p[0] = 0x47;
                p[1] = (pid >> 8) as u8;
                p[2] = pid as u8;
                p[3] = 0x10 | cc;
                packets.extend(p);
            }
            vec![
                text("r1_004.ts,4.000000,5.000000\nr1_000.ts,5.000000,6.000000\nr1_001.ts,6.000000,7.000000\n"),
                packets,
                text(
                    r#"{"version":1,"clips":[{"id":"0123456789ab","name":"a b","file":"a_b-0123456789ab.ts","thumb":"a_b-0123456789ab.png","created":1,"seconds":1.5,"bytes":188}]}"#,
                ),
                text("waiting\nrecording 480 270\nstopped window-changed\ngave-up\n"),
            ]
        }
        "x11" => {
            let mut tree = vec![1, 0, 5, 0, 2, 0, 0, 0];
            tree.extend_from_slice(&[0; 8]);
            tree.extend_from_slice(&2u16.to_le_bytes());
            tree.extend_from_slice(&[0; 14]);
            tree.extend_from_slice(&0x400037u32.to_le_bytes());
            tree.extend_from_slice(&0x40003du32.to_le_bytes());
            let mut property = vec![1, 8, 0, 0, 2, 0, 0, 0];
            property.extend_from_slice(&31u32.to_le_bytes());
            property.extend_from_slice(&0u32.to_le_bytes());
            property.extend_from_slice(&7u32.to_le_bytes());
            property.extend_from_slice(&[0; 12]);
            property.extend_from_slice(b"bastion\0");
            let mut ids = vec![1, 0, 0, 0, 4, 0, 0, 0];
            ids.extend_from_slice(&1u32.to_le_bytes());
            ids.extend_from_slice(&[0; 20]);
            for word in [0x400000u32, 2, 4, 4242] {
                ids.extend_from_slice(&word.to_le_bytes());
            }
            vec![tree, property, ids, xauth_record(0xffff, [b"", b"", b"MIT-MAGIC-COOKIE-1", &[9; 16]])]
        }
        "websocket" => {
            // Server frames as Firefox's remote control sends them: unmasked text, a message
            // in fragments with a ping between them, a 16-bit and a 64-bit length, a close.
            let frame = |fin: bool, opcode: u8, payload: &[u8]| {
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
            };
            let reply = br#"{"type":"success","id":1,"result":{"sessionId":"x","capabilities":{}}}"#;
            vec![
                frame(true, 1, reply),
                [frame(false, 1, b"{\"type\":"), frame(true, 9, b"p"), frame(true, 0, b"\"event\"}")].concat(),
                frame(true, 1, &vec![b'a'; 300]),
                frame(true, 2, &vec![7u8; 70_000]),
                [frame(true, 1, reply), frame(true, 8, &1000u16.to_be_bytes())].concat(),
                text(
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                ),
            ]
        }
        "bidi" => vec![
            text(
                r#"{"type":"event","method":"network.beforeRequestSent","params":{"context":"c1","isBlocked":true,"navigation":"n1","redirectCount":0,"request":{"request":"8-ab","url":"https://chat.example/x","method":"GET","destination":"","initiatorType":"fetch"}}}"#,
            ),
            text(
                r#"{"type":"event","method":"network.beforeRequestSent","params":{"context":"c1","isBlocked":true,"navigation":null,"request":{"request":"9","url":"wss://img.cdn.example/?v=9","destination":"script","initiatorType":"script"}}}"#,
            ),
            text(
                r#"{"type":"success","id":12,"result":{"data":"/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgICAgMCAgIDAwMDBAYEBAQEBAgGBgUGCQgKCgkICQkKDA8MCgsOCwkJDRENDg8QEBEQCgwSExIQEw8QEBD/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AVN//2Q=="}}"#,
            ),
            text(
                r#"{"type":"error","id":3,"error":"no such frame","message":"Browsing context with id c9 not found"}"#,
            ),
            text(
                r#"{"type":"event","method":"browsingContext.load","params":{"context":"c1","navigation":"n1","timestamp":1,"url":"https://chat.example/"}}"#,
            ),
            // A JPEG's start: SOI, APP0, SOF0 (800 by 600).
            [
                &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10][..],
                b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0",
                &[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x02, 0x58, 0x03, 0x20, 0x03, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1],
            ]
            .concat(),
        ],
        "browser_command" => vec![
            text("browser open discord"),
            text("browser mouse down 120 48 right"),
            text("browser scroll -40 2000"),
            text("browser size 400 700"),
            text("browser key ctrl-v"),
            text("browser text -- héllo 👩\u{200d}💻"),
            text("browser frames on"),
        ],
        "bedrock_store" => bastion_mc::bedrock::store::fuzz_seeds(),
        "raknet" => {
            use bastion_mc::egress::raknet::{Offline, PROTOCOL};
            let server: std::net::SocketAddr = "127.64.0.5:19132".parse().unwrap();
            let v6: std::net::SocketAddr = "[2001:db8::7]:19133".parse().unwrap();
            vec![
                Offline::UnconnectedPing { open_connections: false, time: 1, client_guid: 2 }.encode(),
                Offline::UnconnectedPing { open_connections: true, time: 1, client_guid: 2 }.encode(),
                Offline::UnconnectedPong { time: 1, server_guid: 2, data: b"MCPE;motd;800;1.26.62;0;10".to_vec() }
                    .encode(),
                Offline::OpenConnectionRequest1 { protocol: PROTOCOL, mtu: 576 }.encode(),
                Offline::OpenConnectionReply1 { server_guid: 9, cookie: Some(5), mtu: 1400 }.encode(),
                Offline::OpenConnectionRequest2 { cookie: Some(5), server, mtu: 1400, client_guid: 1 }.encode(),
                Offline::OpenConnectionRequest2 { cookie: None, server: v6, mtu: 1400, client_guid: 1 }.encode(),
                Offline::OpenConnectionReply2 { server_guid: 9, client: server, mtu: 1400, security: false }.encode(),
                Offline::IncompatibleProtocol { protocol: 10, server_guid: 9 }.encode(),
                // A script for the Bedrock relay's decisions (four bytes an event).
                vec![
                    1, 0, 0, 0, 5, 0, 0, 0x81, 5, 0, 0, 3, 2, 0, 0, 0, 5, 1, 4, 0x81, 5, 2, 4, 0x81, 3, 0, 10, 0, 0,
                    255, 0, 0, 4, 0, 0, 0,
                ],
            ]
        }
        "bedrock_realms" => vec![
            text(
                r#"{"address":"203.0.113.9:19132","networkProtocol":"DEFAULT","pendingUpdate":false,"sessionRegionData":{"regionName":"EastUS","serviceQuality":1}}"#,
            ),
            text(r#"{"address":"[2001:db8::9]:19133"}"#),
            text(r#"{"address":"1234567890","networkProtocol":"NETHERNET_JSONRPC"}"#),
            text(r#"{"address":"realm.example.org:19132","networkProtocol":"DEFAULT"}"#),
            text("/worlds/7/join"),
            text("/Worlds/123456789/JOIN"),
            text("/api/v2.0/join/world/client"),
        ],
        "bedrock_gatherings" => vec![
            text(
                r#"{"result":{"networkProtocol":"Default","ipV4Address":"198.51.100.20","port":19132,"destinationInfo":{"creatorId":"1","experienceId":"6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44","experienceName":"Live Event","worldName":"Lobby"}}}"#,
            ),
            text(r#"{"result":{"networkProtocol":"NetherNet","ipV4Address":"","port":0}}"#),
            text(
                r#"{"experienceId":"6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44","worldId":"00000000-0000-0000-0000-000000000001"}"#,
            ),
        ],
        other => panic!("no seeds for {other}"),
    }
}

/// A 24×16 test pattern as ffmpeg's JPEG encoder writes it (the shape album art and the
/// recorder's thumbnails have).
const TINY_JPEG: &str = "/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAyAP/bAEMACCgoLygvNzc3Nzc3QTxBQ0NDQUFBQUNDQ0hISFVVVUhISENDSEhQUFVVXF9cV1dVV19fZGRkeHhzc4yMkaysz//EAHMAAQADAQAAAAAAAAAAAAAAAAUDAQQHAQEBAAAAAAAAAAAAAAAAAAAEBhAAAgAFAQYHAQAAAAAAAAAAAREDAgAhMRIi0UFCgVFxkaFTQ1IUExEAAgIBAgcBAQAAAAAAAAAAAQIRIQADMUGRwbFh4RITUf/AABEIABAAGAMBIgACEQADEQD/2gAMAwEAAhEDEQA/AOXbZmIl8eFuppePNGimUTXQJ5QFa7wqkk+QZJ0ld1mmZiP56FfOh3Wpp911ob67NqK0bEx4+hZmq5ZQslkTtPfbAhFiw+KaOJS60foi/b0G6qmsIYGyQSUeVlh0q4nuw/Mbqb+jG+p95JaqKzkkSauAeA/uf//Z";

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
    let tested = [
        "persist",
        "sni",
        "dns",
        "broker",
        "xauthority",
        "wayland",
        "profile",
        "zipdir",
        "udp",
        "companion",
        "phone",
        "registry",
        "jar",
        "realms",
        "sync",
        "dbus",
        "spotify",
        "keeper",
        "picture",
        "clips",
        "x11",
        "websocket",
        "bidi",
        "browser_command",
        "bedrock_store",
        "raknet",
        "bedrock_realms",
        "bedrock_gatherings",
    ];
    let mut names: Vec<&str> = fuzzing::ALL.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    let mut expected = tested.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected, "a harness was added without a robustness test (or the other way round)");
}

#[test]
fn bedrock_android_manifests_version_manifests_and_elf_headers() {
    run("bedrock_store");
}

#[test]
fn realms_table_cookie_and_join_answer() {
    run("realms");
}

#[test]
fn raknet_offline_messages_and_bedrock_relay_decisions() {
    run("raknet");
}

#[test]
fn bedrock_realms_join_answers() {
    run("bedrock_realms");
}

#[test]
fn bedrock_gatherings_join_answers_and_requests() {
    run("bedrock_gatherings");
}

/// The Bedrock starting points are what their parsers accept (a mutation of nothing tests
/// nothing).
#[test]
fn the_bedrock_starting_points_are_accepted() {
    use bastion_mc::egress::{raknet, services, udp};
    let raknet_seeds = seeds("raknet");
    let (script, messages) = raknet_seeds.split_last().unwrap();
    for message in messages {
        assert!(raknet::parse(message).is_ok(), "{}", hex(message));
    }
    let (relayed, refused) = udp::fuzz_bedrock_table(script);
    assert!(relayed > 0 && refused > 0, "the script relays and refuses: {relayed}/{refused}");
    let realms = seeds("bedrock_realms");
    assert!(services::parse_realms_join(&realms[0]).is_ok());
    assert!(services::parse_realms_join(&realms[1]).is_ok());
    assert!(matches!(services::parse_realms_join(&realms[2]), Err(services::JoinError::NetherNet(_))));
    assert_eq!(services::parse_realms_join(&realms[3]), Err(services::JoinError::HostName));
    let gatherings = seeds("bedrock_gatherings");
    assert!(services::parse_gatherings_join(&gatherings[0]).is_ok());
    assert!(matches!(services::parse_gatherings_join(&gatherings[1]), Err(services::JoinError::NetherNet(_))));
    assert!(services::experience_key(&gatherings[2]).is_ok());
}

#[test]
fn settings_sync_merge() {
    run("sync");
}

/// The Realms and sync starting points are what their parsers accept.
#[test]
fn the_realms_and_sync_starting_points_are_accepted() {
    use bastion_mc::realms::table;
    let realms = seeds("realms");
    let profile = "069a79f444e94726a5befca90e38aaf5";
    for cookie in &realms[..2] {
        assert!(table::rebuild_cookie(cookie, "bastion-placeholder-fuzz", b"T.O.K", profile).is_ok());
    }
    assert!(table::parse_join_reply(&realms[2]).is_ok());
    let sync = seeds("sync");
    let at = sync[0].iter().position(|b| *b == 0).unwrap();
    let copy = bastion_mc::sync::store::Store::parse(&sync[0][at + 1..]);
    assert_eq!((copy.len(), copy.dropped), (7, 0));
    let merged = bastion_mc::sync::merge(&copy, Some(bastion_mc::sync::parse_options(&sync[0][..at])), None).unwrap();
    assert!(merged.taken.len() >= 3, "{:?}", merged.taken);
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

#[test]
fn datagram_relay_decisions() {
    run("udp");
}

#[test]
fn companion_channel_reader() {
    run("companion");
}

#[test]
fn phone_frames_and_dispatch() {
    run("phone");
}

#[test]
fn mod_registry_answers() {
    run("registry");
}

#[test]
fn browser_websocket_frames() {
    run("websocket");
}

#[test]
fn browser_bidi_messages_and_pictures() {
    run("bidi");
}

#[test]
fn browser_commands_from_the_phone() {
    run("browser_command");
}

/// The starting points of the browser's harnesses are what their parsers accept.
#[test]
fn the_browser_starting_points_are_accepted() {
    use bastion_mc::browser::{bidi, ws};
    let frames = seeds("websocket");
    assert_eq!(ws::fuzz(&frames[0]), 1);
    assert_eq!(ws::fuzz(&frames[1]), 2, "a ping between the fragments, then the message");
    assert_eq!(ws::fuzz(&frames[4]), 2, "a message, then the close");
    let messages = seeds("bidi");
    let Some(bidi::Incoming::Event { params, .. }) = bidi::parse_incoming(std::str::from_utf8(&messages[0]).unwrap())
    else {
        panic!("an event")
    };
    let paused = bidi::paused(&params).expect("a paused request");
    assert!(paused.navigation && paused.api_call);
    assert_eq!(bidi::jpeg_size(&messages[5]), Some((800, 600)));
    for line in seeds("browser_command") {
        let command = bastion_mc::phone::Command::parse(&line).expect("the grammar");
        let sites = ["discord".to_string()];
        bastion_mc::browser::app::BrowserCommand::parse(&command, &sites).expect("the browser's words");
    }
}

/// The starting points of the registry harness are answers its parsers accept in full.
#[test]
fn the_registry_starting_points_are_accepted() {
    use bastion_mc::profile::modrinth;
    let inputs = seeds("registry");
    let version = modrinth::parse_version(&inputs[0]).expect("a build");
    let jar = version.primary_jar().expect("its file passes every check");
    assert_eq!((jar.filename.as_str(), jar.size), ("ViaFabric-0.4.22+188-26.x.jar", 5_916_758));
    assert_eq!(version.dependencies.len(), 2);
    assert_eq!(modrinth::parse_project(&inputs[1]).expect("a project").license, "GPL-3.0-only");
    assert_eq!(modrinth::parse_versions(&inputs[2]).expect("a list of builds").len(), 2);
    let lookup = modrinth::parse_hash_lookup(&inputs[3], std::slice::from_ref(&jar.sha512)).expect("a hash lookup");
    assert!(lookup[&jar.sha512].file_with_sha512(&jar.sha512).is_some_and(|file| file.is_ok()));
    assert_eq!(modrinth::parse_timestamp(std::str::from_utf8(&inputs[4]).unwrap()).unwrap(), 1_792_696_271);
}

#[test]
fn mod_jars_and_class_files() {
    run("jar");
}

#[test]
fn dbus_messages() {
    run("dbus");
}

#[test]
fn spotify_web_api_replies() {
    run("spotify");
}

#[test]
fn token_keeper_answers() {
    run("keeper");
}

#[test]
fn pictures_for_the_phone() {
    run("picture");
}

#[test]
fn clip_buffer_and_index() {
    run("clips");
}

#[test]
fn x11_replies_to_the_recorder() {
    run("x11");
}

/// The media harnesses' starting points are inputs their parsers accept in full.
#[test]
fn the_media_starting_points_are_accepted() {
    use bastion_mc::{
        clips::{buffer, library, recorder::Status, x11},
        spotify::{dbus::Message, picture, web},
    };
    for seed in seeds("dbus") {
        Message::parse(&seed).expect("a D-Bus message");
    }
    let replies = seeds("spotify");
    assert_eq!(web::parse_state(200, &replies[0]).unwrap().unwrap().title, "Harder");
    assert_eq!(web::parse_devices(&replies[1]).unwrap().len(), 1);
    assert_eq!(web::parse_search(&replies[2]).unwrap().len(), 2);
    assert!(web::parse_token_reply(200, &replies[3]).is_ok());
    assert_eq!(web::parse_error(403, &replies[4]), web::WebError::Premium);
    assert!(matches!(web::parse_token_reply(400, &replies[5]), Err(web::WebError::SignedOut(_))));
    let picture = picture::jpeg_to_png(&seeds("picture")[0], 320, 320).expect("a JPEG");
    assert_eq!((picture.width, picture.height), (24, 16));
    let clips = seeds("clips");
    assert_eq!(buffer::parse_list(&clips[0]).unwrap().len(), 3);
    buffer::Joiner::default().feed(&mut clips[1].clone()).expect("transport-stream packets");
    assert_eq!(library::parse_index(&clips[2]).unwrap().len(), 1);
    assert_eq!(clips[3].split(|b| *b == b'\n').filter(|l| Status::parse(l).is_some()).count(), 4);
    let x = seeds("x11");
    assert_eq!(x11::parse_query_tree(&x[0]).unwrap().len(), 2);
    assert_eq!(x11::parse_property(&x[1]).unwrap().unwrap().1, b"bastion");
    assert_eq!(x11::parse_client_ids(&x[2]).unwrap(), Some(4242));
    assert_eq!(x11::first_cookie(&x[3]), Some(vec![9; 16]));
}

/// The starting points of the jar harness are a jar the capability scan reads in full.
#[test]
fn the_jar_starting_points_are_scanned_in_full() {
    use bastion_mc::profile::capabilities;
    for (n, bytes) in seeds("jar")[..2].iter().enumerate() {
        let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("robustness-seed-{n}.jar"));
        std::fs::write(&path, bytes).unwrap();
        let report = capabilities::scan_jar(&path).expect("a jar");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(report.mod_id.as_deref(), Some("example-mod"));
        assert_eq!((report.classes, report.nested.len()), (2, 1), "{report:?}");
        assert_eq!(report.natives, ["META-INF/jars/helper-1.2.jar!/natives/libhelper.so"]);
        // Both class files are the same class, so each capability is counted once.
        for capability in ["network", "runs programs", "raw memory (named in a string)"] {
            assert_eq!(report.capabilities.get(capability).map(Vec::as_slice), Some(&["mod.Main".to_string()][..]));
        }
        assert!(report.unreadable.is_empty(), "{:?}", report.unreadable);
    }
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
    // The relay scripts: datagrams relayed and refused by each, so that both outcomes and
    // every ending are among the starting points.
    let outcomes: Vec<(u64, u64)> = seeds("udp").iter().map(|s| bastion_mc::egress::udp::fuzz_table(s)).collect();
    assert_eq!(outcomes, [(3, 0), (6, 4), (3, 6), (4, 1)]);
    use bastion_mc::companion::channel::{self, Message};
    for seed in &seeds("companion")[..3] {
        for line in seed.split(|b| *b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l)).filter(|l| !l.is_empty()) {
            assert!(channel::parse_line(line).is_some(), "{:?}", String::from_utf8_lossy(line));
        }
    }
    assert!(matches!(
        channel::parse_line(b"hello 1 0.1.0 zoom,hud,fullbright"),
        Some(Message::Hello { protocol: 1, ref features, .. }) if features.len() == 3
    ));
}
