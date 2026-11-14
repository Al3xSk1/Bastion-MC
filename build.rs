//! Also compiles `native/noclip.c`, the clipboard cut-off preloaded into the nested
//! compositor (see `src/sandbox/nested.rs`), with the C compiler Rust already links with.
//!
//! Also compiles and packs the companion Fabric mod (`java/companion`) with the same javac;
//! the recipe is `src/companion/jar.rs`, shared with the tests that reproduce it.
//!
//! Compiles `java/BastionBootstrap.java` with the *system* javac and embeds the class file.
//!
//! No precompiled binary blob is checked into the repository: the bootstrap that handles the
//! access token is built from auditable source on every build. `--release 8` keeps the class
//! loadable by every JVM Minecraft has ever targeted; newer JDKs that drop that target fall
//! back to the oldest level they still support (and the launcher then refuses older JVMs).

use std::{env, fs, path::PathBuf, process::Command};

#[path = "src/companion/jar.rs"]
mod companion_jar;

fn main() {
    println!("cargo:rerun-if-changed=java/BastionBootstrap.java");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=JAVAC");
    println!("cargo:rerun-if-changed=native/noclip.c");
    println!("cargo:rerun-if-env-changed=CC");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    build_noclip(&out_dir);
    let javac = env::var_os("JAVAC").unwrap_or_else(|| "javac".into());
    build_companion(&out_dir, &javac);
    let class_file = out_dir.join("bastion/boot/BastionBootstrap.class");

    let mut last_err = String::new();
    let mut built = None;
    for release in ["8", "11", "17", "21"] {
        let _ = fs::remove_file(&class_file);
        let output = Command::new(&javac)
            .args(["--release", release, "-Xlint:-options", "-encoding", "UTF-8", "-g:none", "-d"])
            .arg(&out_dir)
            .arg("java/BastionBootstrap.java")
            .output();
        match output {
            Ok(o) if o.status.success() && class_file.is_file() => {
                built = Some(release);
                break;
            }
            Ok(o) => last_err = String::from_utf8_lossy(&o.stderr).into_owned(),
            Err(e) => {
                panic!(
                    "cannot run javac ({}): {e}. Install a JDK (Arch: `pacman -S jdk-openjdk`) or set JAVAC.",
                    javac.to_string_lossy()
                )
            }
        }
    }
    let release = built.unwrap_or_else(|| panic!("javac failed to compile the bootstrap class:\n{last_err}"));
    if release != "8" {
        println!(
            "cargo:warning=javac no longer supports --release 8; bootstrap targets Java {release}. \
             Instances needing an older JVM will be refused at launch."
        );
    }

    // Class file layout: u4 magic, u2 minor, u2 major.
    let bytes = fs::read(&class_file).expect("read class file");
    assert!(bytes.len() > 8 && bytes[..4] == [0xCA, 0xFE, 0xBA, 0xBE], "javac produced an invalid class file");
    let class_major = u16::from_be_bytes([bytes[6], bytes[7]]);
    // Class major 52 == Java 8; Java N == major N + 44.
    let min_java = class_major.saturating_sub(44);
    fs::write(
        out_dir.join("bootstrap_meta.rs"),
        format!("/// Lowest Java feature release able to load the embedded bootstrap class.\npub const BOOTSTRAP_MIN_JAVA: u32 = {min_java};\n"),
    )
    .expect("write bootstrap_meta.rs");
}

/// The companion mod's jar, embedded by `src/companion/mod.rs`. Built from source on every
/// build, like the bootstrap class: no binary of it is checked in.
fn build_companion(out_dir: &std::path::Path, javac: &std::ffi::OsStr) {
    for input in ["java/companion", "java/stubs", "src/companion/jar.rs", "LICENSE"] {
        println!("cargo:rerun-if-changed={input}");
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let built = companion_jar::build(&root, javac, &version, &out_dir.join("companion"))
        .unwrap_or_else(|why| panic!("building the companion mod failed: {why}"));
    for line in built.warnings.lines().filter(|line| !line.trim().is_empty()) {
        println!("cargo:warning=companion mod (javac): {line}");
    }
    fs::write(out_dir.join("bastion-companion.jar"), built.jar).expect("write bastion-companion.jar");
}

fn build_noclip(out_dir: &std::path::Path) {
    println!("cargo:rerun-if-env-changed=CFLAGS");
    let cc = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let lib = out_dir.join("libbastion_noclip.so");
    // The packager's hardening flags (CFLAGS) first, then what this library needs whatever
    // they are. `-U` before `-D`: a package's own _FORTIFY_SOURCE level wins without a
    // redefinition warning (which -Werror would turn into a failure).
    let cflags = env::var("CFLAGS").unwrap_or_default();
    let fortify = if cflags.contains("_FORTIFY_SOURCE") { vec![] } else { vec!["-D_FORTIFY_SOURCE=2"] };
    let output = Command::new(&cc)
        .args(cflags.split_ascii_whitespace().filter(|flag| *flag != "-fPIE" && *flag != "-pie"))
        .args(["-shared", "-fPIC", "-O2", "-Wall", "-Werror", "-fstack-protector-strong"])
        .args(fortify)
        .args(["-Wl,-z,relro,-z,now,-z,noexecstack", "-Wl,--build-id=none", "-o"])
        .arg(&lib)
        .arg("native/noclip.c")
        .arg("-ldl")
        .output()
        .unwrap_or_else(|e| panic!("cannot run the C compiler ({}): {e}. Set CC.", cc.to_string_lossy()));
    assert!(output.status.success(), "compiling native/noclip.c failed:\n{}", String::from_utf8_lossy(&output.stderr));
}
