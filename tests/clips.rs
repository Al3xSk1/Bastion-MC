//! Clips: what saving makes, checked with ffprobe; saving from a process confined the way the
//! launcher is in step 9; the phone app's limits; and (opt-in) the recorder itself, in its
//! sandbox, recording a window on a private display.
//!
//! The segments the saving tests read are made by ffmpeg with the recorder's own segment
//! settings, from a test pattern: what the recorder writes, without a display.
//!
//! ```sh
//! # the recorder on a private display (tests/support/private-x11.sh), never a desktop's:
//! BASTION_CLIPS_DISPLAY=:84 cargo test --test clips -- --nocapture
//! ```

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bastion_mc::{
    clips::{self, ClipsConfig, buffer, library},
    confine::{self, Confinement, Demand, Support},
    phone::{Command as PhoneCommand, FrameFormat, Outbound, Outbox},
    spotify::picture,
    util,
};

fn scratch(tag: &str) -> PathBuf {
    let dir = PathBuf::from(format!("/tmp/bmc-{tag}-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
    util::create_private_dir_all(&dir).unwrap();
    dir
}

fn have_ffmpeg() -> bool {
    let ok = Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).status().is_ok_and(|s| s.success())
        && Command::new("ffprobe").arg("-version").stdout(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("skipped: needs ffmpeg and ffprobe");
    }
    ok
}

/// A buffer directory as the recorder leaves it after `seconds` of recording a 480×270
/// window at 30 frames a second.
fn recorded_buffer(dir: &Path, seconds: u32) {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-t", &seconds.to_string()])
        .args(["-f", "lavfi", "-i", "testsrc2=size=480x270:rate=30"])
        .args([
            "-filter_complex",
            "[0:v]format=yuv420p,split=2[rec][th];[th]fps=1/2,scale=w='min(iw,320)':h='min(ih,320)':force_original_aspect_ratio=decrease[thumb]",
        ])
        .args(["-map", "[rec]", "-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-g", "30", "-keyint_min", "30"])
        .args(["-sc_threshold", "0", "-f", "segment", "-segment_time", "1", "-segment_wrap", "16", "-segment_list_size", "12"])
        .arg("-segment_list")
        .arg(dir.join(buffer::LIST))
        .args(["-segment_list_type", "csv", "-reset_timestamps", "0", "-segment_format", "mpegts"])
        .arg(dir.join("r1_%03d.ts"))
        .args(["-map", "[thumb]", "-c:v", "mjpeg", "-q:v", "5", "-f", "image2", "-update", "1"])
        .arg(dir.join(buffer::THUMB))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

struct Probed {
    duration: f64,
    frames: u64,
    width: u64,
    height: u64,
    codec: String,
    decode_errors: String,
}

fn probe(clip: &Path) -> Probed {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=codec_name,width,height,nb_read_frames:format=duration", "-of", "json"])
        .arg(clip)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let stream = &v["streams"][0];
    let decode = Command::new("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-i"])
        .arg(clip)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    Probed {
        duration: v["format"]["duration"].as_str().unwrap().parse().unwrap(),
        frames: stream["nb_read_frames"].as_str().unwrap().parse().unwrap(),
        width: stream["width"].as_u64().unwrap(),
        height: stream["height"].as_u64().unwrap(),
        codec: stream["codec_name"].as_str().unwrap().into(),
        decode_errors: String::from_utf8_lossy(&decode.stderr).into_owned(),
    }
}

fn check_clip(folder: &Path, clip: &library::Clip, seconds: f64) {
    let file = folder.join(&clip.file);
    let p = probe(&file);
    assert_eq!((p.codec.as_str(), p.width, p.height), ("h264", 480, 270));
    assert!((p.duration - seconds).abs() < 0.25, "duration {} for {seconds} s", p.duration);
    let expected = (seconds * 30.0) as i64;
    assert!((p.frames as i64 - expected).abs() <= 2, "{} frames for {seconds} s", p.frames);
    assert!(p.decode_errors.is_empty(), "the joined stream decodes cleanly: {}", p.decode_errors);
    assert_eq!(clip.bytes, std::fs::metadata(&file).unwrap().len());
    let thumb = std::fs::read(folder.join(clip.thumb.as_ref().expect("a thumbnail"))).unwrap();
    assert_eq!(picture::png_dimensions(&thumb), Some((320, 180)), "the thumbnail fits 320×320");
}

#[test]
fn saving_joins_the_newest_segments_into_one_clean_stream() {
    if !have_ffmpeg() {
        return;
    }
    let dir = scratch("save");
    let (buffer_dir, folder) = (dir.join("buffer"), dir.join("clips"));
    util::create_private_dir_all(&buffer_dir).unwrap();
    util::create_private_dir_all(&folder).unwrap();
    recorded_buffer(&buffer_dir, 8);
    let clip = clips::save_blocking(&buffer_dir, &folder, 5.0, "test clip", "0123456789ab").unwrap();
    assert_eq!(clip.seconds, 5.0);
    assert_eq!(clip.file, "test_clip-0123456789ab.ts");
    check_clip(&folder, &clip, 5.0);
    // Asking for more than was recorded gives what there is.
    let all = clips::save_blocking(&buffer_dir, &folder, 30.0, "all of it", "0123456789ac").unwrap();
    check_clip(&folder, &all, 8.0);
    // Nothing but the two clips and their thumbnails was written.
    let mut names: Vec<String> =
        std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "all_of_it-0123456789ac.png",
            "all_of_it-0123456789ac.ts",
            "test_clip-0123456789ab.png",
            "test_clip-0123456789ab.ts"
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_damaged_buffer_is_refused_not_saved() {
    let dir = scratch("damaged");
    let (buffer_dir, folder) = (dir.join("buffer"), dir.join("clips"));
    util::create_private_dir_all(&buffer_dir).unwrap();
    util::create_private_dir_all(&folder).unwrap();
    // A list naming a segment that is a link to a file elsewhere.
    std::fs::write(dir.join("secret"), vec![0x47u8; 188]).unwrap();
    std::os::unix::fs::symlink(dir.join("secret"), buffer_dir.join("r1_000.ts")).unwrap();
    std::fs::write(buffer_dir.join(buffer::LIST), "r1_000.ts,0.000000,1.000000\n").unwrap();
    let refused = clips::save_blocking(&buffer_dir, &folder, 5.0, "x", "0123456789ab").unwrap_err();
    assert!(refused.contains("cannot be opened"), "{refused}");
    // Bytes that are not transport-stream packets.
    std::fs::remove_file(buffer_dir.join("r1_000.ts")).unwrap();
    std::fs::write(buffer_dir.join("r1_000.ts"), vec![0u8; 188 * 4]).unwrap();
    assert!(clips::save_blocking(&buffer_dir, &folder, 5.0, "x", "0123456789ab").unwrap_err().contains("sync"));
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0, "nothing is left behind of a refused save");
    std::fs::remove_dir_all(&dir).unwrap();
}

const CONFINED_CHILD: &str = "BASTION_CLIPS_CONFINED_CHILD";

/// The re-executed half of the next test: confines itself as the launcher does in step 9 —
/// the baseline plus exactly what the clips service adds (the folder and the buffer, to
/// write) — and saves a clip.
#[test]
fn confined_child() {
    let Ok(dir) = std::env::var(CONFINED_CHILD) else { return };
    let dir = PathBuf::from(dir);
    let (buffer_dir, folder) = (dir.join("buffer"), dir.join("clips"));
    let mut keep = Confinement::supervisor();
    keep.write.extend([folder.clone(), buffer_dir.clone()]);
    let applied = confine::apply(&keep, Demand::Whole).expect("confining the child");
    assert!(applied.is_whole());
    match clips::save_blocking(&buffer_dir, &folder, 4.0, "confined", "0123456789ad") {
        Ok(clip) => println!("\nsaved={} seconds={} bytes={}", clip.file, clip.seconds, clip.bytes),
        Err(why) => println!("\nsave failed: {why}"),
    }
    // Control: the confinement is in force — nothing outside those two directories can be
    // written, and nothing can be executed.
    let outside = std::fs::write(dir.join("outside"), b"x").map_err(|e| e.raw_os_error());
    println!("write_outside={outside:?}");
    let exec = Command::new("/usr/bin/true").status().map(|_| ()).map_err(|e| e.raw_os_error());
    println!("exec={exec:?}");
}

/// Saving needs nothing a confined launcher cannot do: no program is executed (the segments
/// are joined by the launcher itself), and it writes only the folder and reads the buffer.
#[test]
fn a_confined_launcher_can_save_a_clip() {
    if !have_ffmpeg() {
        return;
    }
    if confine::support() != Support::Whole {
        eprintln!("skipped: {:?}", confine::support());
        return;
    }
    let dir = scratch("confined");
    let (buffer_dir, folder) = (dir.join("buffer"), dir.join("clips"));
    util::create_private_dir_all(&buffer_dir).unwrap();
    util::create_private_dir_all(&folder).unwrap();
    recorded_buffer(&buffer_dir, 6);
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["confined_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CONFINED_CHILD, &dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("saved=confined-0123456789ad.ts seconds=4"), "{stdout}");
    assert!(stdout.contains(&format!("write_outside=Err(Some({}))", libc::EACCES)), "{stdout}");
    assert!(stdout.contains("exec=Err("), "nothing can be executed:\n{stdout}");
    let index = library::Library::open(&folder);
    assert!(index.clips.is_empty(), "the child called the save routine only; the app keeps the index");
    let clip = library::Clip {
        id: "0123456789ad".into(),
        name: "confined".into(),
        file: "confined-0123456789ad.ts".into(),
        thumb: Some("confined-0123456789ad.png".into()),
        created: 0,
        seconds: 4.0,
        bytes: std::fs::metadata(folder.join("confined-0123456789ad.ts")).unwrap().len(),
    };
    check_clip(&folder, &clip, 4.0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// States, frames (slot, width, height, format, bytes) and notices the app sent.
type Drained = (Vec<serde_json::Value>, Vec<(u8, u16, u16, FrameFormat, Vec<u8>)>, Vec<String>);

fn drain(rx: &mut tokio::sync::mpsc::Receiver<Outbound>) -> Drained {
    let (mut states, mut frames, mut notices) = (Vec::new(), Vec::new(), Vec::new());
    while let Ok(m) = rx.try_recv() {
        match m {
            Outbound::State { json, .. } => states.push(serde_json::from_str(&json).unwrap()),
            Outbound::Frame { slot, width, height, format, bytes, .. } => {
                frames.push((slot, width, height, format, bytes))
            }
            Outbound::Notice { text, .. } => notices.push(text),
        }
    }
    (states, frames, notices)
}

async fn next_state(rx: &mut tokio::sync::mpsc::Receiver<Outbound>, notices: &mut Vec<String>) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        while let Ok(m) = rx.try_recv() {
            match m {
                Outbound::State { json, .. } => return serde_json::from_str(&json).unwrap(),
                Outbound::Notice { text, .. } => notices.push(text),
                Outbound::Frame { .. } => {}
            }
        }
        assert!(Instant::now() < deadline, "no state in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The app as the phone uses it, over a recorded buffer: save, list, thumbnails, rename,
/// delete — and each limit: saves per minute, the number of clips, names, ids.
#[tokio::test(flavor = "multi_thread")]
async fn the_clips_app_saves_lists_and_keeps_its_limits() {
    if !have_ffmpeg() {
        return;
    }
    let dir = scratch("app");
    let (buffer_dir, folder) = (dir.join("buffer"), dir.join("clips"));
    util::create_private_dir_all(&buffer_dir).unwrap();
    util::create_private_dir_all(&folder).unwrap();
    recorded_buffer(&buffer_dir, 6);
    std::fs::write(folder.join("my-own-video.ts"), b"not this feature's").unwrap();
    let cfg = ClipsConfig { enabled: true, saves_per_minute: 3, max_clips: 2, ..ClipsConfig::default() };
    let logged = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = logged.clone();
    let mut app =
        clips::app_over_buffer(cfg, &folder, &buffer_dir, "fab", Arc::new(move |l| sink.lock().unwrap().push(l)));
    assert_eq!(app.name(), "clips");
    let (out, mut rx) = Outbox::new(64);
    let cmd = |line: &str| PhoneCommand::parse(line.as_bytes()).unwrap();
    let mut notices = Vec::new();

    app.hello(&out);
    let state = next_state(&mut rx, &mut notices).await;
    assert_eq!(state["recording"], true);
    assert_eq!(state["buffer_seconds"], 30);
    assert_eq!(state["clips"].as_array().unwrap().len(), 0);
    let folder_shown = state["folder"].as_str().unwrap();
    assert!(!folder_shown.contains(&std::env::var("USER").unwrap_or_else(|_| "ski".into())), "{folder_shown}");

    app.command(cmd("clips save 3"), &out);
    let state = next_state(&mut rx, &mut notices).await;
    let first = state["clips"][0].clone();
    assert!(notices.iter().any(|n| n.starts_with("saved fab ") && n.ends_with("(3.0 s)")), "{notices:?}");
    for field in ["id", "name", "created", "seconds", "bytes"] {
        assert!(!first[field].is_null(), "clip field {field}: {first}");
    }
    assert_eq!(first["seconds"], 3.0);
    let id = first["id"].as_str().unwrap().to_string();

    app.command(cmd(&format!("clips thumb {id}")), &out);
    let deadline = Instant::now() + Duration::from_secs(10);
    let frame = loop {
        let (_, frames, _) = drain(&mut rx);
        if let Some(f) = frames.into_iter().next() {
            break f;
        }
        assert!(Instant::now() < deadline, "no thumbnail frame");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!((frame.0, frame.1, frame.2, frame.3), (0, 320, 180, FrameFormat::Png), "slot = its place in the list");

    app.command(cmd(&format!("clips rename {id} -- Best ../ moment!")), &out);
    let state = next_state(&mut rx, &mut notices).await;
    assert_eq!(state["clips"][0]["name"], "Best moment");
    assert!(folder.join(format!("Best_moment-{id}.ts")).exists());

    // Saves per minute: three, then refused.
    app.command(cmd("clips save 2"), &out);
    next_state(&mut rx, &mut notices).await;
    app.command(cmd("clips save 2"), &out);
    let state = next_state(&mut rx, &mut notices).await;
    // At most two clips: the oldest of this feature's went; the user's own file stayed.
    assert_eq!(state["clips"].as_array().unwrap().len(), 2);
    assert!(!folder.join(format!("Best_moment-{id}.ts")).exists(), "the oldest clip was removed for the cap");
    assert!(folder.join("my-own-video.ts").exists());
    assert!(logged.lock().unwrap().iter().any(|l| l.contains("removed the oldest clip, Best moment")));
    app.command(cmd("clips save 2"), &out);
    let state = next_state(&mut rx, &mut notices).await;
    assert_eq!(state["error"], "at most 3 saves a minute");

    for (line, why) in [
        ("clips save 31", "save takes 1 to 30 seconds"),
        ("clips save -- 5", "save takes at most a number of seconds"),
        ("clips delete ../../etc", "delete takes a clip id"),
        ("clips delete 000000000000", "no such clip"),
        ("clips thumb 000000000000", "that clip is not in the last list"),
        ("clips rename 000000000000", "rename takes a clip id and a name: rename <id> -- <name>"),
        ("clips list now", "list takes no arguments"),
    ] {
        app.command(cmd(line), &out);
        let state = next_state(&mut rx, &mut notices).await;
        assert_eq!(state["error"], why, "{line}");
    }
    let newest = state["clips"][0]["id"].as_str().unwrap().to_string();
    app.command(cmd(&format!("clips delete {newest}")), &out);
    let state = next_state(&mut rx, &mut notices).await;
    assert_eq!(state["clips"].as_array().unwrap().len(), 1);
    assert_eq!(state["error"], serde_json::Value::Null);
    drop(app);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The recorder in its sandbox, on a private display: it finds the one window there, records
/// it, and a save makes a clip of what it shows.
#[tokio::test(flavor = "multi_thread")]
async fn the_recorder_records_a_window_on_a_private_display() {
    let Ok(display) = std::env::var("BASTION_CLIPS_DISPLAY") else {
        eprintln!("skipped: set BASTION_CLIPS_DISPLAY to a private display (tests/support/private-x11.sh)");
        return;
    };
    if !have_ffmpeg() {
        return;
    }
    let number = display.trim_start_matches(':');
    let socket = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
    // A window with a moving test pattern: ffplay, drawn through X11.
    let mut window = Command::new("ffplay")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=480x270:rate=30",
            "-window_title",
            "clip-probe",
        ])
        .env_remove("WAYLAND_DISPLAY")
        .env("DISPLAY", &display)
        .env("XAUTHORITY", "/dev/null")
        .env("SDL_VIDEODRIVER", "x11")
        .env("SDL_AUDIODRIVER", "dummy")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("ffplay");
    let dir = PathBuf::from(format!(
        "/run/user/{}/bmc-rec-{}",
        nix::unistd::getuid(),
        util::hex_encode(&util::random_bytes::<3>().unwrap())
    ));
    let (buffer_dir, folder) = (dir.join("buffer"), dir.join("clips"));
    util::create_private_dir_all(&buffer_dir).unwrap();
    util::create_private_dir_all(&folder).unwrap();
    let settings = clips::recorder::Settings::new(clips::recorder::Mode::Alone, 10, 30, 1280);
    let mut recorder = clips::recorder::Recorder::start(
        &clips::recorder::Target::Nested(socket),
        &buffer_dir,
        &settings,
        Path::new(env!("CARGO_BIN_EXE_bastion-mc")),
        bastion_mc::sandbox::policy::SeccompPolicy::default(),
        None,
    )
    .expect("the recorder's sandbox");
    use tokio::io::AsyncBufReadExt;
    let mut status = tokio::io::BufReader::new(recorder.running.child.stdout.take().unwrap()).lines();
    let errors = recorder.running.child.stderr.take().unwrap();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(errors).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("recorder: {line}");
        }
    });
    let recording = tokio::time::timeout(Duration::from_secs(20), async {
        while let Ok(Some(line)) = status.next_line().await {
            eprintln!("status: {line}");
            if let Some(clips::recorder::Status::Recording { width, height }) =
                clips::recorder::Status::parse(line.as_bytes())
            {
                return Some((width, height));
            }
        }
        None
    })
    .await
    .expect("the recorder found the window in time")
    .expect("a recording status");
    eprintln!("recording a {}×{} window", recording.0, recording.1);
    tokio::time::sleep(Duration::from_secs(6)).await;
    let clip = tokio::task::spawn_blocking({
        let (buffer_dir, folder) = (buffer_dir.clone(), folder.clone());
        move || clips::save_blocking(&buffer_dir, &folder, 4.0, "probe", "0123456789ae")
    })
    .await
    .unwrap()
    .expect("a clip");
    let p = probe(&folder.join(&clip.file));
    eprintln!("clip: {} s, {} frames, {}×{}, {} bytes", p.duration, p.frames, p.width, p.height, clip.bytes);
    assert_eq!(p.codec, "h264");
    assert!(p.width <= 1280 && p.width.is_multiple_of(2) && p.height.is_multiple_of(2));
    assert!((p.duration - 4.0).abs() < 0.6, "{}", p.duration);
    assert!(p.frames >= 90 && p.frames <= 125, "{} frames", p.frames);
    let thumb = std::fs::read(folder.join(clip.thumb.unwrap())).unwrap();
    let (tw, th) = picture::png_dimensions(&thumb).unwrap();
    assert!(tw <= 320 && th <= 320, "{tw}×{th}");
    drop(recorder);
    let _ = window.kill();
    let _ = window.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
