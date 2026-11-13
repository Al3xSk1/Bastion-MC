//! Real installs of legacy game versions from Mojang (network; `BASTION_NET_TESTS=1`).
//! These cover the code paths that cannot be launched on the development machine, which only
//! has a current JDK: classifier natives that the launcher must unpack, and the pre-1.6
//! asset layout.

use std::path::PathBuf;

use bastion_mc::{
    Error,
    mojang::install::{Store, extract_natives},
    net::{http::HttpClient, tls},
    paths::Paths,
    util::Expected,
};

#[tokio::test(flavor = "multi_thread")]
async fn legacy_versions_install_verify_and_unpack() {
    if std::env::var_os("BASTION_NET_TESTS").is_none() {
        eprintln!("skipped: set BASTION_NET_TESTS=1 to download legacy versions from Mojang");
        return;
    }
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("legacy-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = Paths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        state: root.join("state"),
        runtime: None,
    };
    paths.ensure().unwrap();
    let http =
        HttpClient::new(tls::client_config(tls::load_system_roots().unwrap(), tls::PinSet::default()).unwrap(), &[]);
    let reporter = bastion_mc::events::Reporter::human(true);
    let store = Store { paths: &paths, http: &http, parallel: 8, repair: false, reporter: &reporter };

    // --- 1.12.2: LWJGL 2, natives shipped inside classifier jars -------------------------
    let v112 = store.install("1.12.2", None).await.expect("install 1.12.2");
    assert_eq!(v112.json.java_major(), 8);
    assert!(v112.json.legacy_arguments.is_some());
    assert!(v112.logging.is_some(), "the patched Log4j configuration must be part of the install");
    assert!(v112.logical_assets.is_none());
    assert!(v112.natives.len() >= 3, "lwjgl, jinput, text2speech natives");
    let files = v112.verify_for_launch(true, 8).expect("re-verification");
    let natives_dir = root.join("natives");
    std::fs::create_dir(&natives_dir).unwrap();
    let mut count = 0;
    for (_stored, exclude, file) in &files.natives {
        count += extract_natives(file, exclude, &natives_dir).expect("extract natives");
    }
    let mut names: Vec<String> =
        std::fs::read_dir(&natives_dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(count, names.len());
    for wanted in ["liblwjgl64.so", "libopenal64.so", "libjinput-linux64.so"] {
        assert!(names.iter().any(|n| n == wanted), "{wanted} missing from {names:?}");
    }
    assert!(
        names.iter().all(|n| n.ends_with(".so") || n.contains(".so.")),
        "only shared objects are unpacked: {names:?}"
    );
    // Unpacking twice into the same directory must fail (O_EXCL), never overwrite.
    assert!(extract_natives(&files.natives[0].2, &files.natives[0].1, &natives_dir).is_err());

    // --- 1.5.2: launchwrapper era, assets must appear under their logical names -----------
    let v15 = store.install("1.5.2", None).await.expect("install 1.5.2");
    assert_eq!(v15.json.main_class, "net.minecraft.launchwrapper.Launch");
    assert!(v15.assets.map_to_resources);
    let tree = v15.logical_assets.clone().expect("logical asset tree");
    let sample = v15.assets.objects.keys().next().unwrap();
    assert!(tree.join(sample).is_file(), "{sample} missing from the logical tree");
    v15.verify_for_launch(true, 8).expect("logical tree verifies against the index");
    // A file in the logical tree that no longer matches the index is caught at launch.
    let victim = tree.join(sample);
    let original = std::fs::read(&victim).unwrap();
    std::fs::remove_file(&victim).unwrap(); // (it is a hard link; replace rather than edit the shared blob)
    std::fs::write(&victim, b"tampered").unwrap();
    assert!(matches!(v15.verify_for_launch(true, 8), Err(Error::Integrity(_))));
    std::fs::remove_file(&victim).unwrap();
    std::fs::write(&victim, original).unwrap();

    // --- pinning and unknown versions ------------------------------------------------------
    let right = Expected::sha1(&v15.json_sha1).unwrap();
    store.install("1.5.2", Some(&right)).await.expect("matching pin");
    let wrong = Expected::sha1(&"0".repeat(40)).unwrap();
    assert!(
        matches!(store.install("1.5.2", Some(&wrong)).await, Err(Error::Integrity(_))),
        "a profile pin that differs must refuse"
    );
    assert!(matches!(store.install("9.99.99", None).await, Err(Error::Usage(_))));

    std::fs::remove_dir_all(&root).unwrap();
}
