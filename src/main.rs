//! Entry point.
//!
//! First of all, by the name it was started under: as `mcpelauncher-webview` this program is
//! the Bedrock sign-in bridge's stub inside the game's sandbox (`bedrock::signin::stub`), and
//! it does that and nothing else — no configuration, no hardening calls the sandbox's filter
//! may not allow, no files.
//!
//! The internal re-exec modes are dispatched before anything else: two must run
//! single-threaded and exist only to `execve` onward (the network-namespace helper into
//! bubblewrap, the init stage into the JVM), the third is the nested compositor's
//! placeholder child, the fourth the Realms dialer (inside a sandbox of its own, see
//! `realms::dial`), the fifth the clip recorder's helper (inside its sandbox), the sixth the
//! browser's stage (single-threaded too: it forks Firefox). Everything else gets the
//! launcher-side process hardening first — the Spotify token keeper too, which is
//! dispatched right after it.

use std::ffi::OsString;

use bastion_mc::{bedrock, browser, cli, clips, hardening, realms, sandbox, spotify};

/// Every heap block in this process is wiped before it is returned to the allocator.
#[global_allocator]
static ALLOCATOR: hardening::ZeroOnFree = hardening::ZeroOnFree;

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();
    // The sign-in bridge's stub, mounted in the Bedrock sandbox under the webview's name.
    if bedrock::signin::is_stub(args.first()) {
        bedrock::signin::stub::main(&args[1..]);
    }
    match args.get(1).and_then(|a| a.to_str()) {
        Some(sandbox::init::INIT_ARG) => sandbox::init::main(&args[2..]),
        Some(sandbox::netns::NETNS_ARG) => sandbox::netns::main(&args[2..]),
        Some(sandbox::nested::READY_ARG) => sandbox::nested::ready_main(),
        Some(realms::dial::DIAL_ARG) => realms::dial::main(&args[2..]),
        // The clip recorder's helper, inside the recorder's sandbox (whose init stage
        // started it, after applying the sandbox's restrictions).
        Some(clips::recorder::RECORDER_ARG) => clips::recorder::recorder_main(&args[2..]),
        // The browser's sandbox: hardens itself, then bridges to Firefox (see `browser::stage`).
        Some(browser::stage::STAGE_ARG) => browser::stage::main(&args[2..]),
        _ => {}
    }

    let hardening = hardening::harden_process();
    // The Spotify token keeper (started by a launch, before step 9): hardened like the
    // launcher, then nothing but its one job.
    if args.get(1).and_then(|a| a.to_str()) == Some(spotify::keeper::KEEPER_ARG) {
        spotify::keeper::keeper_main();
    }
    let code = match cli::run(args.into_iter().skip(1).collect(), hardening) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("bastion-mc: {err}");
            err.exit_code()
        }
    };
    std::process::exit(code);
}
