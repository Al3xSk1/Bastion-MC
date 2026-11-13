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
    auth::{self, identity, msa, store::TokenStore},
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
        author,
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

  versions [--snapshots] [--limit N]  list Minecraft versions from Mojang's manifest
  instance new <id> --minecraft <version>     create a vanilla instance
  instance list
  install <id> [--repair]             download and verify an instance's files without launching
  launch  <id> [--account NAME] [--join HOST[:PORT]] [--demo] [--repair] [--quiet]

  key generate <file>                 create an Ed25519 signing key (file mode 0600)
  key public <file>                   print the public half of a signing key
  key trust <name> <public-key|file>  trust a key for signing profiles (per-user store)
  key list                            show trusted keys and which store is authoritative

  profile new <id> --minecraft <version> (--fabric | --quilt) [--loader-version V]
  profile lock <id>                   record the current mods/ directory in the manifest
  profile sign <id> --key <file>      sign the manifest
  profile verify <id>                 verify signature and mods/ exactly as a launch would

  --help, --version
  --events    (first argument) report progress as JSON lines on standard output, for a front
              end such as bastion-mc-gui: status, login, logout, versions, instance, install, launch

--demo starts the client in Mojang's own demo mode with no account: useful for testing the
sandbox before your Azure application is approved. No token exists in that mode.";

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

fn account_flag(args: &mut Args) -> Result<String> {
    let mut account = "default".to_string();
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
    if machine && !matches!(command, "status" | "login" | "logout" | "versions" | "instance" | "install" | "launch") {
        return Err(usage(format!("--events is not available for `{command}`")));
    }
    let stop = StopSignal::never();
    let paths = Paths::from_env()?;
    paths.ensure()?;

    match command {
        "status" => {
            let account = account_flag(args)?;
            let cfg = Config::load(&paths)?;
            let summary = account_summary(&paths, &cfg, &account);
            let instances = instance_summaries(&paths)?;
            if machine {
                reporter.emit(Event::Account(summary));
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
            let (mut flow, mut account) = (cfg.auth.flow, "default".to_string());
            args.flags(|name, a| {
                match name {
                    "device" => flow = AuthFlow::Device,
                    "account" => account = a.value("--account")?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
            validate_id(&account)?;
            block_on(reporter, &stop, login(&paths, &cfg, flow, &account, reporter))?;
            reporter.emit(Event::Account(account_summary(&paths, &cfg, &account)));
        }
        "logout" => {
            let account = account_flag(args)?;
            let cfg = Config::load(&paths)?;
            TokenStore::open(&cfg.auth, &paths)?.delete(&account)?;
            identity::forget(&paths, &account)?;
            reporter.status(format!("signed out ({account})"));
            reporter.emit(Event::Account(account_summary(&paths, &cfg, &account)));
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
                account: "default".into(),
                demo: false,
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
    // Display-only; see `auth::identity`.
    let _ = identity::remember(
        paths,
        account,
        &identity::Identity { player: session.profile_name.clone(), uuid: session.profile_id.clone() },
    );
    reporter.status(format!("signed in as {} — refresh token stored in: {}", session.profile_name, store.describe()));
    Ok(())
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
            println!("The previous signature (if any) was removed. Review the manifest, then sign it.");
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
