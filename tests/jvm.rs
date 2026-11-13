//! The real system JVM under the full sandbox, started exactly the way the launcher starts
//! Minecraft: `java <hardening flags> -cp … BastionBootstrap`, with the main class and the
//! argument vector (including the access token) delivered over the fd-0 socket.

use std::{io::Write, net::Shutdown, path::Path, process::Command, time::Duration};

use bastion_mc::{
    BOOTSTRAP_CLASS, BOOTSTRAP_CLASS_NAME,
    config::JavaMatch,
    java,
    mojang::args::{Piece, Vars, build_frame},
    sandbox::{
        self, BaseOptions, Mount, Network, Resolv, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
    secret::Secret,
};
use tokio::io::AsyncReadExt;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

fn compile_test_classes() -> std::path::PathBuf {
    let out = std::env::temp_dir().join(format!("bastion-jvm-test-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/java/EchoMain.java");
    let status =
        Command::new("javac").args(["--release", "8", "-Xlint:-options", "-d"]).arg(&out).arg(src).status().unwrap();
    assert!(status.success(), "javac failed");
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn jvm_starts_sandboxed_and_receives_the_token_out_of_band() {
    let classes = compile_test_classes();
    let jvms = java::discover().expect("system JVMs");
    let jvm = java::select(&jvms, 8, JavaMatch::Newer, None).expect("a JVM");

    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 256,
        jvm: Some(&jvm),
        exec_allow: &[],
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })
    .unwrap();
    base.mounts.push(Mount::RoData {
        data: BOOTSTRAP_CLASS.to_vec(),
        dst: format!("/bastion/boot/{BOOTSTRAP_CLASS_NAME}.class").into(),
        mode: 0o444,
    });
    base.mounts.push(Mount::RoBind { src: classes.clone(), dst: "/bastion/test".into() });
    base.mounts.push(Mount::Dir { dst: "/game".into(), mode: 0o700 });
    base.landlock.write.push("/game".into());
    base.landlock.tcp = TcpPolicy::Ports(vec![]);

    let mut argv = vec!["java".to_string(), "-Xmx256m".into(), "-cp".into(), "/bastion/boot:/bastion/test".into()];
    argv.extend(java::hardening_flags(jvm.major, false));
    argv.push(BOOTSTRAP_CLASS_NAME.into());

    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::None,
        cwd: "/game".into(),
        limits: None,
        policy: InitPolicy {
            exec: jvm.java.clone(),
            argv,
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };

    let token = Secret::from_slice(b"eyJraWQiOiJ.SUPER-SECRET-MINECRAFT-TOKEN.c2ln").unwrap();
    let mut vars = Vars::default();
    vars.set("auth_player_name", "Steve");
    vars.set_secret("auth_access_token", vec![Piece::Sec(&token)]);
    let templates: Vec<String> =
        ["--accessToken", "${auth_access_token}", "--username", "${auth_player_name}", "unicode-✓"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    let frame = build_frame("EchoMain", &templates, &vars).unwrap();

    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    running.channel.write_all(frame.expose()).unwrap();
    running.channel.shutdown(Shutdown::Write).unwrap();

    // While the JVM runs, a different process of the same UID must not find the token on the
    // JVM's command line or environment (what `ps` and /proc expose to everyone).
    let mut out = running.child.stdout.take().unwrap();
    let mut err = running.child.stderr.take().unwrap();
    let (mut so, mut se) = (String::new(), String::new());
    let status = tokio::time::timeout(Duration::from_secs(90), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut so), err.read_to_string(&mut se));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("JVM timed out");
    let get = |k: &str| {
        so.lines()
            .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
            .unwrap_or_else(|| panic!("missing {k}\n--- stdout ---\n{so}\n--- stderr ---\n{se}"))
            .to_string()
    };
    assert_eq!(status.code(), Some(0), "JVM failed\n--- stdout ---\n{so}\n--- stderr ---\n{se}");
    assert_eq!(get("done"), "ok");
    // The frame arrived intact, including the secret and non-ASCII text.
    assert_eq!(get("argc"), "5");
    assert_eq!(get("arg0"), "--accessToken");
    assert_eq!(get("arg1"), "eyJraWQiOiJ.SUPER-SECRET-MINECRAFT-TOKEN.c2ln");
    assert_eq!(get("arg3"), "Steve");
    assert_eq!(get("arg4"), "unicode-✓");
    // …and is visible nowhere that other processes can look.
    assert_eq!(get("token_in_cmdline"), "false");
    assert_eq!(get("token_in_environ"), "false");
    assert_eq!(get("token_in_sun_java_command"), "false");
    assert_eq!(get("sun_java_command"), "BastionBootstrap");
    assert_eq!(get("stdin_after"), "-1", "System.in is empty for everything after the bootstrap");
    // Synthetic identity.
    assert_eq!(get("user_name"), "player");
    assert_eq!(get("user_home"), "/game");
    // Hardening that must be observable from inside.
    assert_eq!(get("deserialize"), "rejected", "jdk.serialFilter=!* must reject deserialisation");
    assert_eq!(get("exec_shell"), "denied", "Runtime.exec of a shell must fail");
    assert_eq!(get("localhost"), "127.0.0.1");
    let _ = std::fs::remove_dir_all(classes);
}

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_rejects_malformed_frames() {
    let jvms = java::discover().expect("system JVMs");
    let jvm = java::select(&jvms, 8, JavaMatch::Newer, None).expect("a JVM");
    for (label, frame, expect_code) in [
        ("empty", Vec::new(), 70),
        ("bad magic", b"XXXX\0\0\0\x01\0\0\0\x01\0\0\0\x01A".to_vec(), 70),
        ("truncated", b"BSTN\0\0\0\x01\0\0\0\x02\0\0\0\x01A".to_vec(), 70),
        ("oversized length", b"BSTN\0\0\0\x01\0\0\0\x01\x7f\xff\xff\xff".to_vec(), 70),
        ("trailing bytes", b"BSTN\0\0\0\x01\0\0\0\x01\0\0\0\x01Aextra".to_vec(), 70),
        ("unknown class", b"BSTN\0\0\0\x01\0\0\0\x01\0\0\0\x0bno.such.Cls".to_vec(), 71),
    ] {
        let mut base = sandbox::base(&BaseOptions {
            tmp_size_mb: 64,
            jvm: Some(&jvm),
            exec_allow: &[],
            resolv: Resolv::None,
            expose_timezone: false,
            trust: None,
        })
        .unwrap();
        base.mounts.push(Mount::RoData {
            data: BOOTSTRAP_CLASS.to_vec(),
            dst: format!("/bastion/boot/{BOOTSTRAP_CLASS_NAME}.class").into(),
            mode: 0o444,
        });
        let mut argv = vec!["java".to_string(), "-Xmx64m".into(), "-cp".into(), "/bastion/boot".into()];
        argv.extend(java::hardening_flags(jvm.major, false));
        argv.push(BOOTSTRAP_CLASS_NAME.into());
        let spec = SandboxSpec {
            mounts: base.mounts,
            network: Network::None,
            cwd: "/tmp".into(),
            limits: None,
            policy: InitPolicy {
                exec: jvm.java.clone(),
                argv,
                env: base.env,
                landlock: base.landlock,
                seccomp: SeccompPolicy::default(),
            },
        };
        let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
        running.channel.write_all(&frame).unwrap();
        running.channel.shutdown(Shutdown::Write).unwrap();
        let mut se = String::new();
        let mut err = running.child.stderr.take().unwrap();
        let status = tokio::time::timeout(Duration::from_secs(60), async {
            err.read_to_string(&mut se).await.unwrap();
            running.child.wait().await.unwrap()
        })
        .await
        .expect("timeout");
        assert_eq!(status.code(), Some(expect_code), "{label}: stderr: {se}");
        assert!(se.contains("bastion-bootstrap:"), "{label}: {se}");
    }
}
