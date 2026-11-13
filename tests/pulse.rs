//! The module-loading probe against a real sound server. Opt-in, because the answer depends
//! on the machine:
//!
//! ```text
//! BASTION_PULSE_SOCKET=$XDG_RUNTIME_DIR/pulse/native BASTION_PULSE_EXPECT=allowed \
//!     cargo test --test pulse -- --nocapture
//! ```
//!
//! `BASTION_PULSE_EXPECT` is `allowed` (PulseAudio, or pipewire-pulse as shipped) or `denied`
//! (pipewire-pulse with `pulse.allow-module-loading = false`).

use std::path::PathBuf;

use bastion_mc::sandbox::pulse::{ModuleLoading, probe};

#[test]
fn the_running_sound_server_answers_as_expected() {
    let (Some(socket), Ok(expect)) = (std::env::var_os("BASTION_PULSE_SOCKET"), std::env::var("BASTION_PULSE_EXPECT"))
    else {
        eprintln!("skipped: set BASTION_PULSE_SOCKET and BASTION_PULSE_EXPECT to run");
        return;
    };
    let answer = probe(&PathBuf::from(socket));
    eprintln!("the server answered: {answer:?}");
    match expect.as_str() {
        "allowed" => assert_eq!(answer, ModuleLoading::Allowed),
        "denied" => assert_eq!(answer, ModuleLoading::Denied),
        other => panic!("BASTION_PULSE_EXPECT must be `allowed` or `denied`, not {other:?}"),
    }
}
