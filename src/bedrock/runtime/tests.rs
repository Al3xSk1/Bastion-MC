//! Tests of client discovery. The ownership rules are exercised on trees built here under a
//! directory of the test's user, with a policy whose trusted owner is that user and whose
//! system root is that directory (only this module can build such a policy); the production
//! policy (root, `/`) is exercised against programs root really owns on this machine. There
//! is no mcpelauncher-client on the machine these tests were written on.

use std::{fs, os::unix::fs::PermissionsExt};

use super::*;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("bastion-bedrock-runtime-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn policy(root: &Path) -> Policy {
    let homes = vec![root.join("home/player")];
    Policy {
        root: root.to_path_buf(),
        owner: nix::unistd::geteuid().as_raw(),
        flatpak_roots: flatpak_roots(root, &homes),
        homes,
    }
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn program() -> Vec<u8> {
    let mut bytes = elf::synthetic(3, elf::EM_X86_64);
    bytes.extend_from_slice(&[0u8; 256]);
    bytes
}

/// What the AUR package installs, and its record in pacman's database.
fn packaged(root: &Path) {
    write_mode(&root.join("usr/bin/mcpelauncher-client"), &program(), 0o755);
    write_mode(&root.join("usr/bin/mcpelauncher-webview"), &program(), 0o755);
    write_mode(&root.join("usr/share/mcpelauncher/lib/x86_64/libsqliteX.so"), &program(), 0o755);
    write_mode(&root.join("usr/share/mcpelauncher/gamecontrollerdb.txt"), b"# mappings\n", 0o644);
    let db = root.join("var/lib/pacman/local/mcpelauncher-linux-1.8.7-1");
    write_mode(&db.join("desc"), b"%NAME%\nmcpelauncher-linux\n\n%VERSION%\n1.8.7-1\n\n%ARCH%\nx86_64\n\n", 0o644);
    write_mode(
        &db.join("files"),
        b"%FILES%\nusr/\nusr/bin/\nusr/bin/mcpelauncher-client\nusr/bin/mcpelauncher-webview\n\
          usr/lib/mcpelauncher/\nusr/lib/mcpelauncher/mcpelauncher-webview\nusr/share/mcpelauncher/\n\n%BACKUP%\n",
        0o644,
    );
    let other = root.join("var/lib/pacman/local/coreutils-9.9-1");
    write_mode(&other.join("desc"), b"%NAME%\ncoreutils\n\n%VERSION%\n9.9-1\n\n", 0o644);
    write_mode(&other.join("files"), b"%FILES%\nusr/\nusr/bin/\nusr/bin/true\n\n", 0o644);
}

fn refused(result: Result<Client>) -> String {
    match result {
        Err(Error::Policy(message)) => message,
        Err(other) => panic!("expected a policy refusal, got {other}"),
        Ok(client) => panic!("expected a refusal, found {client:?}"),
    }
}

fn configured(path: &Path) -> BedrockConfig {
    BedrockConfig { client: Some(path.to_path_buf()), ..BedrockConfig::default() }
}

#[test]
fn a_packaged_client_is_found_with_its_data_directory_version_and_every_webview_place() {
    let root = scratch("packaged");
    packaged(&root);
    let p = policy(&root);
    let client = discover_with(&BedrockConfig::default(), &p).unwrap();
    assert_eq!(client.program, root.join("usr/bin/mcpelauncher-client"));
    assert_eq!(client.lib_dirs, [root.join("usr/share/mcpelauncher")]);
    assert_eq!(client.version.as_deref(), Some("1.8.7-1 (mcpelauncher-linux)"));
    // Beside the client and in the sandbox's PATH (the same file here), and every one the
    // package database lists — whether or not it exists.
    assert_eq!(
        client.webview_paths,
        [root.join("usr/bin/mcpelauncher-webview"), root.join("usr/lib/mcpelauncher/mcpelauncher-webview")]
    );
    assert_eq!(client.installed_webviews(), [root.join("usr/bin/mcpelauncher-webview")]);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_writable_directory_anywhere_on_the_way_is_refused() {
    let root = scratch("writable");
    packaged(&root);
    let p = policy(&root);
    for (path, mode) in [(root.join("usr/bin"), 0o775), (root.join("usr"), 0o757), (root.clone(), 0o775)] {
        let dir = path.display();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let message = refused(discover_with(&BedrockConfig::default(), &p));
        assert!(message.contains("writable by group or others"), "{dir}: {message}");
        assert!(message.contains(&format!("{} is", path.display())), "{dir}: {message}");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    // A data directory is held to the same rule.
    fs::set_permissions(root.join("usr/share/mcpelauncher/lib"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(refused(discover_with(&BedrockConfig::default(), &p)).contains("lib is writable"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_client_writable_by_others_set_user_id_or_not_executable_is_refused() {
    let root = scratch("modes");
    packaged(&root);
    let p = policy(&root);
    let client = root.join("usr/bin/mcpelauncher-client");
    for (mode, expected) in [
        (0o775, "writable by nobody else"),
        (0o757, "writable by nobody else"),
        (0o4755, "set-user-ID"),
        (0o2755, "set-user-ID or set-group-ID"),
        (0o750, "not executable by you"),
    ] {
        fs::set_permissions(&client, fs::Permissions::from_mode(mode)).unwrap();
        let message = refused(discover_with(&BedrockConfig::default(), &p));
        assert!(message.contains(expected), "{mode:o}: {message}");
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn links_are_followed_one_hop_at_a_time_and_every_hop_is_checked() {
    let root = scratch("links");
    packaged(&root);
    let p = policy(&root);
    // usr/bin/mcpelauncher-client -> ../lib/mcpelauncher/client (relative), as packages do.
    let real = root.join("usr/lib/mcpelauncher/client");
    write_mode(&real, &program(), 0o755);
    let link = root.join("usr/bin/mcpelauncher-client");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("../lib/mcpelauncher/client", &link).unwrap();
    let client = discover_with(&BedrockConfig::default(), &p).unwrap();
    assert_eq!(client.program, real, "the real path, every link resolved");
    // An absolute link inside the system works the same.
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(discover_with(&BedrockConfig::default(), &p).unwrap().program, real);
    // A link into a directory others can write: refused at that directory.
    let tmp = root.join("tmp");
    fs::create_dir(&tmp).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o1777)).unwrap();
    write_mode(&tmp.join("mcpelauncher-client"), &program(), 0o755);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("../../tmp/mcpelauncher-client", &link).unwrap();
    let message = refused(discover_with(&BedrockConfig::default(), &p));
    assert!(message.contains(&format!("{} is writable by group or others", tmp.display())), "{message}");
    // A loop.
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("mcpelauncher-client", &link).unwrap();
    assert!(refused(discover_with(&BedrockConfig::default(), &p)).contains("too many symbolic links"));
    // Out of the system (here: out of the test's root).
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("/usr/bin/true", &link).unwrap();
    assert!(refused(discover_with(&BedrockConfig::default(), &p)).contains("leads outside the system"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_client_under_the_home_directory_is_refused_with_the_reason() {
    let root = scratch("home");
    packaged(&root);
    let p = policy(&root);
    let in_home = root.join("home/player/.local/bin/mcpelauncher-client");
    write_mode(&in_home, &program(), 0o755);
    let message = refused(discover_with(&configured(&in_home), &p));
    assert!(
        message.contains("is under your home directory") && message.contains("every program running as you"),
        "{message}"
    );
    // Reached through a link from a trusted place: refused all the same.
    let link = root.join("usr/bin/mcpelauncher-client");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&in_home, &link).unwrap();
    assert!(refused(discover_with(&BedrockConfig::default(), &p)).contains("under your home directory"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn flatpak_installs_are_refused_by_location_and_by_layout_with_the_reason() {
    let root = scratch("flatpak");
    let deployed = "app/io.mrarm.mcpelauncher/x86_64/stable/0123abcd/files/bin/mcpelauncher-client";
    let system = root.join("var/lib/flatpak").join(deployed);
    write_mode(&system, &program(), 0o755);
    let user = root.join("home/player/.local/share/flatpak").join(deployed);
    write_mode(&user, &program(), 0o755);
    let p = policy(&root);
    for path in [&system, &user] {
        let message = refused(discover_with(&configured(path), &p));
        assert!(message.contains("is a Flatpak install") && message.contains("new namespaces"), "{message}");
    }
    // Anywhere else, known by the deployment's layout.
    let elsewhere = root.join("opt/apps/app/io.mrarm.mcpelauncher/x86_64/stable/0123abcd");
    write_mode(&elsewhere.join("metadata"), b"[Application]\nname=io.mrarm.mcpelauncher\n", 0o644);
    write_mode(&elsewhere.join("files/bin/mcpelauncher-client"), &program(), 0o755);
    let message = refused(discover_with(&configured(&elsewhere.join("files/bin/mcpelauncher-client")), &p));
    assert!(message.contains("part of a Flatpak deployment"), "{message}");
    // An installation configured in /etc/flatpak/installations.d is a Flatpak root too.
    let custom = root.join("srv/flatpak");
    write_mode(&custom.join(deployed), &program(), 0o755);
    write_mode(
        &root.join("etc/flatpak/installations.d/extra.conf"),
        format!("[Installation \"extra\"]\nPath={}\n", custom.display()).as_bytes(),
        0o644,
    );
    let p = policy(&root);
    assert!(refused(discover_with(&configured(&custom.join(deployed)), &p)).contains("is a Flatpak install"));
    // Nothing installed by a package, but a Flatpak: the message says why it is not used.
    match discover_with(&BedrockConfig::default(), &p) {
        Err(Error::Config(message)) => {
            assert!(message.contains("mcpelauncher-linux") && message.contains("The Flatpak install at"), "{message}")
        }
        other => panic!("expected a configuration error, got {other:?}"),
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_appimage_a_script_or_a_program_for_another_machine_is_refused() {
    let root = scratch("kinds");
    packaged(&root);
    let p = policy(&root);
    let client = root.join("usr/bin/mcpelauncher-client");
    let mut appimage = program();
    appimage[8..11].copy_from_slice(b"AI\x02");
    for (bytes, expected) in [
        (appimage, "is an AppImage"),
        (b"#!/bin/sh\nexec flatpak run io.mrarm.mcpelauncher \"$@\"\n".to_vec(), "is a script"),
        (elf::synthetic(3, 183), "a program for ARM64"),
        (b"MZ\x90\x00".to_vec(), "not an ELF file"),
    ] {
        write_mode(&client, &bytes, 0o755);
        let message = refused(discover_with(&BedrockConfig::default(), &p));
        assert!(message.contains(expected), "{message}");
    }
    // Inside an AppImage's mount, refused before anything there is opened.
    let mount = root.join("tmp/.mount_McpelaXyZ123/usr/bin/mcpelauncher-client");
    let message = refused(discover_with(&configured(&mount), &p));
    assert!(message.contains("inside an AppImage's mount"), "{message}");
    // And a FUSE file system, whatever its owner says.
    let fuse = refuse_file_system(FUSE_SUPER_MAGIC, Path::new("/tmp/.mount_x")).unwrap_err();
    assert!(fuse.to_string().contains("FUSE file system"), "{fuse}");
    assert!(refuse_file_system(nix::sys::statfs::TMPFS_MAGIC, Path::new("/tmp")).is_ok());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_configured_client_must_be_absolute_and_exist_and_nothing_installed_says_what_to_install() {
    let root = scratch("config");
    let p = policy(&root);
    assert!(
        matches!(discover_with(&configured(Path::new("bin/mcpelauncher-client")), &p), Err(Error::Config(m)) if m.contains("absolute"))
    );
    assert!(
        matches!(discover_with(&configured(&root.join("nowhere/client")), &p), Err(Error::Config(m)) if m.contains("does not exist"))
    );
    match discover_with(&BedrockConfig::default(), &p) {
        Err(Error::Config(message)) => {
            assert!(message.contains("mcpelauncher-client is not installed"), "{message}");
            assert!(
                message.contains("mcpelauncher-linux") && message.contains("never downloads or builds one"),
                "{message}"
            );
            assert!(!message.contains("Flatpak"), "{message}");
        }
        other => panic!("expected a configuration error, got {other:?}"),
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_data_directory_holds_only_plain_files_and_directories() {
    let root = scratch("data");
    packaged(&root);
    let p = policy(&root);
    let data = root.join("usr/share/mcpelauncher");
    std::os::unix::fs::symlink("/etc/passwd", data.join("gamecontrollerdb-link.txt")).unwrap();
    assert!(
        refused(discover_with(&BedrockConfig::default(), &p))
            .contains("is a symbolic link inside the Bedrock client's data directory")
    );
    fs::remove_file(data.join("gamecontrollerdb-link.txt")).unwrap();
    fs::set_permissions(data.join("gamecontrollerdb.txt"), fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        refused(discover_with(&BedrockConfig::default(), &p)).contains("gamecontrollerdb.txt must be owned by root")
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_manual_install_is_searched_for_webviews_in_its_own_tree() {
    let root = scratch("manual");
    let prefix = root.join("opt/mcpe");
    write_mode(&prefix.join("bin/mcpelauncher-client"), &program(), 0o755);
    write_mode(&prefix.join("libexec/mcpelauncher/mcpelauncher-webview"), &program(), 0o755);
    write_mode(&prefix.join("share/mcpelauncher/lib/x86_64/libsqliteX.so"), &program(), 0o755);
    let p = policy(&root);
    let client = discover_with(&configured(&prefix.join("bin/mcpelauncher-client")), &p).unwrap();
    assert_eq!(client.lib_dirs, [prefix.join("share/mcpelauncher")]);
    assert_eq!(client.version, None, "no package owns it");
    assert_eq!(
        client.webview_paths,
        [
            prefix.join("bin/mcpelauncher-webview"),
            root.join("usr/bin/mcpelauncher-webview"),
            prefix.join("libexec/mcpelauncher/mcpelauncher-webview"),
        ]
    );
    assert_eq!(client.installed_webviews(), [prefix.join("libexec/mcpelauncher/mcpelauncher-webview")]);
    // A link among them is listed with what it leads to.
    std::os::unix::fs::symlink("../libexec/mcpelauncher/mcpelauncher-webview", prefix.join("bin/mcpelauncher-webview"))
        .unwrap();
    let client = discover_with(&configured(&prefix.join("bin/mcpelauncher-client")), &p).unwrap();
    assert_eq!(client.installed_webviews(), [prefix.join("libexec/mcpelauncher/mcpelauncher-webview")]);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn pacman_records_are_read_strictly() {
    let text = "%NAME%\nmcpelauncher-linux\n\n%VERSION%\n1.8.7-1\n\n%FILES%\nusr/\nusr/bin/x\n\n%BACKUP%\netc/x\n";
    assert_eq!(files_entries(text).collect::<Vec<_>>(), ["usr/", "usr/bin/x"]);
    assert_eq!(desc_field(text, "%NAME%"), Some("mcpelauncher-linux"));
    assert_eq!(desc_field(text, "%VERSION%"), Some("1.8.7-1"));
    assert_eq!(desc_field("%NAME%\nbad name\n", "%NAME%"), None);
    assert_eq!(desc_field("%NAME%\n\x1b[31mred\n", "%NAME%"), None);
    assert_eq!(desc_field("%NAME%\n", "%NAME%"), None);
    assert_eq!(files_entries("no files section").count(), 0);
    fuzz_pacman(&fuzz_seed_pacman());
}

#[test]
fn the_production_policy_accepts_a_program_root_owns_and_reads_its_package() {
    // /usr/bin/true stands in for the client: a root-owned x86-64 program from a package.
    let client = discover(&configured(Path::new("/usr/bin/true"))).unwrap();
    assert_eq!(client.program, Path::new("/usr/bin/true"));
    assert!(client.version.as_deref().is_some_and(|v| v.ends_with("(coreutils)")), "{:?}", client.version);
    assert!(client.webview_paths.contains(&PathBuf::from("/usr/bin/mcpelauncher-webview")));
    assert!(client.lib_dirs.iter().all(|d| d.starts_with("/usr")), "{:?}", client.lib_dirs);
}

#[test]
fn the_production_policy_refuses_a_program_the_user_owns() {
    // In /tmp: refused at /tmp, which everyone can write.
    let dir = scratch("owned");
    let copy = dir.join("mcpelauncher-client");
    fs::copy("/usr/bin/true", &copy).unwrap();
    let message = refused(discover(&configured(&copy)));
    assert!(message.contains("/tmp is writable by group or others"), "{message}");
    fs::remove_dir_all(&dir).unwrap();
    // Under the home directory: refused for that, before anything is opened — checked only
    // where this checkout lies under a home directory (the two the discovery itself reads:
    // `$HOME` and the account's passwd entry). A checkout elsewhere, such as CI's under `/__w/`,
    // would only show that a path which does not exist is refused as not existing.
    let home_copy = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/bedrock-home-test/mcpelauncher-client");
    let homes: Vec<PathBuf> = [
        std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute()),
        nix::unistd::User::from_uid(nix::unistd::getuid()).ok().flatten().map(|u| u.dir),
    ]
    .into_iter()
    .flatten()
    .filter(|home| home.as_os_str().len() > 1)
    .collect();
    if homes.iter().any(|home| home_copy.starts_with(home)) {
        let message = refused(discover(&configured(&home_copy)));
        assert!(message.contains("under your home directory"), "{message}");
    }
}

#[test]
fn on_this_machine_there_is_no_client_and_describe_says_where_it_looked() {
    if Path::new("/usr/bin/mcpelauncher-client").exists() || Path::new("/usr/local/bin/mcpelauncher-client").exists() {
        return; // A client was installed after these tests were written.
    }
    let description = describe(&BedrockConfig::default());
    assert!(matches!(&description.client, Err(Error::Config(m)) if m.contains("mcpelauncher-linux")));
    assert_eq!(
        description.searched,
        [PathBuf::from("/usr/bin/mcpelauncher-client"), PathBuf::from("/usr/local/bin/mcpelauncher-client")]
    );
}
