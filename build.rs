//! Compiles `java/BastionBootstrap.java` with the *system* javac and embeds the class file.
//!
//! No precompiled binary blob is checked into the repository: the bootstrap that handles the
//! access token is built from auditable source on every build. `--release 8` keeps the class
//! loadable by every JVM Minecraft has ever targeted; newer JDKs that drop that target fall
//! back to the oldest level they still support (and the launcher then refuses older JVMs).

use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=java/BastionBootstrap.java");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=JAVAC");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let javac = env::var_os("JAVAC").unwrap_or_else(|| "javac".into());
    let class_file = out_dir.join("BastionBootstrap.class");

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
