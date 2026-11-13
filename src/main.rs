//! Entry point.
//!
//! The internal re-exec modes are dispatched before anything else: two must run
//! single-threaded and exist only to `execve` onward (the network-namespace helper into
//! bubblewrap, the init stage into the JVM), the third is the nested compositor's
//! placeholder child. Everything else gets the launcher-side process hardening first.

use std::ffi::OsString;

use bastion_mc::{cli, hardening, sandbox};

/// Every heap block in this process is wiped before it is returned to the allocator.
#[global_allocator]
static ALLOCATOR: hardening::ZeroOnFree = hardening::ZeroOnFree;

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();
    match args.get(1).and_then(|a| a.to_str()) {
        Some(sandbox::init::INIT_ARG) => sandbox::init::main(&args[2..]),
        Some(sandbox::netns::NETNS_ARG) => sandbox::netns::main(&args[2..]),
        Some(sandbox::nested::READY_ARG) => sandbox::nested::ready_main(),
        _ => {}
    }

    let hardening = hardening::harden_process();
    let code = match cli::run(args.into_iter().skip(1).collect(), hardening) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("bastion-mc: {err}");
            err.exit_code()
        }
    };
    std::process::exit(code);
}
