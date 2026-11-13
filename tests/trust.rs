//! The generated Java keystore, read back by the JDK's own tool: if `keytool` accepts the
//! integrity value and lists every certificate, the JVM will.

use std::process::Command;

use bastion_mc::{
    java,
    sandbox::trust::{JAVA_STORE_PASSWORD, TrustStore},
};

#[test]
fn keytool_accepts_the_generated_keystore() {
    let Some(jvm) = java::discover().ok().and_then(|all| all.into_iter().next_back()) else {
        eprintln!("skipped: no system JVM");
        return;
    };
    let keytool = jvm.home.join("bin/keytool");
    if !keytool.is_file() {
        eprintln!("skipped: {} has no keytool (a JRE, not a JDK)", jvm.home.display());
        return;
    }
    let store = TrustStore::build(None).unwrap();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("trust-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("cacerts");
    std::fs::write(&file, store.java_keystore()).unwrap();

    let list = |password: &str| {
        Command::new(&keytool)
            .args(["-list", "-storetype", "jks", "-keystore"])
            .arg(&file)
            .args(["-storepass", password])
            .env_clear()
            .output()
            .unwrap()
    };
    let good = list(JAVA_STORE_PASSWORD);
    let text = String::from_utf8_lossy(&good.stdout);
    assert!(good.status.success(), "{text}\n{}", String::from_utf8_lossy(&good.stderr));
    assert!(
        text.contains(&format!("Your keystore contains {} entries", store.anchors)),
        "expected {} entries:\n{}",
        store.anchors,
        text.lines().take(6).collect::<Vec<_>>().join("\n")
    );
    assert!(text.contains("system-0000") && text.contains("trustedCertEntry"));
    // The integrity value is really checked: a wrong password must be refused.
    assert!(!list("not-the-password").status.success());

    // A flipped byte in a certificate is detected too.
    let mut damaged = store.java_keystore().to_vec();
    let middle = damaged.len() / 2;
    damaged[middle] ^= 0x01;
    std::fs::write(&file, &damaged).unwrap();
    assert!(!list(JAVA_STORE_PASSWORD).status.success(), "a damaged store must not verify");
    std::fs::remove_dir_all(&dir).unwrap();
}
