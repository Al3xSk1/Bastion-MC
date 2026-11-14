//! Command-line interface. Deliberately a plain CLI: no TUI toolkit, no terminal-control
//! library in the same process that holds tokens.
//!
//! The same rule is why the graphical front end is a different program. It runs this one
//! with `--events` as its first argument and reads typed progress events from standard
//! output (see `events` and the `bastion-mc-proto` crate); every command a front end needs
//! reports through a [`Reporter`] instead of printing.

use std::{
    ffi::OsString,
    fs::OpenOptions,
    future::Future,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use crate::{
    auth::{self, identity, msa, registry, store::TokenStore},
    config::{AuthFlow, Config, validate_version_id},
    doctor,
    error::{Context, Error, Result},
    events::{self, Event, Reporter, StopSignal},
    hardening::Hardening,
    instance::{self, Kind},
    launch::{self, LaunchOptions},
    mojang::install::Store,
    paths::{Paths, validate_id},
    profile::{
        author, capabilities,
        manifest::LoaderKind,
        mods,
        signing::{PublicKey, SecretKey, TrustStore},
    },
    util,
};

pub const EXAMPLE_CONFIG: &str = include_str!("../examples/config.toml");

const HELP: &str = "\
bastion-mc — sandboxed, zero-trust Minecraft launcher for Arch Linux

USAGE: bastion-mc [--events] <command> [options]

  status [--account NAME]             sign-in state and instances at a glance
  doctor                              check kernel, sandbox and trust setup (starts a test sandbox)
  config path | example               show the config file location / print a commented example

  login  [--device] [--account NAME]  sign in with your own Azure client ID
  logout [--account NAME]             forget the stored refresh token
  account list                        every local account and its sign-in state
  account default [NAME]              show or set the account used when --account is not given

  vault reseal [--account NAME]       re-encrypt the age vault to the recipients configured now
  realms totp-setup [--replace]       make the TOTP secret for approving Realms joins ([realms] approve =
                                      \"totp\"); shown once, on this terminal, and kept in the token store

  skin show   [--account NAME]        the account's skins and capes, as Mojang reports them
  skin set <file.png> [--slim] [--account NAME]   upload a skin (metadata is stripped first)
  skin reset  [--account NAME]        back to the default skin
  skin export <file.png> [--account NAME]         save the active skin
  cape set <alias|none> [--account NAME]          wear one of the account's capes, or none
  canary show   [--account NAME]      the pinned profile state and its history (no network)
  canary accept [--account NAME]      pin the account's current profile after a change made elsewhere

  spotify login [--open]              sign in to the Spotify Web API with your own [spotify] client_id
                                      (the refresh token goes to the token store; --open starts your browser)
  spotify logout                      forget the stored Spotify refresh token
  spotify status [--offline]          the desktop app over MPRIS, album art and the Web API: set up, reachable,
                                      signed in (--offline: from the configuration and the store only)

  scram [--yes]                       EMERGENCY: stop running games, remove every stored token (no network)
  wiki scram [--links]                the built-in recovery runbook for a compromised account
  export-proofs <file> [--account NAME]   what this launcher recorded that proves the account is yours

  versions [--snapshots] [--limit N]  list Minecraft versions from Mojang's manifest
  instance new <id> --minecraft <version>     create a vanilla instance
  instance list
  install <id> [--repair]             download and verify an instance's files without launching
  launch  <id> [--account NAME] [--join HOST[:PORT]] [--demo] [--burner] [--repair] [--quiet]

  pack list   <id>                    resource and shader packs of an instance
  pack import <id> <file.zip> [--shader] [--name NAME.zip]   check, hash and add a pack
  pack remove <id> <NAME.zip> [--shader]
  pack adopt  <id>                    record packs you copied into the instance by hand

  key generate <file>                 create an Ed25519 signing key (file mode 0600)
  key public <file>                   print the public half of a signing key
  key trust <name> <public-key|file>  trust a key for signing profiles (per-user store)
  key list                            show trusted keys and which store is authoritative

  profile new <id> --minecraft <version> (--fabric | --quilt) [--loader-version V]
  profile add <id> <mod>... [--prerelease] [--allow-fresh] [--dry-run]
                                      fetch mods from Modrinth into the draft: <mod> is a slug, a project id or
                                      an alias (viafabric, simple-voice-chat, fabric-api, sodium, lithium, modmenu).
                                      Newest release for the profile's Minecraft version and loader, required
                                      dependencies included, SHA-512 verified; nothing is signed.
                                      --prerelease: a beta or alpha where there is no release; --allow-fresh:
                                      also builds and files less than 48 hours old; --dry-run: only show the plan
  profile check <id>                  ask Modrinth about the hash of every jar in mods/: known, unknown, fresh
  profile lock <id>                   record the current mods/ directory in the manifest (and say which
                                      sensitive Java APIs each updated mod newly refers to)
  profile audit <id>                  list, per mod jar, the sensitive APIs its classes refer to
  profile sign <id> --key <file>      sign the manifest
  profile verify <id>                 verify signature and mods/ exactly as a launch would

  --help, --version
  --events    (first argument) report progress as JSON lines on standard output, for a front
              end such as bastion-mc-gui: status, login, logout, account, versions, instance, install, launch, scram

--demo starts the client in Mojang's own demo mode with no account: useful for testing the
sandbox before your Azure application is approved. No token exists in that mode.

--burner is for content you do not trust (a downloaded world, a resource pack): no network,
no sign-in, no token, and the game works on copies of the instance's worlds and settings
that are deleted when it exits. Nothing it writes is kept.";

fn usage(msg: impl Into<String>) -> Error {
    Error::Usage(format!("{}\n\nRun `bastion-mc --help` for usage.", msg.into()))
}

struct Args {
    parser: lexopt::Parser,
}

impl Args {
    fn positional(&mut self, what: &str) -> Result<String> {
        match self.parser.next().map_err(|e| usage(e.to_string()))? {
            Some(lexopt::Arg::Value(v)) => v.into_string().map_err(|_| usage(format!("{what} is not valid UTF-8"))),
            _ => Err(usage(format!("missing {what}"))),
        }
    }

    fn value(&mut self, flag: &str) -> Result<String> {
        self.parser
            .value()
            .map_err(|_| usage(format!("{flag} needs a value")))?
            .into_string()
            .map_err(|_| usage(format!("{flag} value is not valid UTF-8")))
    }

    /// Remaining `--flags`, dispatched to `f`; positionals are an error at this point.
    fn flags(&mut self, mut f: impl FnMut(&str, &mut Self) -> Result<bool>) -> Result<()> {
        loop {
            let arg = self.parser.next().map_err(|e| usage(e.to_string()))?;
            let name = match arg {
                None => return Ok(()),
                Some(lexopt::Arg::Long(l)) => l.to_string(),
                Some(lexopt::Arg::Short(c)) => return Err(usage(format!("unknown option -{c}"))),
                Some(lexopt::Arg::Value(v)) => return Err(usage(format!("unexpected argument {v:?}"))),
            };
            if !f(&name, self)? {
                return Err(usage(format!("unknown option --{name}")));
            }
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().ctx(|| "starting the async runtime")
}

/// Run a command's asynchronous part.
///
/// In event mode the front end may ask for a stop at any time. Once the command supervises a
/// running game it handles that itself (graceful shutdown). Before that — while downloading,
/// verifying, or waiting for a sign-in — the command is cancelled by dropping its future, so
/// everything it holds is released the ordinary way.
fn block_on<T>(reporter: &Reporter, stop: &StopSignal, future: impl Future<Output = Result<T>>) -> Result<T> {
    let runtime = runtime()?;
    let result = runtime.block_on(async {
        if !reporter.is_events() {
            return future.await;
        }
        let control = async {
            reporter.stop_requested().await;
            stop.trigger();
            std::future::pending::<()>().await
        };
        tokio::select! {
            result = future => result,
            _ = control => unreachable!("the control channel future never completes"),
            _ = stop.wait_unsupervised() => Err(Error::Cancelled),
        }
    });
    // A cancelled command may have left read-only work (hashing) on a blocking thread;
    // there is no reason to wait for it.
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}

pub fn run(argv: Vec<OsString>, hardening: Hardening) -> Result<i32> {
    let mut args = Args { parser: lexopt::Parser::from_args(argv) };
    let mut machine = false;
    let command = loop {
        match args.parser.next().map_err(|e| usage(e.to_string()))? {
            None if machine => return Err(usage("--events needs a command")),
            None => {
                println!("{HELP}");
                return Ok(2);
            }
            Some(lexopt::Arg::Long("events")) if !machine => machine = true,
            Some(lexopt::Arg::Long("help")) | Some(lexopt::Arg::Short('h')) => {
                println!("{HELP}");
                return Ok(0);
            }
            Some(lexopt::Arg::Long("version")) | Some(lexopt::Arg::Short('V')) => {
                println!("bastion-mc {}", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            Some(lexopt::Arg::Value(v)) => break v.into_string().map_err(|_| usage("command is not valid UTF-8"))?,
            Some(other) => return Err(usage(format!("unexpected {other:?}"))),
        }
    };

    if !machine {
        return dispatch(&mut args, &command, &Reporter::human(false), hardening);
    }
    // Event mode: the outcome, good or bad, is part of the stream; nothing is printed as text.
    let reporter = Reporter::events();
    let code = match dispatch(&mut args, &command, &reporter, hardening) {
        Ok(code) => code,
        Err(err) => {
            let code = err.exit_code();
            reporter.emit(Event::Error { message: err.to_string(), code });
            code
        }
    };
    reporter.finish();
    Ok(code)
}

/// What `status` and the account commands report about one account. Never a credential.
fn account_summary(paths: &Paths, cfg: &Config, account: &str) -> events::Account {
    let (signed_in, store) = match TokenStore::open(&cfg.auth, paths) {
        Ok(store) => match store.has(account) {
            Ok(present) => (present, store.describe().to_string()),
            Err(e) => (false, format!("{} — unavailable: {e}", store.describe())),
        },
        Err(e) => (false, format!("unavailable: {e}")),
    };
    events::Account {
        account: account.to_string(),
        signed_in,
        player: identity::recall(paths, account).filter(|_| signed_in).map(|i| i.player),
        store,
        client_id_configured: cfg.auth.client_id.is_some(),
        device_flow: cfg.auth.flow == AuthFlow::Device,
    }
}

/// Every instance directory, launchable or not (the reason is part of the entry).
fn instance_summaries(paths: &Paths) -> Result<Vec<events::Instance>> {
    Ok(instance::list(paths)?
        .into_iter()
        .map(|id| match instance::load(paths, &id) {
            Ok(inst) => {
                let (kind, detail) = match &inst.kind {
                    Kind::Vanilla => ("vanilla", None),
                    Kind::Curated { manifest, signed_by } => {
                        ("curated", Some(format!("serial {}, signed by {signed_by}", manifest.serial)))
                    }
                };
                events::Instance { id, minecraft: Some(inst.minecraft), kind: kind.into(), detail, error: None }
            }
            Err(e) => events::Instance { id, kind: "unknown".into(), error: Some(e.to_string()), ..Default::default() },
        })
        .collect())
}

fn print_instances(instances: &[events::Instance]) {
    for i in instances {
        match (&i.error, &i.minecraft) {
            (Some(error), _) => println!("{}\tNOT LAUNCHABLE: {error}", i.id),
            (None, minecraft) => println!(
                "{}\tMinecraft {}\t{}{}",
                i.id,
                minecraft.as_deref().unwrap_or("?"),
                i.kind,
                i.detail.as_ref().map(|d| format!(", {d}")).unwrap_or_default()
            ),
        }
    }
}

fn account_summaries(paths: &Paths, cfg: &Config) -> Event {
    Event::Accounts {
        default: registry::default_account(paths),
        accounts: registry::list(paths).iter().map(|name| account_summary(paths, cfg, name)).collect(),
    }
}

fn print_accounts(event: &Event) {
    let Event::Accounts { default, accounts } = event else { return };
    for a in accounts {
        let state = match (a.signed_in, &a.player) {
            (true, Some(player)) => format!("signed in as {player}"),
            (true, None) => "signed in".to_string(),
            (false, _) => "not signed in".to_string(),
        };
        println!("{}{}\t{state}", a.account, if &a.account == default { " (default)" } else { "" });
    }
}

fn account_flag(args: &mut Args, paths: &Paths) -> Result<String> {
    let mut account = registry::default_account(paths);
    args.flags(|name, a| {
        if name == "account" {
            account = a.value("--account")?;
            return Ok(true);
        }
        Ok(false)
    })?;
    validate_id(&account)?;
    Ok(account)
}

fn dispatch(args: &mut Args, command: &str, reporter: &Reporter, hardening: Hardening) -> Result<i32> {
    let machine = reporter.is_events();
    if machine
        && !matches!(
            command,
            "status" | "login" | "logout" | "account" | "scram" | "versions" | "instance" | "install" | "launch"
        )
    {
        return Err(usage(format!("--events is not available for `{command}`")));
    }
    let stop = StopSignal::never();
    let paths = Paths::from_env()?;
    paths.ensure()?;

    match command {
        "status" => {
            let account = account_flag(args, &paths)?;
            let cfg = Config::load(&paths)?;
            let summary = account_summary(&paths, &cfg, &account);
            let instances = instance_summaries(&paths)?;
            if machine {
                reporter.emit(Event::Account(summary));
                reporter.emit(account_summaries(&paths, &cfg));
                reporter.emit(Event::Instances { instances });
            } else {
                match (summary.signed_in, &summary.player) {
                    (true, Some(player)) => println!("account {account}: signed in as {player}"),
                    (true, None) => println!("account {account}: signed in"),
                    (false, _) => println!("account {account}: not signed in"),
                }
                println!("refresh token store: {}", summary.store);
                if !summary.client_id_configured {
                    println!("auth.client_id is not set: only `launch --demo` works until it is");
                }
                if instances.is_empty() {
                    println!("no instances yet (create one with `bastion-mc instance new`)");
                }
                print_instances(&instances);
            }
        }
        "config" => match args.positional("`path` or `example`")?.as_str() {
            "path" => println!("{}", paths.config_file().display()),
            "example" => print!("{EXAMPLE_CONFIG}"),
            other => return Err(usage(format!("unknown config subcommand {other:?}"))),
        },
        "doctor" => {
            args.flags(|_, _| Ok(false))?;
            let cfg = Config::load(&paths)?;
            return Ok(runtime()?.block_on(doctor::run(&paths, &cfg, &hardening)));
        }
        "login" => {
            let cfg = Config::load(&paths)?;
            let (mut flow, mut account) = (cfg.auth.flow, registry::default_account(&paths));
            args.flags(|name, a| {
                match name {
                    "device" => flow = AuthFlow::Device,
                    "account" => account = a.value("--account")?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
            validate_id(&account)?;
            // A sign-in ends by storing a token, possibly minutes from now: `scram` must be
            // able to find and stop it.
            let _marker = crate::running::Marker::for_command(&paths, "login")?;
            block_on(reporter, &stop, login(&paths, &cfg, flow, &account, reporter))?;
            reporter.emit(Event::Account(account_summary(&paths, &cfg, &account)));
            reporter.emit(account_summaries(&paths, &cfg));
        }
        "account" => {
            let cfg = Config::load(&paths)?;
            match args.positional("`list` or `default`")?.as_str() {
                "list" => args.flags(|_, _| Ok(false))?,
                "default" => {
                    if let Some(lexopt::Arg::Value(name)) = args.parser.next().map_err(|e| usage(e.to_string()))? {
                        let name = name.into_string().map_err(|_| usage("account name is not valid UTF-8"))?;
                        validate_id(&name)?;
                        registry::set_default(&paths, &name)?;
                        reporter.status(format!("default account is now {name}"));
                    } else if !machine {
                        println!("{}", registry::default_account(&paths));
                        return Ok(0);
                    }
                }
                other => return Err(usage(format!("unknown account subcommand {other:?}"))),
            }
            let accounts = account_summaries(&paths, &cfg);
            if machine { reporter.emit(accounts) } else { print_accounts(&accounts) }
        }
        "logout" => {
            let account = account_flag(args, &paths)?;
            let cfg = Config::load(&paths)?;
            TokenStore::open(&cfg.auth, &paths)?.delete(&account)?;
            identity::forget(&paths, &account)?;
            registry::clear_default_if(&paths, &account);
            reporter.status(format!("signed out ({account})"));
            reporter.emit(Event::Account(account_summary(&paths, &cfg, &account)));
            reporter.emit(account_summaries(&paths, &cfg));
        }
        "spotify" => {
            let cfg = Config::load(&paths)?;
            match args.positional("`login`, `logout` or `status`")?.as_str() {
                "login" => {
                    let mut open = false;
                    args.flags(|name, _| {
                        open |= name == "open";
                        Ok(name == "open")
                    })?;
                    // A sign-in ends by storing a token: `scram` must be able to stop it.
                    let _marker = crate::running::Marker::for_command(&paths, "spotify-login")?;
                    block_on(reporter, &stop, crate::spotify::login::login(&paths, &cfg, open, reporter))?;
                }
                "logout" => {
                    args.flags(|_, _| Ok(false))?;
                    crate::spotify::login::logout(&paths, &cfg, reporter)?;
                }
                "status" => {
                    let mut offline = false;
                    args.flags(|name, _| {
                        offline |= name == "offline";
                        Ok(name == "offline")
                    })?;
                    let program = Path::new("/proc/self/exe");
                    block_on(reporter, &stop, crate::spotify::login::status(&paths, &cfg, program, offline, reporter))?;
                }
                other => return Err(usage(format!("unknown spotify subcommand {other:?}"))),
            }
        }
        "versions" => {
            let cfg = Config::load(&paths)?;
            let (mut snapshots, mut limit) = (false, 15usize);
            args.flags(|name, a| {
                match name {
                    "snapshots" => snapshots = true,
                    "limit" => limit = a.value("--limit")?.parse().map_err(|_| usage("--limit needs a number"))?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
            block_on(reporter, &stop, versions(&paths, &cfg, snapshots, limit, reporter))?;
        }
        "instance" => match args.positional("`new` or `list`")?.as_str() {
            "list" => {
                let instances = instance_summaries(&paths)?;
                if machine {
                    reporter.emit(Event::Instances { instances });
                } else {
                    print_instances(&instances);
                }
            }
            "new" => {
                let id = args.positional("instance id")?;
                let mut minecraft = None;
                args.flags(|name, a| {
                    if name == "minecraft" {
                        minecraft = Some(a.value("--minecraft")?);
                        return Ok(true);
                    }
                    Ok(false)
                })?;
                let minecraft = minecraft.ok_or_else(|| usage("--minecraft <version> is required"))?;
                let dir = instance::create(&paths, &id, Some(&minecraft))?;
                reporter
                    .status(format!("created vanilla instance {id} for Minecraft {minecraft} at {}", dir.display()));
                if machine {
                    reporter.emit(Event::Instances { instances: instance_summaries(&paths)? });
                }
            }
            other => return Err(usage(format!("unknown instance subcommand {other:?}"))),
        },
        "install" => {
            let id = args.positional("instance id")?;
            let mut repair = false;
            args.flags(|name, _| {
                if name == "repair" {
                    repair = true;
                    return Ok(true);
                }
                Ok(false)
            })?;
            let cfg = Config::load(&paths)?;
            block_on(reporter, &stop, async {
                let inst = instance::load(&paths, &id)?;
                let http = launch::http_client(&cfg)?;
                let p = launch::prepare(&paths, &cfg, &http, &inst, repair, reporter).await?;
                reporter.status(format!(
                    "Minecraft {} is installed and verified: client jar, {} libraries, {} asset objects.",
                    p.installed.json.id,
                    p.installed.libraries.len() + p.loader_libs.len(),
                    p.installed.asset_objects.len()
                ));
                Ok(())
            })?;
        }
        "launch" => {
            let id = args.positional("instance id")?;
            let mut opts = LaunchOptions {
                instance: id,
                account: registry::default_account(&paths),
                demo: false,
                burner: false,
                join: None,
                repair: false,
                stop: stop.clone(),
                program: "/proc/self/exe".into(),
            };
            let mut quiet = false;
            args.flags(|name, a| {
                match name {
                    "account" => opts.account = a.value("--account")?,
                    "join" => opts.join = Some(a.value("--join")?),
                    "demo" => opts.demo = true,
                    "burner" => opts.burner = true,
                    "repair" => opts.repair = true,
                    "quiet" => quiet = true,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
            validate_id(&opts.account)?;
            let cfg = Config::load(&paths)?;
            // `--quiet` only means something on a terminal; a front end always gets events.
            let reporter = if quiet && !machine { Reporter::human(true) } else { reporter.clone() };
            return block_on(&reporter, &stop, launch::launch(&paths, &cfg, &opts, &reporter));
        }
        "scram" => {
            let mut yes = false;
            args.flags(|name, _| {
                if name == "yes" {
                    yes = true;
                    return Ok(true);
                }
                Ok(false)
            })?;
            if !yes {
                if machine {
                    return Err(usage("in event mode `scram` needs --yes (the front end asks the user)"));
                }
                eprintln!(
                    "This stops every running game started by this launcher and removes all stored sign-ins\n\
                     (refresh tokens in the kernel keyring, age vault files, remembered players) for every account.\n\
                     You will have to sign in again. Nothing is revoked at Microsoft by this.\n\
                     Type SCRAM and press Enter to continue; anything else cancels:"
                );
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer).ctx(|| "reading the confirmation")?;
                if answer.trim() != "SCRAM" {
                    eprintln!("cancelled; nothing was changed");
                    return Ok(1);
                }
            }
            let report = crate::scram::run(&paths)?;
            for line in &report.lines {
                if machine { reporter.status(line) } else { println!("{line}") }
            }
            if machine && let Ok(cfg) = Config::load(&paths) {
                reporter.emit(account_summaries(&paths, &cfg));
            }
            return Ok(if report.incomplete { 1 } else { 0 });
        }
        "wiki" => {
            use bastion_mc_proto::runbook;
            match args.positional("topic (`scram`)")?.as_str() {
                "scram" => {}
                other => return Err(usage(format!("unknown wiki topic {other:?} (there is one: scram)"))),
            }
            let mut links = false;
            args.flags(|name, _| {
                if name == "links" {
                    links = true;
                    return Ok(true);
                }
                Ok(false)
            })?;
            if links {
                for (link, what) in runbook::LINKS {
                    println!("{link}\n    {what}");
                }
            } else {
                print!("{}", runbook::SCRAM);
            }
        }
        "export-proofs" => {
            let out = PathBuf::from(args.positional("output file")?);
            let account = account_flag(args, &paths)?;
            let text = crate::proofs::bundle(&paths, &account, auth::canary::now())?;
            // Never over an existing file, never through a symlink, readable by the user only.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&out)
                .ctx(|| format!("creating {}", out.display()))?;
            file.write_all(text.as_bytes())
                .and_then(|_| file.sync_all())
                .ctx(|| format!("writing {}", out.display()))?;
            println!("written to {} (mode 0600). Read it, add the purchase details, then send it to", out.display());
            println!("Minecraft Support only — the form is linked in `bastion-mc wiki scram`.");
        }
        "skin" | "cape" | "canary" => {
            let sub = args.positional(&format!("{command} subcommand"))?;
            profile_admin(args, &paths, command, &sub, reporter, &stop)?;
        }
        "realms" => {
            match args.positional("realms subcommand (`totp-setup`)")?.as_str() {
                "totp-setup" => {}
                other => return Err(usage(format!("unknown realms subcommand {other:?}"))),
            }
            let mut replace = false;
            args.flags(|name, _| {
                if name == "replace" {
                    replace = true;
                    return Ok(true);
                }
                Ok(false)
            })?;
            let cfg = Config::load(&paths)?;
            let store = TokenStore::open(&cfg.auth, &paths)?;
            // Only ever on the terminal: the command is not offered with --events.
            print!("{}", crate::realms::totp_setup(&store, replace)?);
        }
        "vault" => {
            match args.positional("vault subcommand (`reseal`)")?.as_str() {
                "reseal" => {}
                other => return Err(usage(format!("unknown vault subcommand {other:?}"))),
            }
            let mut only = None;
            args.flags(|name, a| {
                if name == "account" {
                    only = Some(a.value("--account")?);
                    return Ok(true);
                }
                Ok(false)
            })?;
            let cfg = Config::load(&paths)?;
            let store = TokenStore::open(&cfg.auth, &paths)?;
            if !matches!(store, TokenStore::Age(_)) {
                return Err(Error::Config("auth.token_store is not \"age\": there is no vault to re-seal".into()));
            }
            // Re-sealing writes vault files: stoppable by `scram`.
            let _marker = crate::running::Marker::for_command(&paths, "vault")?;
            let accounts = match only {
                Some(account) => {
                    validate_id(&account)?;
                    vec![account]
                }
                None => registry::list(&paths),
            };
            let mut sealed = 0;
            for account in &accounts {
                // One decryption (a touch, with a hardware key) and one encryption each.
                if store.reseal(account)? {
                    println!("account {account}: vault re-encrypted to the current recipients");
                    sealed += 1;
                }
            }
            if sealed == 0 {
                println!("no vault file found for {}", accounts.join(", "));
            }
        }
        "pack" => {
            use crate::profile::packs::{self, PackKind};
            let sub = args.positional("pack subcommand")?;
            let id = args.positional("instance id")?;
            let dir = paths.instance(&id)?;
            if !dir.is_dir() {
                return Err(usage(format!("no instance called {id:?}")));
            }
            let operand = match sub.as_str() {
                "import" => Some(args.positional("pack file")?),
                "remove" => Some(args.positional("pack name")?),
                "list" | "adopt" => None,
                other => return Err(usage(format!("unknown pack subcommand {other:?}"))),
            };
            let (mut kind, mut name) = (PackKind::Resource, None);
            args.flags(|flag, a| {
                match (flag, sub.as_str()) {
                    ("shader", "import" | "remove") => kind = PackKind::Shader,
                    ("name", "import") => name = Some(a.value("--name")?),
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
            match sub.as_str() {
                "import" => {
                    let entry =
                        packs::import(&dir, kind, Path::new(operand.as_deref().expect("file")), name.as_deref())?;
                    println!("added {} {} ({} bytes, sha256 {})", kind.noun(), entry.file, entry.size, entry.sha256);
                    println!("The game sees it read-only from the next launch on; select it in the game's options.");
                }
                "remove" => {
                    packs::remove(&dir, kind, operand.as_deref().expect("name"))?;
                    println!("removed {} {}", kind.noun(), operand.as_deref().expect("name"));
                }
                "adopt" => {
                    let lock = packs::adopt(&dir)?;
                    println!(
                        "recorded {} resource pack(s) and {} shader pack(s) as they are now",
                        lock.resourcepacks.len(),
                        lock.shaderpacks.len()
                    );
                }
                _ => {
                    let lock = packs::Lock::load(&dir)?.unwrap_or_default();
                    for kind in PackKind::ALL {
                        for entry in lock.entries(kind) {
                            println!("{}\t{}\t{} bytes\tsha256 {}", kind.noun(), entry.file, entry.size, entry.sha256);
                        }
                    }
                    if lock.resourcepacks.is_empty() && lock.shaderpacks.is_empty() {
                        println!("no packs (add one with `bastion-mc pack import {id} <file.zip>`)");
                    }
                }
            }
        }
        "key" => key_command(args, &paths)?,
        "profile" => {
            let sub = args.positional("profile subcommand")?;
            let id = args.positional("instance id")?;
            profile_command(args, &paths, &sub, &id)?;
        }
        other => return Err(usage(format!("unknown command {other:?}"))),
    }
    Ok(0)
}

async fn login(paths: &Paths, cfg: &Config, flow: AuthFlow, account: &str, reporter: &Reporter) -> Result<()> {
    let client_id = cfg.auth.client_id.as_deref().ok_or_else(|| {
        Error::Config(format!(
            "auth.client_id is not set in {}. This launcher has no built-in client ID; register your own Azure \
             application (README, \"Your own OAuth application\").",
            paths.config_file().display()
        ))
    })?;
    let store = TokenStore::open(&cfg.auth, paths)?;
    let http = launch::http_client(cfg)?;
    let tokens = msa::interactive_login(&http, client_id, flow, reporter).await?;
    let refresh = tokens.refresh_token.ok_or_else(|| {
        Error::Auth("Microsoft returned no refresh token; the app must be allowed the `offline_access` scope".into())
    })?;
    // Walk the whole chain now, so problems (Xbox profile, Mojang app approval, ownership)
    // surface at login rather than at the first launch.
    let session = auth::session_from_msa(&http, tokens.access_token, cfg.auth.allow_game_pass).await?;
    store.save(account, &refresh)?;
    let _ = auth::activity::record(paths, account, "sign-in", "");
    // A sign-in pins the profile the first time. A profile that differs from an existing
    // pin is reported, not accepted: signing in again is exactly what a worried user does.
    match launch::canary_verdict(paths, cfg, account, &session.profile)? {
        Ok(note) if note.notable => reporter.warn(&note.text),
        Ok(note) => reporter.status(&note.text),
        Err(refusal) => reporter.warn(format!("WARNING: {refusal}")),
    }
    // Display-only; see `auth::identity`.
    let _ = identity::remember(
        paths,
        account,
        &identity::Identity { player: session.profile_name.clone(), uuid: session.profile_id.clone() },
    );
    reporter.status(format!("signed in as {} — refresh token stored in: {}", session.profile_name, store.describe()));
    Ok(())
}

/// Refresh token → session, with the rotated refresh token stored again. The session's
/// Minecraft token lives until the caller drops it.
async fn open_session(
    paths: &Paths,
    cfg: &Config,
    http: &crate::net::http::HttpClient,
    account: &str,
) -> Result<auth::Session> {
    let client_id = cfg
        .auth
        .client_id
        .as_deref()
        .ok_or_else(|| Error::Config("auth.client_id is not set; sign in first (`bastion-mc login`)".into()))?;
    let store = TokenStore::open(&cfg.auth, paths)?;
    let refresh = store
        .load(account)?
        .ok_or_else(|| Error::Auth(format!("not signed in (account {account:?}). Run `bastion-mc login` first.")))?;
    let (session, next) = auth::authenticate(http, client_id, &refresh, cfg.auth.allow_game_pass).await?;
    drop(refresh);
    store.save(account, &next)?;
    Ok(session)
}

fn date(unix: u64) -> String {
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

fn print_profile(profile: &auth::minecraft::Profile) {
    println!("player: {} ({})", profile.name, profile.id);
    match profile.active_skin() {
        Some(skin) => println!("skin:   {} model, texture {}", skin.variant.name(), skin.texture),
        None => println!("skin:   default"),
    }
    if profile.capes.is_empty() {
        println!("capes:  none");
    }
    for cape in &profile.capes {
        println!("cape:   {}{}", cape.alias, if cape.active { "  (worn)" } else { "" });
    }
}

fn print_snapshot(label: &str, s: &auth::canary::Snapshot) {
    let skin = match &s.skin {
        Some((texture, variant)) => format!("{variant} {texture}"),
        None => "default".into(),
    };
    let cape = s.cape.as_ref().map(|c| c.1.as_str()).unwrap_or("none");
    println!(
        "{label} {}  name {}  skin {skin}  cape {cape}  owned capes: {}",
        date(s.recorded),
        s.name,
        if s.capes_owned.is_empty() { "none".to_string() } else { s.capes_owned.join(", ") }
    );
}

/// `skin`, `cape` and `canary`: everything that reads or changes the account's profile.
fn profile_admin(
    args: &mut Args,
    paths: &Paths,
    command: &str,
    sub: &str,
    reporter: &Reporter,
    stop: &StopSignal,
) -> Result<()> {
    use auth::{canary, minecraft};
    // One positional where the subcommand takes one, then flags.
    let operand = match (command, sub) {
        ("skin", "set" | "export") => Some(args.positional("file")?),
        ("cape", "set") => Some(args.positional("cape alias or `none`")?),
        ("skin", "show" | "reset") | ("canary", "show" | "accept") => None,
        _ => return Err(usage(format!("unknown {command} subcommand {sub:?}"))),
    };
    let (mut account, mut slim) = (registry::default_account(paths), false);
    args.flags(|name, a| {
        match name {
            "account" => account = a.value("--account")?,
            "slim" if (command, sub) == ("skin", "set") => slim = true,
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    validate_id(&account)?;
    let cfg = Config::load(paths)?;

    if (command, sub) == ("canary", "show") {
        match canary::load(paths, &account)? {
            None => println!("nothing pinned for account {account} yet (it is pinned at the first sign-in or launch)"),
            Some(record) => {
                println!("account {account}, player id {}", record.current.uuid);
                for old in &record.history {
                    print_snapshot("  was   ", old);
                }
                print_snapshot("  pinned", &record.current);
            }
        }
        return Ok(());
    }

    // Read and checked before anything is asked of the network.
    let upload = match (command, sub, &operand) {
        ("skin", "set", Some(file)) => {
            let raw = util::read_small_file(Path::new(file), 4 * minecraft::MAX_SKIN_BYTES as u64)?;
            // Decoded and encoded again: what is uploaded is the image's pixels in a file of
            // the launcher's own making, whatever else the original held.
            let clean = minecraft::sanitize_skin_png(&raw)?;
            reporter.status(format!(
                "skin: the image was decoded and written out again; only its pixels are uploaded ({} → {} bytes)",
                raw.len(),
                clean.len()
            ));
            Some(clean)
        }
        _ => None,
    };

    // Opening a session stores the rotated refresh token: stoppable by `scram`.
    let _marker = crate::running::Marker::for_command(paths, command)?;
    block_on(reporter, stop, async {
        let http = launch::http_client(&cfg)?;
        let session = open_session(paths, &cfg, &http, &account).await?;
        let token = &session.access_token;
        if (command, sub) == ("canary", "accept") {
            match canary::check(paths, &account, &session.profile) {
                Ok(canary::Check::Changed(differences)) => {
                    println!("accepting these changes for account {account}:");
                    for d in &differences {
                        println!("  - {d}");
                    }
                }
                Ok(canary::Check::Unchanged) => println!("the profile already matches what is pinned"),
                Ok(canary::Check::First) => println!("pinning account {account} for the first time"),
                Err(_) => println!("replacing a damaged canary record for account {account}"),
            }
            canary::accept(paths, &account, &session.profile)?;
            print_profile(&session.profile);
            return Ok(());
        }
        // A change made elsewhere must be looked at before more changes are stacked on it:
        // an enforcing canary stops here. One that only warns says so — every time, in every
        // command — and what it has to say is never dropped.
        let note = launch::canary_verdict(paths, &cfg, &account, &session.profile)??;
        if note.notable {
            reporter.warn(&note.text)
        } else {
            reporter.status(&note.text)
        }
        match (command, sub) {
            ("skin", "show") => {
                print_profile(&session.profile);
                return Ok(());
            }
            ("skin", "export") => {
                let skin = session.profile.active_skin().ok_or_else(|| {
                    Error::Usage("this account uses the default skin; there is nothing to export".into())
                })?;
                let bytes =
                    http.get(&minecraft::texture_url(&skin.texture), 4 * minecraft::MAX_SKIN_BYTES as u64).await?;
                // The same check as for uploads: only an image is written to disk.
                let png = minecraft::sanitize_skin_png(&bytes)?;
                let out = PathBuf::from(operand.as_deref().expect("file operand"));
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&out)
                    .ctx(|| format!("creating {}", out.display()))?;
                file.write_all(&png).ctx(|| format!("writing {}", out.display()))?;
                println!("active skin ({} model) written to {}", skin.variant.name(), out.display());
                return Ok(());
            }
            ("skin", "set") => {
                let variant = if slim { minecraft::Variant::Slim } else { minecraft::Variant::Classic };
                minecraft::upload_skin(&http, token, upload.as_deref().expect("checked skin"), variant).await?;
            }
            ("skin", "reset") => minecraft::reset_skin(&http, token).await?,
            ("cape", "set") => {
                let wanted = operand.as_deref().expect("cape operand");
                let id = if wanted.eq_ignore_ascii_case("none") {
                    None
                } else {
                    let cape = session
                        .profile
                        .capes
                        .iter()
                        .find(|c| c.alias.eq_ignore_ascii_case(wanted) || c.id == wanted)
                        .ok_or_else(|| {
                            let owned: Vec<&str> = session.profile.capes.iter().map(|c| c.alias.as_str()).collect();
                            Error::Usage(format!(
                                "this account has no cape called {wanted:?} (it has: {})",
                                if owned.is_empty() { "none".to_string() } else { owned.join(", ") }
                            ))
                        })?;
                    Some(cape.id.clone())
                };
                minecraft::set_active_cape(&http, token, id.as_deref()).await?;
            }
            _ => unreachable!("subcommands were checked above"),
        }
        // What Mojang now says the profile is — not what was asked for — is shown. Pinned is
        // the part this command changed, and only that: the rest of the profile was not
        // changed here, and if it differs from the pin (somebody else's doing, a moment ago
        // or before a warning that was not acted on), it stays a difference.
        let after = minecraft::profile(&http, token).await?;
        if cfg.auth.canary != crate::config::CanaryMode::Off {
            let (aspect, what) =
                if command == "cape" { (canary::Aspect::Cape, "cape") } else { (canary::Aspect::Skin, "skin") };
            match canary::accept_change(paths, &account, &after, aspect) {
                Ok(unpinned) if unpinned.is_empty() => {}
                Ok(unpinned) => reporter.warn(format!(
                    "canary: WARNING: the new {what} is pinned, but the profile also differs from what is pinned in \
                     ways this command did not make: {}. That is NOT pinned. If it was you, run `bastion-mc canary \
                     accept --account {account}`; if not, `bastion-mc wiki scram`.",
                    unpinned.join("; ")
                )),
                Err(e) => reporter.warn(format!("canary: WARNING: the new {what} was not pinned: {e}")),
            }
        }
        print_profile(&after);
        Ok(())
    })
}

async fn versions(paths: &Paths, cfg: &Config, snapshots: bool, limit: usize, reporter: &Reporter) -> Result<()> {
    let http = launch::http_client(cfg)?;
    let silent = Reporter::human(true);
    let store = Store { paths, http: &http, parallel: 1, repair: false, reporter: &silent };
    let manifest = store.fetch_manifest().await?;
    let wanted = if snapshots { "snapshot" } else { "release" };
    // Ids come back as `--minecraft <id>`: list only what that would accept.
    let listed =
        manifest.versions.iter().filter(|v| v.kind == wanted && validate_version_id(&v.id).is_ok()).take(limit);
    if reporter.is_events() {
        fn date(time: &str) -> Option<&str> {
            time.get(..10).filter(|d| d.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
        }
        reporter.emit(Event::Versions {
            latest_release: auth::clean(&manifest.latest.release),
            latest_snapshot: auth::clean(&manifest.latest.snapshot),
            versions: listed
                .map(|v| events::Version {
                    id: v.id.clone(),
                    kind: v.kind.clone(),
                    released: date(&v.release_time).unwrap_or_default().to_string(),
                })
                .collect(),
        });
        return Ok(());
    }
    println!("latest release: {}    latest snapshot: {}", manifest.latest.release, manifest.latest.snapshot);
    for v in listed {
        println!("{:<24} {}", v.id, v.release_time.get(..10).unwrap_or(""));
    }
    Ok(())
}

fn key_command(args: &mut Args, paths: &Paths) -> Result<()> {
    match args.positional("key subcommand")?.as_str() {
        "generate" => {
            let out = PathBuf::from(args.positional("output file")?);
            let key = SecretKey::generate()?;
            let encoded = key.encode()?;
            // O_EXCL: never overwrite an existing key, never follow a planted symlink.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&out)
                .ctx(|| format!("creating {}", out.display()))?;
            file.write_all(encoded.expose())
                .and_then(|_| file.sync_all())
                .ctx(|| format!("writing {}", out.display()))?;
            println!("signing key written to {} (mode 0600). Keep it off this machine if you can.", out.display());
            println!("public key: {}", key.public().encode());
            println!("trust it with:  bastion-mc key trust <name> {}", key.public().encode());
        }
        "public" => {
            let key = SecretKey::load(Path::new(&args.positional("key file")?))?;
            println!("{}", key.public().encode());
        }
        "trust" => {
            let name = args.positional("key name")?;
            validate_id(&name)?;
            let given = args.positional("public key or file")?;
            let text = if given.starts_with("ed25519:") {
                given
            } else {
                String::from_utf8(util::read_small_file(Path::new(&given), 4096)?)
                    .map_err(|_| Error::Format("public key file is not UTF-8".into()))?
            };
            let key = PublicKey::parse(&text)?;
            let dir = paths.user_trusted_keys();
            util::create_private_dir_all(&dir)?;
            util::write_atomic(&dir.join(format!("{name}.pub")), format!("{}\n", key.encode()).as_bytes(), 0o600)?;
            println!("trusted {name} [{}] in {}", key.id(), dir.display());
            println!(
                "note: keys in {} (root-owned) take precedence; when any exist there, per-user keys are ignored.",
                crate::paths::SYSTEM_TRUSTED_KEYS
            );
        }
        "list" => {
            let store = TrustStore::load(paths)?;
            println!("authoritative store: {:?} ({})", store.source, store.dir.display());
            for (name, key) in &store.keys {
                println!("{name}\t[{}]\t{}", key.id(), key.encode());
            }
            if store.keys.is_empty() {
                println!("(no trusted keys)");
            }
        }
        other => return Err(usage(format!("unknown key subcommand {other:?}"))),
    }
    Ok(())
}

fn profile_command(args: &mut Args, paths: &Paths, sub: &str, id: &str) -> Result<()> {
    validate_id(id)?;
    match sub {
        "add" | "check" => return crate::profile::catalog::command(&mut args.parser, paths, sub, id),
        "new" => {
            let (mut minecraft, mut kind, mut loader_version) = (None, None, None);
            args.flags(|name, a| {
                match name {
                    "minecraft" => minecraft = Some(a.value("--minecraft")?),
                    "fabric" => kind = Some(LoaderKind::Fabric),
                    "quilt" => kind = Some(LoaderKind::Quilt),
                    "loader-version" => loader_version = Some(a.value("--loader-version")?),
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
            let minecraft = minecraft.ok_or_else(|| usage("--minecraft <version> is required"))?;
            let kind = kind.ok_or_else(|| usage("choose a loader: --fabric or --quilt"))?;
            let cfg = Config::load(paths)?;
            let dir = runtime()?.block_on(async {
                let http = launch::http_client(&cfg)?;
                author::new_profile(paths, &http, id, &minecraft, kind, loader_version.as_deref()).await
            })?;
            println!("draft profile created at {}", dir.display());
            println!(
                "next: put mod jars into {0}/mods, then\n  bastion-mc profile lock {id}\n  (review {0}/profile.json)\n  bastion-mc profile sign {id} --key <your key>",
                dir.display()
            );
        }
        "lock" => {
            args.flags(|_, _| Ok(false))?;
            let manifest = author::lock(paths, id)?;
            println!("profile {id} now at serial {} with {} mod(s):", manifest.serial, manifest.mods.len());
            for m in &manifest.mods {
                println!("  {}  {:>10} bytes  {}", m.sha256, m.size, m.file);
            }
            // What the jars can reach, compared with the previous lock. Informational: a
            // jar that cannot be scanned still locks, and says so.
            let dir = paths.instance(id)?;
            match capabilities::scan_mods(&dir.join("mods")) {
                Ok(scanned) => {
                    let before = capabilities::Recorded::load(&dir);
                    let changes = capabilities::changes(&before, &scanned);
                    if !changes.is_empty() {
                        println!(
                            "\nSince the last lock (static scan of the class files; a signal for review, not proof):"
                        );
                        for line in &changes {
                            println!("  {line}");
                        }
                        println!("`bastion-mc profile audit {id}` shows the full picture.");
                    }
                    capabilities::Recorded::of(&scanned).save(&dir)?;
                }
                Err(e) => println!("\nnote: the mods could not be scanned for API use: {e}"),
            }
            println!("The previous signature (if any) was removed. Review the manifest, then sign it.");
        }
        "audit" => {
            args.flags(|_, _| Ok(false))?;
            let dir = paths.instance(id)?;
            let scanned = capabilities::scan_mods(&dir.join("mods"))?;
            if scanned.is_empty() {
                println!("{id}: no mod jars");
            }
            for (identity, (file, report)) in &scanned {
                println!("{identity}  ({file}: {} classes, {} nested jar(s))", report.classes, report.nested.len());
                if report.capabilities.is_empty() && report.natives.is_empty() {
                    println!("    nothing on the watch list");
                }
                for (capability, classes) in &report.capabilities {
                    let total = report.counts.get(capability).copied().unwrap_or(classes.len());
                    let meaning = capabilities::describe(capability.trim_end_matches(" (named in a string)"));
                    println!("    {capability} — {meaning}");
                    println!(
                        "        {}{}",
                        classes.join(", "),
                        if total > classes.len() { format!(", … {total} classes in all") } else { String::new() }
                    );
                }
                for native in &report.natives {
                    println!("    bundled native library: {native}");
                }
                for problem in &report.unreadable {
                    println!("    could not be read: {problem}");
                }
            }
            println!(
                "\nThis is a static scan of what the class files refer to by name. Reflection, classes defined at\n\
                 run time and native code can hide anything else — that is why those are listed. It informs the\n\
                 decision to sign; it enforces nothing (the sandbox does)."
            );
        }
        "sign" => {
            let mut key = None;
            args.flags(|name, a| {
                if name == "key" {
                    key = Some(a.value("--key")?);
                    return Ok(true);
                }
                Ok(false)
            })?;
            let key = SecretKey::load(Path::new(&key.ok_or_else(|| usage("--key <file> is required"))?))?;
            let (manifest, key_id) = author::sign(paths, id, &key)?;
            println!(
                "signed profile {id} serial {} ({} mods) with key [{key_id}]",
                manifest.serial,
                manifest.mods.len()
            );
        }
        "verify" => {
            args.flags(|_, _| Ok(false))?;
            let inst = instance::load(paths, id)?;
            match &inst.kind {
                Kind::Vanilla => {
                    mods::verify_mods_dir(&inst.dir.join("mods"), &[])?;
                    println!(
                        "{id}: vanilla instance (Minecraft {}); no profile to verify, mods/ is empty",
                        inst.minecraft
                    );
                }
                Kind::Curated { manifest, signed_by } => {
                    let verified = mods::verify_mods_dir(&inst.dir.join("mods"), &manifest.mods)?;
                    println!(
                        "{id}: signature by {signed_by:?} is valid; serial {}; Minecraft {}; {} mod jar(s) match exactly",
                        manifest.serial,
                        inst.minecraft,
                        verified.len()
                    );
                }
            }
        }
        other => return Err(usage(format!("unknown profile subcommand {other:?}"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::date;

    #[test]
    fn dates_are_civil_dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_794_528_000), "2026-11-13");
        assert_eq!(date(1_798_761_599), "2026-12-31");
    }
}
