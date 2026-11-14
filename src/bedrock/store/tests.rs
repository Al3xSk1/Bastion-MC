//! Tests of the version store, on version directories written here (there is no real game
//! download on the machine these were written on): a compiled `AndroidManifest.xml` built by
//! `axml::synthetic`, ELF headers for the libraries, and an asset tree.

use std::{fs, os::unix::fs::PermissionsExt};

use super::*;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("bastion-bedrock-store-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn paths_in(root: &Path) -> Paths {
    let paths = Paths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        state: root.join("state"),
        runtime: None,
    };
    paths.ensure().unwrap();
    paths
}

/// Removes a test's directory, store and all (the store's directories are read-only).
fn cleanup(root: &Path) {
    crate::tree::remove(root, None).unwrap();
}

fn library(extra: usize) -> Vec<u8> {
    let mut bytes = elf::synthetic(3, elf::EM_X86_64);
    bytes.extend((0..extra).map(|i| (i % 251) as u8));
    bytes
}

/// A version directory as mcpelauncher-ui extracts one (version 1.26.62.1, x86_64).
fn fake_version(dir: &Path) {
    for sub in
        ["lib/x86_64", "assets/resource_packs/vanilla", "assets/fonts", "assets/sounds and music", "assets/empty"]
    {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join(ANDROID_MANIFEST),
        axml::synthetic(manifest::PACKAGE, 982606201, "1.26.62.1", None, false, false),
    )
    .unwrap();
    fs::write(dir.join("lib/x86_64/libminecraftpe.so"), library(40_000)).unwrap();
    fs::write(dir.join("lib/x86_64/libc++_shared.so"), library(500)).unwrap();
    fs::write(dir.join("assets/resource_packs/vanilla/manifest.json"), br#"{"format_version":2}"#).unwrap();
    fs::write(dir.join("assets/fonts/Mojangles.ttf"), vec![7u8; 3000]).unwrap();
    fs::write(dir.join("assets/sounds and music/menu 1.ogg"), b"OggS").unwrap();
    fs::write(dir.join("assets/icon.png"), b"\x89PNG").unwrap();
}

fn import_quiet(paths: &Paths, source: &Path, id: Option<&str>) -> Result<Imported> {
    import(paths, source, id, &Reporter::capture())
}

fn policy_error(result: Result<Imported>) -> String {
    match result {
        Err(Error::Policy(message)) => message,
        Err(other) => panic!("expected a policy refusal, got {other}"),
        Ok(imported) => panic!("expected a refusal, imported {:?}", imported.summary),
    }
}

#[test]
fn an_imported_version_is_copied_hashed_read_only_and_opens_verified() {
    let root = scratch("import");
    let paths = paths_in(&root);
    let source = root.join("ui/versions/1.26.62.1");
    fake_version(&source);

    let imported = import_quiet(&paths, &source, None).unwrap();
    let summary = &imported.summary;
    assert_eq!(summary.id, "1.26.62.1", "the version name is the default id");
    assert_eq!((summary.version_name.as_str(), summary.version_code), ("1.26.62.1", 982606201));
    assert_eq!(summary.files, 7);
    assert!(imported.notes.is_empty(), "{:?}", imported.notes);

    let version = versions_dir(&paths).join("1.26.62.1");
    let game = version.join(GAME_DIR);
    assert_eq!(fs::read(game.join("lib/x86_64/libminecraftpe.so")).unwrap(), library(40_000));
    assert_eq!(fs::read(game.join("assets/sounds and music/menu 1.ogg")).unwrap(), b"OggS");
    assert!(game.join("assets/empty").is_dir(), "empty directories are kept");
    // Read-only, all of it, and the manifest beside the game files, not among them.
    for (path, mode) in [
        (version.clone(), 0o555),
        (game.clone(), 0o555),
        (game.join("assets/fonts"), 0o555),
        (game.join("assets/fonts/Mojangles.ttf"), 0o444),
        (version.join(MANIFEST_FILE), 0o444),
    ] {
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, mode, "{}", path.display());
    }
    let text = fs::read_to_string(version.join(MANIFEST_FILE)).unwrap();
    let expected_digest = util::hex_encode(&util::hash_bytes(util::HashAlg::Sha256, &library(500)));
    assert!(
        text.contains(&format!("f {expected_digest} {} lib/x86_64/libc++_shared.so\n", library(500).len())),
        "{text}"
    );
    assert!(text.ends_with("\nend\n"));

    // Opening verifies and hands out the game directory itself.
    let opened = open(&paths, "1.26.62.1").unwrap();
    assert_eq!(opened.path, game);
    let by_fd = fstat(&opened.dir).unwrap();
    let by_path = fs::metadata(&game).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!((by_fd.st_dev, by_fd.st_ino), (by_path.dev(), by_path.ino()));
    drop(opened);
    assert_eq!(verify(&paths, "1.26.62.1").unwrap(), imported.summary);
    // The source is untouched.
    assert_eq!(fs::read(source.join("assets/icon.png")).unwrap(), b"\x89PNG");
    cleanup(&root);
}

#[test]
fn only_the_manifest_the_x86_64_libraries_and_the_assets_are_copied() {
    let root = scratch("subset");
    let paths = paths_in(&root);
    let source = root.join("v");
    fake_version(&source);
    fs::create_dir_all(source.join("lib/arm64-v8a")).unwrap();
    fs::write(source.join("lib/arm64-v8a/libminecraftpe.so"), elf::synthetic(3, 183)).unwrap();
    fs::create_dir_all(source.join("lib/riscv")).unwrap();
    fs::write(source.join("base.apk"), b"PK\x03\x04").unwrap();
    fs::write(source.join("versions.ini"), b"[x]").unwrap();
    // Not copied, so not looked into: what these are does not matter.
    std::os::unix::fs::symlink("/etc/passwd", source.join("stray-link")).unwrap();

    let imported = import_quiet(&paths, &source, Some("subset")).unwrap();
    let notes = imported.notes.join("\n");
    for expected in [
        "not copied: lib/arm64-v8a (libraries for another machine",
        "not copied: lib/riscv (not an ABI",
        "not copied: base.apk (APK files are never imported",
        "not copied: versions.ini",
        "not copied: stray-link",
    ] {
        assert!(notes.contains(expected), "{expected:?} in {notes}");
    }
    let game = versions_dir(&paths).join("subset").join(GAME_DIR);
    let mut top: Vec<String> =
        fs::read_dir(&game).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    top.sort();
    assert_eq!(top, ["AndroidManifest.xml", "assets", "lib"]);
    let libs: Vec<String> =
        fs::read_dir(game.join("lib")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(libs, ["x86_64"]);
    cleanup(&root);
}

type Plant = Box<dyn Fn(&Path)>;

#[test]
fn links_hard_links_fifos_and_sockets_in_the_copied_part_are_refused_by_name() {
    let root = scratch("special");
    let paths = paths_in(&root);
    fs::write(root.join("private-key"), b"SECRET").unwrap();

    let cases: Vec<(&str, Plant, &str)> = vec![
        (
            "symlink",
            Box::new(|v: &Path| std::os::unix::fs::symlink("/etc/passwd", v.join("assets/passwd")).unwrap()),
            "assets/passwd is a symbolic link",
        ),
        (
            "dirlink",
            Box::new(|v: &Path| std::os::unix::fs::symlink("/etc", v.join("assets/fonts/etc")).unwrap()),
            "assets/fonts/etc is a symbolic link",
        ),
        (
            "hardlink",
            Box::new(|v: &Path| fs::hard_link(v.join("../private-key"), v.join("assets/key.txt")).unwrap()),
            "assets/key.txt has 2 hard links",
        ),
        (
            "fifo",
            Box::new(|v: &Path| nix::unistd::mkfifo(&v.join("assets/pipe"), Mode::S_IRWXU).unwrap()),
            "assets/pipe is a FIFO",
        ),
        (
            "socket",
            Box::new(|v: &Path| {
                std::os::unix::net::UnixListener::bind(v.join("assets/sock")).unwrap();
            }),
            "assets/sock is a socket",
        ),
        (
            "liblink",
            Box::new(|v: &Path| {
                fs::rename(v.join("lib/x86_64/libc++_shared.so"), v.join("libc.so")).unwrap();
                std::os::unix::fs::symlink(v.join("libc.so"), v.join("lib/x86_64/libc++_shared.so")).unwrap();
            }),
            "lib/x86_64/libc++_shared.so is a symbolic link",
        ),
        (
            "assets-link",
            Box::new(|v: &Path| {
                fs::rename(v.join("assets"), v.join("real-assets")).unwrap();
                std::os::unix::fs::symlink("real-assets", v.join("assets")).unwrap();
            }),
            "assets is a symbolic link",
        ),
        (
            "manifest-link",
            Box::new(|v: &Path| {
                fs::rename(v.join(ANDROID_MANIFEST), v.join("m.xml")).unwrap();
                std::os::unix::fs::symlink("m.xml", v.join(ANDROID_MANIFEST)).unwrap();
            }),
            "AndroidManifest.xml is a symbolic link",
        ),
    ];
    for (tag, plant, expected) in cases {
        let source = root.join(tag);
        fake_version(&source);
        plant(&source);
        let message = policy_error(import_quiet(&paths, &source, Some(tag)));
        assert!(message.contains(expected), "{tag}: {message}");
        assert!(!versions_dir(&paths).join(tag).exists(), "{tag}: nothing is imported");
    }
    // Refused while looking, before the store was touched: nothing of the secret reached it,
    // and there is no staging directory to leave behind.
    assert!(!versions_dir(&paths).exists());
    assert_eq!(what_it_is(SFlag::S_IFCHR), "a device");
    assert_eq!(what_it_is(SFlag::S_IFBLK), "a device");
    cleanup(&root);
}

#[test]
fn a_version_without_x86_64_libraries_is_refused_with_the_abis_it_has() {
    let root = scratch("abi");
    let paths = paths_in(&root);
    let source = root.join("arm");
    fake_version(&source);
    fs::rename(source.join("lib/x86_64"), source.join("lib/arm64-v8a")).unwrap();
    fs::create_dir_all(source.join("lib/armeabi-v7a")).unwrap();
    let message = policy_error(import_quiet(&paths, &source, None));
    assert!(message.contains("no x86_64 libraries (it has lib/arm64-v8a, lib/armeabi-v7a)"), "{message}");
    assert!(message.contains("only x86_64 builds"), "{message}");
    // x86_64 there, but without the game library.
    let source = root.join("nolib");
    fake_version(&source);
    fs::remove_file(source.join("lib/x86_64/libminecraftpe.so")).unwrap();
    assert!(policy_error(import_quiet(&paths, &source, None)).contains("has no libminecraftpe.so"));
    cleanup(&root);
}

#[test]
fn a_library_for_another_machine_or_with_a_foreign_name_is_refused() {
    let root = scratch("libs");
    let paths = paths_in(&root);
    let cases: Vec<(&str, &str, Vec<u8>, &str)> = vec![
        ("arm", "libminecraftpe.so", elf::synthetic(3, 183), "a library for ARM64, not x86-64"),
        ("x86", "libfmod.so", elf::synthetic(3, 3), "a library for x86 (32-bit)"),
        ("exec", "libfmod.so", elf::synthetic(2, elf::EM_X86_64), "not a shared library"),
        ("text", "libfmod.so", b"#!/bin/sh\necho hi\n".to_vec(), "a script, not a program"),
        ("name", "gdbserver", library(10), "may hold only libraries named lib*.so"),
        ("wrap", "wrap.sh", b"#!/bin/sh\n".to_vec(), "may hold only libraries named lib*.so"),
    ];
    for (tag, name, bytes, expected) in cases {
        let source = root.join(tag);
        fake_version(&source);
        fs::write(source.join("lib/x86_64").join(name), bytes).unwrap();
        let message = policy_error(import_quiet(&paths, &source, Some(tag)));
        assert!(message.contains(expected), "{tag}: {message}");
    }
    // A directory inside lib/x86_64.
    let source = root.join("subdir");
    fake_version(&source);
    fs::create_dir_all(source.join("lib/x86_64/libsub.so")).unwrap();
    let message = policy_error(import_quiet(&paths, &source, Some("subdir")));
    assert!(message.contains("lib/x86_64/libsub.so is a directory"), "{message}");
    cleanup(&root);
}

#[test]
fn an_apk_a_file_or_a_link_is_never_imported() {
    let root = scratch("apk");
    let paths = paths_in(&root);
    let apk = root.join("minecraft.apk");
    fs::write(&apk, b"PK\x03\x04 not unpacked").unwrap();
    let message = policy_error(import_quiet(&paths, &apk, None));
    assert!(message.contains("is not a directory") && message.contains("never an APK"), "{message}");
    let source = root.join("v");
    fake_version(&source);
    std::os::unix::fs::symlink(&source, root.join("latest")).unwrap();
    assert!(policy_error(import_quiet(&paths, &root.join("latest"), None)).contains("is a symbolic link"));
    assert!(matches!(import_quiet(&paths, &root.join("absent"), None), Err(Error::Usage(_))));
    // mcpelauncher-ui's versions directory instead of a version in it.
    let versions = root.join("versions");
    fake_version(&versions.join("1.26.62.1"));
    let message = policy_error(import_quiet(&paths, &versions, None));
    assert!(
        message.contains("has no AndroidManifest.xml")
            && message.contains("import one of the versions in it: 1.26.62.1"),
        "{message}"
    );
    cleanup(&root);
}

#[test]
fn another_package_or_a_splits_manifest_is_refused() {
    let root = scratch("package");
    let paths = paths_in(&root);
    let source = root.join("other");
    fake_version(&source);
    fs::write(source.join(ANDROID_MANIFEST), axml::synthetic("com.example.game", 5, "5.0", None, true, false)).unwrap();
    let message = policy_error(import_quiet(&paths, &source, Some("other")));
    assert!(message.contains("com.example.game") && message.contains("Minecraft only"), "{message}");
    let source = root.join("split");
    fake_version(&source);
    fs::write(
        source.join(ANDROID_MANIFEST),
        axml::synthetic(manifest::PACKAGE, 5, "5.0", Some("config.x86_64"), false, false),
    )
    .unwrap();
    assert!(policy_error(import_quiet(&paths, &source, Some("split"))).contains("split APK config.x86_64"));
    let source = root.join("garbage");
    fake_version(&source);
    fs::write(source.join(ANDROID_MANIFEST), b"<manifest package=\"com.mojang.minecraftpe\"/>").unwrap();
    assert!(matches!(import_quiet(&paths, &source, Some("garbage")), Err(Error::Format(_))));
    cleanup(&root);
}

#[test]
fn the_import_stops_at_its_limits_before_copying() {
    let root = scratch("limits");
    let paths = paths_in(&root);
    let source = root.join("v");
    fake_version(&source);
    let roomy = Limits { entries: 1000, bytes: 1 << 30, file_bytes: 1 << 30, depth: 8, libraries: 8 };
    let refused = |limits: Limits| match import_with(&paths, &source, Some("limited"), &Reporter::capture(), &limits) {
        Err(Error::Policy(message)) => message,
        other => panic!("expected a refusal, got {:?}", other.map(|i| i.summary)),
    };
    assert!(refused(Limits { entries: 6, ..roomy }).contains("more than 6 files and directories"));
    assert!(refused(Limits { bytes: 10_000, ..roomy }).contains("larger than 0 MiB"));
    assert!(refused(Limits { depth: 1, ..roomy }).contains("nested more than 1 levels"));
    assert!(refused(Limits { libraries: 1, ..roomy }).contains("more than 1 files"));
    // A sparse file claims its size without occupying it: refused by its size, at once.
    let sparse = File::create(source.join("assets/huge.bin")).unwrap();
    sparse.set_len(3 << 30).unwrap();
    let started = Instant::now();
    let message = refused(Limits { file_bytes: 2 << 30, bytes: 8 << 30, ..roomy });
    assert!(message.contains("assets/huge.bin is 3072 MiB"), "{message}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!versions_dir(&paths).join("limited").exists());
    // Names: a control character is refused.
    fs::remove_file(source.join("assets/huge.bin")).unwrap();
    fs::write(source.join("assets/bell\u{7}.txt"), b"x").unwrap();
    assert!(refused(roomy).contains("a control character"));
    cleanup(&root);
}

#[test]
fn ids_are_validated_and_an_existing_version_is_never_replaced() {
    let root = scratch("ids");
    let paths = paths_in(&root);
    let source = root.join("Some Dir");
    fake_version(&source);
    fs::write(source.join(ANDROID_MANIFEST), axml::synthetic(manifest::PACKAGE, 7, "Beta 7", None, false, false))
        .unwrap();
    // Neither "Beta 7" nor "Some Dir" is an id.
    assert!(matches!(import_quiet(&paths, &source, None), Err(Error::Usage(m)) if m.contains("--id")));
    for bad in ["../up", "UPPER", ".hidden", "a/b", ""] {
        assert!(matches!(import_quiet(&paths, &source, Some(bad)), Err(Error::Usage(_))), "{bad:?}");
    }
    import_quiet(&paths, &source, Some("beta-7")).unwrap();
    let before = fs::read(versions_dir(&paths).join("beta-7").join(MANIFEST_FILE)).unwrap();
    let message = policy_error(import_quiet(&paths, &source, Some("beta-7")));
    assert!(message.contains("already imported"), "{message}");
    assert_eq!(fs::read(versions_dir(&paths).join("beta-7").join(MANIFEST_FILE)).unwrap(), before);
    cleanup(&root);
}

/// Imports the fake version as `id` and makes the store's copy writable again (as anything
/// running as the user could).
fn imported_writable(paths: &Paths, root: &Path, id: &str) -> PathBuf {
    let source = root.join(format!("src-{id}"));
    fake_version(&source);
    import_quiet(paths, &source, Some(id)).unwrap();
    let game = versions_dir(paths).join(id).join(GAME_DIR);
    for entry in [game.clone(), game.join("assets"), game.join("assets/fonts"), game.join("lib/x86_64")] {
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o755)).unwrap();
    }
    game
}

fn integrity(result: Result<OpenedVersion>) -> String {
    match result {
        Err(Error::Integrity(message)) => message,
        Err(other) => panic!("expected an integrity failure, got {other}"),
        Ok(_) => panic!("a tampered version opened"),
    }
}

#[test]
fn open_finds_a_changed_added_removed_or_replaced_file() {
    let root = scratch("tamper");
    let paths = paths_in(&root);

    let game = imported_writable(&paths, &root, "changed");
    let font = game.join("assets/fonts/Mojangles.ttf");
    fs::set_permissions(&font, fs::Permissions::from_mode(0o644)).unwrap();
    let mut bytes = fs::read(&font).unwrap();
    bytes[100] ^= 1;
    fs::write(&font, &bytes).unwrap();
    assert!(integrity(open(&paths, "changed")).contains("assets/fonts/Mojangles.ttf does not have the SHA-256"));

    let game = imported_writable(&paths, &root, "grown");
    let lib = game.join("lib/x86_64/libc++_shared.so");
    fs::set_permissions(&lib, fs::Permissions::from_mode(0o644)).unwrap();
    fs::OpenOptions::new().append(true).open(&lib).unwrap().write_all(b"more").unwrap();
    assert!(integrity(open(&paths, "grown")).contains("its manifest says"));

    let game = imported_writable(&paths, &root, "added");
    fs::write(game.join("lib/x86_64/libevil.so"), library(10)).unwrap();
    assert!(integrity(open(&paths, "added")).contains("lib/x86_64/libevil.so is in the store but not in its manifest"));

    let game = imported_writable(&paths, &root, "removed");
    fs::remove_file(game.join("assets/icon.png")).unwrap();
    assert!(integrity(open(&paths, "removed")).contains("assets/icon.png is missing"));

    let game = imported_writable(&paths, &root, "linked");
    fs::remove_file(game.join("assets/icon.png")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", game.join("assets/icon.png")).unwrap();
    assert!(integrity(open(&paths, "linked")).contains("assets/icon.png is a symbolic link"));

    let game = imported_writable(&paths, &root, "hardlinked");
    fs::hard_link(game.join("assets/icon.png"), root.join("outside-name")).unwrap();
    assert!(integrity(open(&paths, "hardlinked")).contains("2 hard links"));

    let game = imported_writable(&paths, &root, "dir-for-file");
    fs::remove_file(game.join("assets/icon.png")).unwrap();
    fs::create_dir(game.join("assets/icon.png")).unwrap();
    assert!(
        integrity(open(&paths, "dir-for-file")).contains("assets/icon.png is a directory, its manifest lists a file")
    );
    cleanup(&root);
}

#[test]
fn open_refuses_a_damaged_or_missing_manifest_or_game_directory() {
    let root = scratch("manifest");
    let paths = paths_in(&root);
    imported_writable(&paths, &root, "edited");
    let version = versions_dir(&paths).join("edited");
    fs::set_permissions(&version, fs::Permissions::from_mode(0o755)).unwrap();
    let manifest = version.join(MANIFEST_FILE);
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644)).unwrap();
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, text.replace("abi x86_64", "abi arm64-v8a")).unwrap();
    assert!(integrity(open(&paths, "edited")).contains("manifest is damaged"));
    fs::remove_file(&manifest).unwrap();
    assert!(integrity(open(&paths, "edited")).contains("has no manifest"));
    std::os::unix::fs::symlink("/etc/passwd", &manifest).unwrap();
    assert!(integrity(open(&paths, "edited")).contains("is a symbolic link"));

    imported_writable(&paths, &root, "nogame");
    let version = versions_dir(&paths).join("nogame");
    fs::set_permissions(&version, fs::Permissions::from_mode(0o755)).unwrap();
    crate::tree::remove(&version.join(GAME_DIR), None).unwrap();
    assert!(integrity(open(&paths, "nogame")).contains("no game directory"));

    assert!(matches!(open(&paths, "never-imported"), Err(Error::Usage(_))));
    assert!(matches!(open(&paths, "../escape"), Err(Error::Usage(_))));
    cleanup(&root);
}

#[test]
fn remove_refuses_while_a_launch_holds_the_version_and_removes_it_after() {
    let root = scratch("remove");
    let paths = paths_in(&root);
    let source = root.join("v");
    fake_version(&source);
    import_quiet(&paths, &source, Some("held")).unwrap();

    let opened = open(&paths, "held").unwrap();
    // A duplicate shares the lock (what the launch hands to bubblewrap).
    let duplicate = opened.dir.try_clone().unwrap();
    let message = match remove(&paths, "held") {
        Err(Error::Policy(message)) => message,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert!(message.contains("in use by a running launch"), "{message}");
    // Opening it again meanwhile is fine: the lock is shared.
    drop(open(&paths, "held").unwrap());
    drop(opened);
    assert!(remove(&paths, "held").is_err(), "the duplicate still holds it");
    drop(duplicate);

    // An instance that names the version is reported, not consulted.
    let instance = paths.instances().join("bedrock-main");
    fs::create_dir_all(&instance).unwrap();
    fs::write(instance.join("instance.toml"), "kind = \"bedrock\"\n[bedrock]\nversion = \"held\"\n").unwrap();
    let notes = remove(&paths, "held").unwrap();
    assert!(!versions_dir(&paths).join("held").exists());
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("instance bedrock-main names Bedrock version held"), "{notes:?}");
    assert!(fs::read_dir(versions_dir(&paths)).unwrap().next().is_none(), "no trash is left");
    assert!(matches!(remove(&paths, "held"), Err(Error::Usage(_))));
    assert!(matches!(open(&paths, "held"), Err(Error::Usage(_))));
    cleanup(&root);
}

/// Start time (field 22 of /proc/<pid>/stat) and command name of a process.
fn identity(pid: u32) -> (u64, String) {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let (open, close) = (stat.find('(').unwrap(), stat.rfind(')').unwrap());
    let start = stat[close + 1..].split_ascii_whitespace().nth(19).unwrap().parse().unwrap();
    (start, stat[open + 1..close].to_string())
}

#[test]
fn remove_refuses_while_a_marked_launch_of_a_bedrock_instance_names_the_version() {
    let root = scratch("marked");
    let paths = paths_in(&root);
    let source = root.join("v");
    fake_version(&source);
    import_quiet(&paths, &source, Some("in-use")).unwrap();
    let instance = paths.instances().join("bedrock-main");
    fs::create_dir_all(&instance).unwrap();
    fs::write(instance.join("instance.toml"), "kind = \"bedrock\"\n[bedrock]\nversion = \"in-use\"\n").unwrap();
    let java = paths.instances().join("vanilla");
    fs::create_dir_all(&java).unwrap();
    fs::write(java.join("instance.toml"), "minecraft = \"1.21.4\"\n").unwrap();

    // A live process with a marker that says it is a launch of the instance (the marker
    // records the process's own start time and name, as a launch's marker does).
    let mut child = std::process::Command::new("/usr/bin/sleep").arg("30").spawn().unwrap();
    let marker_dir = paths.cache.join("running");
    fs::create_dir_all(&marker_dir).unwrap();
    let (start, comm) = identity(child.id());
    let marker = marker_dir.join(child.id().to_string());
    fs::write(&marker, format!("{start}\nlaunch:bedrock-main\n{comm}\n")).unwrap();
    let message = match remove(&paths, "in-use") {
        Err(Error::Policy(message)) => message,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert!(message.contains(&format!("instance bedrock-main (process {})", child.id())), "{message}");

    // A running launch of another instance does not hold the version.
    fs::write(&marker, format!("{start}\nlaunch:vanilla\n{comm}\n")).unwrap();
    assert_eq!(launches_using(&paths, "in-use"), Vec::<String>::new());
    // One whose instance cannot be read might: refused, with the reason.
    fs::write(&marker, format!("{start}\nlaunch:gone\n{comm}\n")).unwrap();
    assert!(launches_using(&paths, "in-use")[0].contains("cannot be read"));

    child.kill().unwrap();
    let _ = child.wait();
    remove(&paths, "in-use").unwrap();
    cleanup(&root);
}

#[test]
fn a_file_that_changes_between_the_scan_and_the_copy_ends_the_import() {
    let root = scratch("race");
    let source = root.join("v");
    fake_version(&source);
    let src = nix::fcntl::open(&source, DIR_FLAGS, Mode::empty()).unwrap();
    let planned = scan(&src, &source, &LIMITS).unwrap();
    fs::write(source.join("assets/icon.png"), b"\x89PNG and more").unwrap();
    let dst_path = root.join("copy");
    fs::create_dir(&dst_path).unwrap();
    let dst = nix::fcntl::open(&dst_path, DIR_FLAGS, Mode::empty()).unwrap();
    let message = match copy_planned(&src, &dst, &planned, &Reporter::capture()) {
        Err(Error::Policy(message)) => message,
        other => panic!("expected a refusal, got {:?}", other.map(|e| e.len())),
    };
    assert!(message.contains("assets/icon.png changed while it was being imported"), "{message}");
    // A directory replaced by another one of the same name.
    let source = root.join("w");
    fake_version(&source);
    let src = nix::fcntl::open(&source, DIR_FLAGS, Mode::empty()).unwrap();
    let planned = scan(&src, &source, &LIMITS).unwrap();
    fs::rename(source.join("assets/fonts"), source.join("assets/old-fonts")).unwrap();
    fs::create_dir(source.join("assets/fonts")).unwrap();
    let dst_path = root.join("copy2");
    fs::create_dir(&dst_path).unwrap();
    let dst = nix::fcntl::open(&dst_path, DIR_FLAGS, Mode::empty()).unwrap();
    assert!(
        matches!(copy_planned(&src, &dst, &planned, &Reporter::capture()), Err(Error::Policy(m)) if m.contains("assets/fonts changed"))
    );
    cleanup(&root);
}

#[test]
fn leftovers_of_an_interrupted_import_are_swept_only_when_nobody_holds_them() {
    let root = scratch("sweep");
    let paths = paths_in(&root);
    let versions = versions_dir(&paths);
    fs::create_dir_all(versions.join(".import-0011223344556677/game/assets")).unwrap();
    fs::create_dir_all(versions.join(".remove-old-aabbccdd/game")).unwrap();
    fs::create_dir_all(versions.join(".import-8899aabbccddeeff")).unwrap();
    // Read-only, as a half-finished copy can be.
    fs::set_permissions(versions.join(".import-0011223344556677/game"), fs::Permissions::from_mode(0o555)).unwrap();
    let held = nix::fcntl::open(&versions.join(".import-8899aabbccddeeff"), DIR_FLAGS, Mode::empty()).unwrap();
    let held = Flock::lock(held, FlockArg::LockExclusiveNonblock).unwrap();
    let listing = list(&paths).unwrap();
    assert_eq!(listing.leftovers.len(), 3);
    let vfd = open_versions_dir(&paths, false).unwrap().unwrap();
    // Too young: left alone.
    sweep(&vfd, &versions, Duration::from_secs(3600));
    assert_eq!(list(&paths).unwrap().leftovers.len(), 3);
    sweep(&vfd, &versions, Duration::ZERO);
    assert_eq!(list(&paths).unwrap().leftovers, [".import-8899aabbccddeeff"], "the held one stays");
    drop(held);
    sweep(&vfd, &versions, Duration::ZERO);
    assert!(list(&paths).unwrap().leftovers.is_empty());
    cleanup(&root);
}

#[test]
fn the_listing_shows_versions_damaged_entries_leftovers_and_what_the_ui_has() {
    let root = scratch("list");
    let paths = paths_in(&root);
    assert!(list(&paths).unwrap().versions.is_empty(), "no store yet is an empty store");
    let source = root.join("v");
    fake_version(&source);
    import_quiet(&paths, &source, None).unwrap();
    let versions = versions_dir(&paths);
    fs::create_dir(versions.join("broken")).unwrap();
    fs::create_dir(versions.join("Not An Id")).unwrap();
    fs::create_dir(versions.join(".import-0000000000000000")).unwrap();
    let listing = list(&paths).unwrap();
    assert_eq!(listing.versions.len(), 1);
    assert_eq!(listing.versions[0].id, "1.26.62.1");
    let damaged: Vec<&str> = listing.damaged.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(damaged, ["Not An Id", "broken"]);
    assert!(listing.damaged[1].1.contains("has no manifest"));
    assert_eq!(listing.leftovers.len(), 1);

    let home = root.join("home");
    let other = home.join(".local/share/mcpelauncher/versions/1.26.70.24");
    fake_version(&other);
    fs::write(
        other.join(ANDROID_MANIFEST),
        axml::synthetic(manifest::PACKAGE, 982607024, "1.26.70.24", None, false, false),
    )
    .unwrap();
    let same = home.join(".local/share/mcpelauncher/versions/1.26.62.1");
    fake_version(&same);
    let ui = ui_version_dirs(&home, None);
    let lines = listing_lines(&listing, &ui).join("\n");
    assert!(lines.contains("1.26.62.1\tMinecraft 1.26.62.1 (version code 982606201)\t7 files"), "{lines}");
    assert!(lines.contains("broken\tNOT USABLE"), "{lines}");
    assert!(lines.contains("1 unfinished import(s)"), "{lines}");
    assert!(lines.contains("1.26.70.24\tMinecraft 1.26.70.24 (version code 982607024)"), "{lines}");
    assert!(!lines.contains(&format!("{}\t", same.display())), "an imported version is not offered again: {lines}");
    cleanup(&root);
}

#[test]
fn mcpelauncher_ui_versions_are_found_in_the_native_and_the_flatpak_places() {
    let root = scratch("ui");
    let home = root.join("home");
    let native = home.join(".local/share/mcpelauncher/versions/1.26.62.1");
    fake_version(&native);
    fs::create_dir_all(home.join(".local/share/mcpelauncher/versions/temp-AbCdEf")).unwrap();
    let flatpak = home.join(".var/app/io.mrarm.mcpelauncher/data/mcpelauncher/versions/1.26.70.24");
    fake_version(&flatpak);
    fs::rename(flatpak.join("lib/x86_64"), flatpak.join("lib/arm64-v8a")).unwrap();
    std::os::unix::fs::symlink(&native, home.join(".local/share/mcpelauncher/versions/link")).unwrap();

    let found = ui_version_dirs(&home, None);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!((found[0].origin, found[0].x86_64, found[0].version_code), (Origin::Native, true, Some(982606201)));
    assert_eq!(found[0].path, native);
    assert_eq!((found[1].origin, found[1].x86_64), (Origin::Flatpak, false));
    // $XDG_DATA_HOME moves the native place (and only that).
    let data_home = root.join("xdg-data");
    let moved = data_home.join("mcpelauncher/versions/1.0.0.1");
    fake_version(&moved);
    let found = ui_version_dirs(&home, Some(&data_home));
    let paths: Vec<&Path> = found.iter().map(|u| u.path.as_path()).collect();
    assert_eq!(paths, [moved.as_path(), flatpak.as_path()]);
    cleanup(&root);
}

#[test]
fn instance_version_reads_only_bedrock_instances() {
    let root = scratch("instance");
    let paths = paths_in(&root);
    for (id, text) in [
        ("bedrock-a", "kind = \"bedrock\"\n[bedrock]\nversion = \"1.26.62.1\"\nservers = []\n"),
        ("vanilla", "minecraft = \"1.21.4\"\n"),
        ("broken", "kind = = \"bedrock\""),
    ] {
        let dir = paths.instances().join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("instance.toml"), text).unwrap();
    }
    assert_eq!(instance_version(&paths, "bedrock-a").unwrap().as_deref(), Some("1.26.62.1"));
    assert_eq!(instance_version(&paths, "vanilla").unwrap(), None);
    assert!(instance_version(&paths, "broken").is_err());
    assert!(instance_version(&paths, "missing").is_err());
    assert!(instance_version(&paths, "../x").is_err());
    cleanup(&root);
}

#[test]
fn the_fuzz_seeds_are_accepted_by_their_parsers() {
    let seeds = fuzz_seeds();
    assert!(manifest::parse(&seeds[0]).is_ok());
    assert_eq!(axml::parse(&seeds[1]).unwrap().version_code, 982606201);
    assert_eq!(axml::parse(&seeds[2]).unwrap().version_name, "1.26.70.24");
    assert_eq!(axml::parse(&seeds[3]).unwrap().split.as_deref(), Some("config.x86_64"));
    assert!(elf::check_x86_64_library(&seeds[4]).is_ok());
    for seed in &seeds {
        fuzz(seed);
    }
}
