# Bedrock: the installed client and the version store (store package, round 5)

Code: `src/bedrock/runtime.rs` (+ `runtime/tests.rs`), `src/bedrock/store.rs` with
`store/manifest.rs`, `store/axml.rs`, `store/elf.rs` (+ `store/tests.rs`), the `bedrock` block in
`src/cli.rs`, the `bedrock_store` harness (`src/fuzzing.rs`, `tests/robustness.rs`) and
`tests/bedrock_store.rs`. Hand-over state: `bedrock-store-progress.md`.

**Honest limits, first.** There is no `mcpelauncher-client`, no Minecraft download, no Google
Play purchase and no Microsoft account on the machine this was written on, and none may be
installed. Everything below was verified against stand-ins written for the purpose: fake
install trees (owned by the test's user, under a test root), a synthetic compiled
`AndroidManifest.xml` (both string encodings), ELF headers instead of libraries, and
`/usr/bin/true` as a root-owned program from a package. Section 3 lists exactly what ran.

---

## 1. User documentation (for the README and docs/BEDROCK.md)

### The client bastion runs

bastion runs the `mcpelauncher-client` a **system package** installed — on Arch the AUR package
`mcpelauncher-linux`, which pacman installs as root at `/usr/bin/mcpelauncher-client`. bastion
never downloads or builds a client, and never runs it outside its sandbox (not even to ask its
version: that comes from pacman's database).

```sh
bastion-mc bedrock client      # which client, its version, its data directory, and
                               # which installed sign-in windows the sandbox covers
```

bastion looks at `/usr/bin/mcpelauncher-client`, then `/usr/local/bin/mcpelauncher-client`,
unless `[bedrock] client = "/path/to/mcpelauncher-client"` names one. Whichever it is, it must
pass the same rule as the Java runtime: every directory from `/` down, every symbolic link on
the way and the program itself owned by root and writable by nobody else, not on a FUSE file
system, an x86-64 program (not a script, not set-user-ID). These are refused, each with its
reason in the message:

| Refused | Why |
|---|---|
| anything under your home directory | you, and every program running as you, can change it |
| Flatpak installs (`io.mrarm.mcpelauncher`), system or per-user, and any Flatpak deployment found by its layout | the client runs on the Flatpak runtime's libraries, from files outside the system's package manager (a per-user install even under your home), and Flatpak's own sandbox needs new namespaces, which bastion's sandbox forbids |
| AppImages, their mounts (`/tmp/.mount_*`) and FUSE file systems | an AppImage mounts itself through FUSE, served by a program of yours, and brings its own libraries; a FUSE file system's contents and claimed owners are whatever the serving program says |
| a client in a directory others can write (`/tmp`, a group-writable directory…) | whoever can write it can swap the client |

The client also reads its data directory (`/usr/share/mcpelauncher`: a prebuilt Android-side
`libsqliteX.so` for each ABI and `gamecontrollerdb.txt`); it is held to the same rule, file by
file, and may hold no symbolic links.

### Importing a Bedrock version

Download the version with `mcpelauncher-ui` (it checks with Google that your account owns
Minecraft on Google Play), choosing the **x86_64** build. Then:

```sh
bastion-mc bedrock versions            # what is imported, and what mcpelauncher-ui has
bastion-mc bedrock import ~/.local/share/mcpelauncher/versions/1.26.62.1
bastion-mc bedrock versions --verify   # re-check every file (each launch does this too)
bastion-mc bedrock remove 1.26.62.1
```

mcpelauncher-ui keeps versions in `~/.local/share/mcpelauncher/versions/<version>/` (in
`$XDG_DATA_HOME/mcpelauncher/versions/` if you set `XDG_DATA_HOME`), and the Flatpak in
`~/.var/app/io.mrarm.mcpelauncher/data/mcpelauncher/versions/<version>/`; `bedrock versions`
lists both. The import:

* copies only what the client reads: `AndroidManifest.xml`, `lib/x86_64/` and `assets/`. Other
  ABIs (`lib/arm64-v8a/`…) and anything else are left out, and it says so. **No APK file is
  needed at run time and none is ever imported**: give it the directory mcpelauncher-ui
  extracted, not an `.apk`;
* takes x86_64 builds only: `lib/x86_64/libminecraftpe.so` must exist, and every file in
  `lib/x86_64/` must be an x86-64 shared library named `lib*.so`. A build for another machine is
  refused with the ABIs it has;
* checks `AndroidManifest.xml` is the base APK's manifest of `com.mojang.minecraftpe` (not a
  split's, not another app's) and takes the version name and code from it;
* refuses symbolic links, hard links, FIFOs, sockets and devices in the copied part (a link is
  how a private file of yours would end up readable inside the game's sandbox), names with
  control characters or that are not UTF-8, and trees beyond its limits: **8 GiB in all, 2 GiB
  for one file, 200,000 files and directories, 24 levels below `assets/`, 64 libraries**
  (generous guesses: no real version could be measured here; a refusal names the limit);
* stops if a file changes while it is copied (do not run it while mcpelauncher-ui is still
  writing the version);
* names the version by its version name (`1.26.62.1`), or the directory's name, or `--id`; ids
  are validated like instance ids (`[a-z0-9._-]`, 1–64 characters) and an imported version is
  never replaced — remove it first;
* writes a SHA-256 manifest of every file **last**, and leaves the copy read-only (files `0444`,
  directories `0555`) in `$XDG_DATA_HOME/bastion-mc/bedrock/versions/<id>/` (`manifest` and
  `game/`).

Every launch checks every file of the version against its manifest before the sandbox gets it
(on this machine the hash alone runs at about 2 GiB/s; reading files that are not cached comes
on top). Anything changed, added, removed, linked or hard-linked stops the launch with
"INTEGRITY VIOLATION" and the file's name; import the version again. `bedrock remove` is
refused while a launch uses the version.

---

## 2. Threat model: rows and changes this package implies

New rows (table "Bedrock", for the integrator to number):

| # | Threat | Defence | Residual risk |
|---|---|---|---|
| B-S1 | A program running as the user puts its own program where bastion looks for the client, or points `[bedrock] client` at one | the client must be root-owned with every directory and every link hop on its way root-owned and not group/world-writable, not under the home, not on FUSE; refused otherwise, with the reason | `[bedrock] client` in config.toml is the user's own file: such a program can point it at another *root-owned* program (one any package installed). It cannot make bastion run a program it wrote |
| B-S2 | A Flatpak or AppImage client (files and libraries outside the package manager, a sandbox that cannot nest) | refused by location, by Flatpak deployment layout, by AppImage magic and mount name, by FUSE file system type | a Flatpak installation at a path that is neither configured in `/etc/flatpak/installations.d` nor laid out as a deployment is not recognised as one (it must still pass the ownership rule) |
| B-S3 | The client finds a real `mcpelauncher-webview` (a browser engine) inside the sandbox | `Client::webview_paths` lists every place: beside the client (the compiled-in `.` of upstream AppImage builds), the sandbox's `PATH` (`/usr/bin`), every `mcpelauncher-webview` in pacman's database, and the client's own tree for a manual install; the launch covers each that exists with the stub (`Client::installed_webviews`) | a client compiled with an unusual `XAL_WEBVIEW_QT_PATH` and a webview installed by hand (not by a package) outside the client's tree under `/usr` would be missed; the compiled-in value cannot be read back reliably |
| B-S4 | A link or hard link in the download directory pulls a private file into the store, hence into the game's view | the import refuses links, hard links and special files in what it copies, resolves every name relative to an open directory with `O_NOFOLLOW`, and checks device/inode/size/times between looking and copying | — |
| B-S5 | Tampering with an imported version between sessions (a program running as the user, a crashed copy, a disk error) | per-file SHA-256 manifest written last; every launch checks every file, refuses anything unlisted, links, hard links; the descriptor that was checked is the one mounted | anything able to write the store can write a matching manifest too: the check makes the files *checkable*, not authentic. The check is at launch: a program running as the user can change files after it (as it can change anything of the user's). The files are what the user's download produced — nothing proves they are Mojang's (no APK signature can be checked once the UI has extracted them) |
| B-S6 | A hostile directory meant to exhaust disk, inodes or time | bounded entries, bytes (checked against each file's size before copying, so a sparse file is refused at once), depth, libraries, manifest size; free space checked first; staging copies removed on failure | the bounds are guesses (no real version measured) |
| B-S7 | Removing a version a running session uses | a shared `flock` on the version's directory for as long as the launch holds its descriptor; `remove` takes it exclusively; also refused while a marked launch of a Bedrock instance names the version | a launch that drops its `OpenedVersion` early, *and* lost its marker, can have its version removed under it (the game then fails; nothing else) |
| B-S8 | Parsers of imported bytes (compiled XML, the store's manifest, ELF headers, pacman records) | strict, bounded, fuzzed (`bedrock_store` harness; 2,000,000 generated inputs run here) | — |

Changes to existing rows: the "JVM is root-owned" row becomes "JVM and Bedrock client are
root-owned"; the client's walk is stricter than `java.rs` (every directory and link hop is
checked, plus FUSE), which the JVM row could adopt later.

**Found while reading the client, for the launch package's rows (not this package's code):**
with `-dd /bastion/data` the client's `PathHelper::findDataFile` looks in the data directory
**first**: `lib/x86_64/libsqliteX.so` (an Android library its linker maps),
`lib/native/x86_64/libfmod.so.12.0` (a native library it loads with the host's `dlopen`) and
`gamecontrollerdb.txt`; and its mod loader reads `/bastion/data/mods/` (unless `--free-only`).
The data directory is writable by the game and persists, so a session could plant code the
next session runs — against the contract's "nothing a session wrote is ever executed". Landlock's
execute right does not govern `mmap(PROT_EXEC)` and bubblewrap has no `noexec` bind, so the
remedy is to make those paths impossible: empty read-only directories mounted at
`/bastion/data/lib` and `/bastion/data/mods`. (Sources: mcpelauncher-common `path_helper.cpp`,
mcpelauncher-core `minecraft_utils.cpp`, client `main.cpp`.)

---

## 3. What was verified, how, and what was not

Ran here (commands in the progress file):

* **Client discovery** (`runtime/tests.rs`, 14 tests): on trees built under a test root with
  the test's user as the trusted owner (the policy that allows that can only be built inside the
  module; production always uses root and `/`): a packaged layout is found with its data
  directory, its version from a fake pacman record, and every webview place (including one only
  the package database names); group/world-writable directories at three levels, a writable
  client, set-user-ID/set-group-ID, not executable; relative and absolute links resolved to the
  real path; a link into a world-writable directory, a loop, a link out of the system; a client
  under the home (direct and through a link); system, per-user, configured and layout-detected
  Flatpak installs; AppImage magic, an AppImage mount path, a script, an ARM program, a non-ELF
  file; the FUSE decision (by file system type; no FUSE file system was mounted); a data directory
  with a link or a group-writable file; a manual install searched for webviews in its tree.
  Against the real system with the production policy: `/usr/bin/true` (root's, from coreutils)
  passes and gets "… (coreutils)" as its version from the real pacman database; a copy of it in
  `/tmp` is refused at `/tmp`; a copy under the home is refused for the home; with nothing
  installed, the message names `mcpelauncher-linux` and `describe` lists the two places searched.
* **Import, open, remove** (`store/tests.rs`, 19 tests): a full import (copied bytes, modes
  `0444`/`0555`, manifest lines, verified open returning the very directory, `verify`); only the
  three parts copied, with notes for other ABIs, an `.apk`, stray files and links outside them;
  refusals naming symlinks (file, directory, the library, `assets` itself, the manifest), a hard
  link to a file outside, a FIFO, a socket; no x86_64 libraries (with the ABIs it has), no game
  library, libraries for ARM64/x86/an executable/a script, foreign names, a directory in
  `lib/x86_64`; an APK file, a symlinked source, a missing source, the UI's versions directory;
  another package, a split's manifest, a non-binary manifest; limits on entries, bytes, depth,
  libraries, a 3 GiB sparse file (refused at once, nothing written), control characters; ids
  validated, an existing version never replaced; tampering found by `open` (changed byte, grown
  file, added library, removed file, replaced by a link, hard-linked from outside, a directory for
  a file) and damaged/missing/linked manifests and a missing game directory; `remove` refused
  while `open`'s descriptor *or a duplicate of it* lives, then done, with a note for an instance
  that names the version; `remove` refused while a marked launch (a real process with a marker as
  `running.rs` writes them) of a Bedrock instance names the version, not for another instance,
  and with a reason when the instance cannot be read; a file and a directory changed between scan
  and copy; leftovers swept only when old enough and not locked; the listing; mcpelauncher-ui's
  native and Flatpak places (and `$XDG_DATA_HOME`); `instance_version`; the fuzz seeds.
  Unit tests in `store/axml.rs` (both pool encodings, stripped attribute names, splits, hostile
  lengths/counts/alignment, every prefix, unterminated strings), `store/manifest.rs` (round trip,
  18 malformed variants, the layout rule) and `store/elf.rs`.
* **CLI end to end** (`tests/bedrock_store.rs`, 3 tests): the launcher binary with its XDG
  directories and `HOME` under the build's scratch directory: listing, the UI's version offered,
  import, `versions --verify`, a second import refused (exit 5) and accepted under `--id`, a
  tampered file failing `--verify` with exit 4 and the file named, remove, a missing version
  (exit 2); an APK and an ARM build refused (exit 5) with the reason; `--events bedrock` refused;
  `bedrock client` with nothing installed (exit 3, where it looked), with `/usr/bin/true`
  configured (its coreutils version) and with a program of the user's (exit 5).
* **Fuzzing**: `bedrock_store` harness in the stable robustness suite (4,000 inputs per run by
  default), and here 3 × 150,000 and 1 × 2,000,000 generated inputs (seeds 1, 2, 3, 7): no
  failure.
* **SHA-256 speed** (for the claim in `store.rs`): the `sha2` crate at the locked version, a
  release build, 1 GiB of data in memory: 2180 MiB/s on this machine (it has SHA extensions);
  77 MiB/s in a debug build.

Not verified (and why):

* Anything with the real `mcpelauncher-client`, a real version downloaded by mcpelauncher-ui, a
  real Minecraft `AndroidManifest.xml`, real Flatpak or AppImage installs, a real mounted FUSE
  file system, device files in a download (a user cannot create them), and a real
  `mcpelauncher-linux` record in pacman's database — none exists here, and none may be installed.
  The limits' adequacy for real versions and the time a launch spends verifying a real
  (about-a-GiB) version are therefore unmeasured.
* Whether bubblewrap closes the descriptor given to `--ro-bind-fd` once it has mounted it was not
  checked. It does not decide whether the game gets it: the sandbox's init stage closes every
  descriptor above 2 before it executes the program (`sandbox/init.rs`, `close_range(3, ~0)`;
  read in the code, not exercised by a test of this package).

---

## 4. Integration (signatures the other packages use)

* `bedrock::runtime::discover(&BedrockConfig) -> Result<Client>`; `Client::installed_webviews(&self)
  -> Vec<PathBuf>` (the webview paths that exist, links resolved: what to cover with the stub).
* For `doctor` (risk package): `bedrock::runtime::describe(&BedrockConfig) -> Description` with
  `client: Result<Client>`, `searched: Vec<PathBuf>`, `notes: Vec<String>` (installed webviews the
  launch covers; Flatpak installs not used and why); `bedrock::store::list(&Paths) ->
  Result<Listing>` (`versions: Vec<Summary>`, `damaged: Vec<(String, String)>`, `leftovers:
  Vec<String>`; no hashing) and `bedrock::store::verify(&Paths, &str) -> Result<Summary>` (the full
  check). `Summary { id, version_name, version_code, files, bytes, imported }`.
* For the launch package: `bedrock::store::open(&Paths, &str) -> Result<OpenedVersion>`; keep it
  until the game has exited and give bubblewrap `opened.dir.try_clone()?`. `dir` is a read-only
  directory descriptor (`O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC`), not `O_PATH`: it carries the
  shared lock. `bedrock::store::write_stand_in_version(&Path)` writes a version the import accepts
  (for tests that should go through `open`). `bedrock::store::instance_version(&Paths, &str)`
  reads `kind = "bedrock"` / `[bedrock] version` from `instance.toml` — adjust it if the launch
  package's instance file differs from the contract's shape.
* The launch package should also: put nothing but `/bastion/bin` and `/usr/bin` in the client's
  `PATH` (no empty or relative entry: `EnvPathUtil` treats an empty entry as the current
  directory), or add the directory to `runtime::SANDBOX_PATH_DIRS`; leave `XDG_DATA_DIRS` unset or
  point it at read-only directories; keep the client's working directory read-only; not mount
  `/bastion/game` `noexec` (the client's linker maps the game's libraries with `PROT_EXEC`); and
  make `/bastion/data/lib` and `/bastion/data/mods` impossible (section 2).
* `mcpelauncher-core` stubs Microsoft's telemetry start when `MCPELAUNCHER_DISABLE_TELEMETRY=1`
  is set (`minecraft_utils.cpp`): a client-side complement to `[bedrock] telemetry = false`, for
  the risk/network packages to decide on.

## 5. Open

* A real version: run `bedrock import` and `versions --verify` on one, note its size, file
  count and depth, and the verification time at launch; adjust `LIMITS` if needed (user only).
* `bedrock` is not offered with `--events`; a front end listing versions would need it (and the
  lines as events).
* Verification is single-threaded; hashing in parallel would shorten launches if a real version
  makes it slow.
* The client's `RUNPATH` is not read: a manual install whose private libraries lie outside
  `/usr` and its data directory would fail to start in the sandbox (closed, not open).
* No APK signature check: mcpelauncher-ui keeps no signed APK to check once it has extracted
  the version.
