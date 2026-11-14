//! `bastion-mc-gui`: the window. All launcher work happens in the `bastion-mc` process.

use std::{path::PathBuf, process::ExitCode};

use bastion_mc_gui::{Options, app::is_valid_id, backend};

const HELP: &str = "\
bastion-mc-gui — graphical front end for the bastion-mc launcher

USAGE: bastion-mc-gui [--launcher PATH] [--account NAME]

  --launcher PATH   the bastion-mc binary to drive (default: the one next to this program,
                    else /usr/bin/bastion-mc; $PATH is not searched)
  --account NAME    local account label, as for `bastion-mc --account` (default: the launcher's default account)
  --help, --version

Rendering uses the GPU when one is usable and falls back to software otherwise; set
ICED_BACKEND=tiny-skia to force software rendering.";

fn main() -> ExitCode {
    let mut options = Options { launcher: backend::default_launcher(), account: String::new() };
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let value = |args: &mut dyn Iterator<Item = std::ffi::OsString>| args.next();
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            Some("--version" | "-V") => {
                println!("bastion-mc-gui {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            Some("--launcher") => match value(&mut args) {
                Some(path) => options.launcher = PathBuf::from(path),
                None => return usage("--launcher needs a path"),
            },
            Some("--account") => match value(&mut args).and_then(|name| name.into_string().ok()) {
                Some(name) if is_valid_id(&name) => options.account = name,
                _ => return usage("--account needs a name made of a-z, 0-9, '.', '_' or '-'"),
            },
            _ => return usage(&format!("unexpected argument {arg:?}")),
        }
    }
    match bastion_mc_gui::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bastion-mc-gui: {error}");
            ExitCode::FAILURE
        }
    }
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("bastion-mc-gui: {problem}\n\nRun `bastion-mc-gui --help` for usage.");
    ExitCode::from(2)
}
