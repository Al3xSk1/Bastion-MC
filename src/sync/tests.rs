use std::path::PathBuf;

use super::*;

fn real(file: &str) -> Vec<(String, String)> {
    parse_options(&std::fs::read(format!("{}/tests/data/options/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap())
}

fn get<'a>(lines: &'a [(String, String)], key: &str) -> Option<&'a str> {
    lines.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn set(lines: &mut [(String, String)], key: &str, value: &str) {
    lines.iter_mut().find(|(k, _)| k == key).unwrap_or_else(|| panic!("{key} not in the file")).1 = value.into();
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bmc-sync-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn instance(&self, id: &str, options: Option<&[(String, String)]>) -> PathBuf {
        let persist = self.0.join(id).join("persist");
        std::fs::create_dir_all(&persist).unwrap();
        if let Some(lines) = options {
            std::fs::write(persist.join(OPTIONS), encode_options(lines)).unwrap();
        }
        persist
    }
    fn sync_dir(&self) -> PathBuf {
        self.0.join("sync")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn store_with(entries: &[(&str, &str, u32, u64, &str)]) -> store::Store {
    let mut s = store::Store::default();
    for (name, value, dv, when, id) in entries {
        assert!(s.set(name, value, *dv, *when, id), "{name}");
    }
    s
}

#[test]
fn a_setting_changed_in_1_20_1_reaches_26_3_and_1_21_4_in_their_formats() {
    let s = store_with(&[
        ("fov", "0.25", 3465, 10, "f1201"),
        ("key_key.jump", "key.keyboard.j", 3465, 11, "f1201"),
        ("graphicsMode", "2", 3465, 12, "f1201"),
        ("soundCategory_music", "0.3", 3465, 13, "f1201"),
        ("rawMouseInput", "false", 3465, 14, "f1201"),
    ]);
    let new = merge(&s, Some(real("26.3.txt")), None).unwrap();
    assert!(!new.created);
    assert_eq!(get(&new.lines, "fov"), Some("0.25"));
    assert_eq!(get(&new.lines, "key_key.jump"), Some("key.keyboard.j"));
    assert_eq!(get(&new.lines, "soundCategory_music"), Some("0.3"));
    // Fabulous, as Mojang's fixer splits it.
    assert_eq!(get(&new.lines, "improvedTransparency"), Some("true"));
    assert_eq!(get(&new.lines, "cutoutLeaves"), Some("true"));
    assert_eq!(get(&new.lines, "graphicsPreset"), Some("\"custom\""));
    assert_eq!(get(&new.lines, "graphicsMode"), None, "26.3 has no graphicsMode; nothing is added");
    assert_eq!(get(&new.lines, "rawMouseInput"), None, "a setting 26.3 does not have is not added");
    assert_eq!(get(&new.lines, "version"), Some("5023"), "the file keeps its own DataVersion");
    // Everything else is the instance's own, line for line.
    let original = real("26.3.txt");
    assert_eq!(new.lines.len(), original.len());
    for ((k, v), (k0, v0)) in new.lines.iter().zip(&original) {
        assert_eq!(k, k0);
        if !new.taken.contains(k) {
            assert_eq!(v, v0, "{k} changed without being taken from the synced copy");
        }
    }
    let mid = merge(&s, Some(real("1.21.4.txt")), None).unwrap();
    assert_eq!(get(&mid.lines, "graphicsMode"), Some("2"));
    assert_eq!(get(&mid.lines, "rawMouseInput"), Some("false"));
    assert_eq!(get(&mid.lines, "fov"), Some("0.25"));
}

#[test]
fn a_setting_changed_in_26_3_reaches_1_20_1_where_the_meaning_is_certain() {
    let s = store_with(&[
        ("cutoutLeaves", "false", 5023, 1, "fab"),
        ("weatherRadius", "5", 5023, 1, "fab"),
        ("vignette", "false", 5023, 1, "fab"),
        ("improvedTransparency", "false", 5023, 1, "fab"),
        ("key_key.debug.overlay", "key.keyboard.f4", 5023, 2, "fab"),
        ("musicToast", "\"pause_and_toast\"", 5023, 3, "fab"),
        ("menuBackgroundBlurriness", "3", 5023, 4, "fab"),
        ("lang", "de_de", 5023, 5, "fab"),
    ]);
    let old = merge(&s, Some(real("1.20.1.txt")), None).unwrap();
    assert_eq!(get(&old.lines, "graphicsMode"), Some("0"), "fast: the four parts match its row exactly");
    assert_eq!(get(&old.lines, "lang"), Some("de_de"));
    assert_eq!(get(&old.lines, "key_key.debug.overlay"), None, "1.20.1 has no such binding");
    assert_eq!(get(&old.lines, "menuBackgroundBlurriness"), None, "1.20.1 has no such setting");
    let mid = merge(&s, Some(real("1.21.4.txt")), None).unwrap();
    assert_eq!(get(&mid.lines, "menuBackgroundBlurriness"), Some("3"));
}

#[test]
fn an_instance_without_options_starts_with_the_synced_settings_and_its_own_data_version() {
    let s = store_with(&[("fov", "0.5", 5023, 1, "fab"), ("graphicsMode", "1", 4189, 2, "f1214")]);
    let fresh = merge(&s, None, Some(3465)).unwrap();
    assert!(fresh.created);
    assert_eq!(fresh.lines[0], ("version".to_string(), "3465".to_string()));
    assert_eq!(get(&fresh.lines, "fov"), Some("0.5"));
    assert_eq!(get(&fresh.lines, "graphicsMode"), Some("1"));
    // Without a DataVersion, or for a game older than 1.20.1, nothing is merged.
    assert!(merge(&s, None, None).is_none());
    assert!(merge(&s, None, Some(3337)).is_none());
    let mut old = real("1.20.1.txt");
    set(&mut old, "version", "3337");
    assert!(merge(&s, Some(old), None).is_none());
}

#[test]
fn only_the_settings_changed_in_a_session_go_back_and_the_last_writer_wins() {
    let scratch = Scratch::new("cycle");
    let cfg = SyncConfig::default();
    let a = scratch.instance("a", Some(&real("26.3.txt")));
    let b = scratch.instance("b", Some(&real("1.20.1.txt")));

    // Instance a: nothing synced yet; the user changes the field of view and a binding.
    let (session, notes) = Session::prepare(&cfg, scratch.sync_dir(), "a", &a, None, false, true);
    assert!(notes.iter().any(|n| n.contains("nothing synced yet")), "{notes:?}");
    assert!(session.writes().is_some());
    let mut after = real("26.3.txt");
    set(&mut after, "fov", "0.75");
    set(&mut after, "key_key.jump", "key.keyboard.j");
    set(&mut after, "lastServer", "play.example.org"); // not a synced setting
    set(&mut after, "resourcePacks", "[\"file/x.zip\"]"); // nor this
    std::fs::write(a.join(OPTIONS), encode_options(&after)).unwrap();
    let mut synced = session.sync_back(&a.join(OPTIONS)).unwrap();
    synced.sort();
    assert_eq!(synced, ["fov", "key_key.jump"]);
    // A second sync of the same file changes nothing.
    assert!(session.sync_back(&a.join(OPTIONS)).unwrap().is_empty());

    // Instance b (1.20.1) gets both; its own other settings are untouched.
    let (session_b, notes) = Session::prepare(&cfg, scratch.sync_dir(), "b", &b, None, false, true);
    assert!(notes.iter().any(|n| n.contains("2 setting(s) taken")), "{notes:?}");
    let merged = parse_options(&std::fs::read(b.join(OPTIONS)).unwrap());
    assert_eq!(get(&merged, "fov"), Some("0.75"));
    assert_eq!(get(&merged, "key_key.jump"), Some("key.keyboard.j"));
    assert_eq!(get(&merged, "lastServer"), Some(""), "b's own last server");
    assert_eq!(get(&merged, "version"), Some("3465"));
    // In b the user changes the field of view again: last writer wins; the binding stays.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut b_after = merged.clone();
    set(&mut b_after, "fov", "0.1");
    std::fs::write(b.join(OPTIONS), encode_options(&b_after)).unwrap();
    assert_eq!(session_b.sync_back(&b.join(OPTIONS)).unwrap(), ["fov"]);
    let copy = store::Store::load(&scratch.sync_dir()).unwrap();
    assert_eq!(copy.synced()["fov"].value, "0.1");
    assert_eq!(copy.synced()["fov"].data_version, 3465, "stored in the format of the family that set it");
    assert_eq!(copy.synced()["key_key.jump"].value, "key.keyboard.j");
    assert_eq!(copy.instances(), 2);
    let status = session_b.status();
    assert_eq!(
        (status["sync"].as_bool(), status["keys"].as_u64(), status["instances"].as_u64()),
        (Some(true), Some(2), Some(2))
    );
    assert_eq!(status["last_instance"].as_str(), Some("b"));
    assert!(status["error"].is_null());
}

#[test]
fn a_burner_session_or_sync_switched_off_touches_nothing() {
    let scratch = Scratch::new("off");
    let a = scratch.instance("a", Some(&real("26.3.txt")));
    let before = std::fs::read(a.join(OPTIONS)).unwrap();
    store::update(&scratch.sync_dir(), |s| s.set("fov", "1.0", 5023, 1, "z")).unwrap();
    for (cfg, burner, carried) in [
        (SyncConfig::default(), true, true),
        (SyncConfig { options: false }, false, true),
        (SyncConfig::default(), false, false),
    ] {
        let (session, _) = Session::prepare(&cfg, scratch.sync_dir(), "a", &a, None, burner, carried);
        assert_eq!(std::fs::read(a.join(OPTIONS)).unwrap(), before, "the instance's settings were not changed");
        assert!(session.writes().is_none(), "the confined launcher needs no write access");
        let mut after = real("26.3.txt");
        set(&mut after, "gamma", "1.0");
        std::fs::write(a.join(OPTIONS), encode_options(&after)).unwrap();
        assert!(session.sync_back(&a.join(OPTIONS)).unwrap().is_empty());
        std::fs::write(a.join(OPTIONS), &before).unwrap();
        assert_eq!(session.status()["sync"].as_bool(), Some(false));
    }
    assert_eq!(store::Store::load(&scratch.sync_dir()).unwrap().len(), 1, "the synced copy was not changed");
}

#[test]
fn a_hostile_options_file_can_put_only_listed_settings_of_the_right_shape_into_the_copy() {
    let scratch = Scratch::new("hostile");
    let a = scratch.instance("a", Some(&real("26.3.txt")));
    let (session, _) = Session::prepare(&SyncConfig::default(), scratch.sync_dir(), "a", &a, None, false, true);
    let hostile = "version:5023\nfov:0.9\nlang:../../etc\nkey_key.jump:$(reboot)\ngamma:1e9999\nlastServer:evil\n\
                   resourcePacks:[\"file/evil.zip\"]\nmaxFps:\x1b[31m60\nsoundCategory_master:0.2\nzzz_new:1\n";
    std::fs::write(a.join(OPTIONS), hostile).unwrap();
    let mut synced = session.sync_back(&a.join(OPTIONS)).unwrap();
    synced.sort();
    assert_eq!(synced, ["fov", "soundCategory_master"]);
}

#[test]
fn the_data_version_comes_from_the_client_jars_version_json() {
    let path = std::env::temp_dir().join(format!("bmc-sync-jar-{}.jar", std::process::id()));
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("version.json", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut zip, br#"{"id":"1.20.1","world_version":3465,"protocol_version":763}"#).unwrap();
        zip.finish().unwrap();
    }
    assert_eq!(data_version_of_jar(&std::fs::File::open(&path).unwrap()), Some(3465));
    std::fs::write(&path, b"not a zip").unwrap();
    assert_eq!(data_version_of_jar(&std::fs::File::open(&path).unwrap()), None);
    std::fs::remove_file(&path).unwrap();
    // The real 26.3 jar of the test data root, where it exists.
    let jar = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/e2e/data/bastion-mc/versions/26.3/26.3.jar");
    if let Ok(file) = std::fs::File::open(jar) {
        assert_eq!(data_version_of_jar(&file), Some(5023));
    }
}
