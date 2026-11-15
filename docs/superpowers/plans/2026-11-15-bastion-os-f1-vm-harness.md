# Bastion OS — F1: VM test harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust test harness that boots QEMU guests with OVMF Secure Boot variables in setup mode, an swtpm TPM2, the pinned and verified official Arch ISO, a serial console it can drive, and 9p shares. Every later VM test is built on it.

**Architecture:**
- A dev-only crate, `crates/vmtest`, builds QEMU's command line as a pure function and supervises QEMU and swtpm as child processes (killed on drop). It speaks QMP for screenshots, keys and power, and drives a root shell over the serial console with markers.
- The Arch ISO and swtpm come through `tests/support/` scripts that verify signatures before anything runs.

**Tech Stack:** Rust (std, serde_json), QEMU (host package), OVMF from `edk2-ovmf` (host package, `/usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd`, `OVMF_VARS.4m.fd`), swtpm 0.10.2 + libtpms (fetched as verified Arch packages), bubblewrap, gpgv, bsdtar.

**Spec:** `docs/superpowers/specs/2026-11-14-bastion-os-design.md`, section "Verification (in a VM only)". Read the index `docs/superpowers/plans/2026-11-15-bastion-os.md` first, in particular Global Constraints and C5.

## Global Constraints

The index's Global Constraints apply. In addition:
- **What may run on the host:** QEMU and OVMF from the host's own packages; swtpm only from `tests/support/fetch.sh` (Arch packages verified with gpgv against the pacman keyring), run in bubblewrap; the ISO only after its BLAKE2b sum and its signature are verified. Nothing else downloaded is executed.
- **Opt-in:** every test that starts a VM calls `vmtest::os_tests_enabled()` first. Without `BASTION_OS_TESTS=1`, it prints `skipped: set BASTION_OS_TESTS=1` and passes.
- **Socket paths:** UNIX sockets live under `/run/user/<uid>/bastion-vm-<pid>-<n>/` (well under the 108-byte limit), never under `target/`.
- **Artifacts:** every VM's files go to `target/vm/<test name>/`: `serial.log`, `qmp.log`, screenshots, disks, a copy of the OVMF variables, and the TPM state.

## Review Focus

1. **A guest that hangs** (kernel panic, firmware menu, a prompt nobody answers). Every wait has a timeout, and a timeout fails the test with the last 40 serial lines and a screenshot. Pinned by Task 3 `expect_times_out_with_the_tail_of_the_output`.
2. **Orphans after a failure.** QEMU and swtpm run in their own process group and are killed on drop and on panic. Pinned by Task 3 `dropping_a_vm_kills_qemu_and_swtpm`.
3. **Socket paths over the UNIX limit when `target/` is deep.** Pinned by Task 3 `socket_paths_stay_under_the_unix_limit`.
4. **A partial or swapped ISO download.** The BLAKE2b sum and the signature both refuse it; a partial file is deleted, never resumed. Pinned by Task 1 `fetch_iso_refuses_a_file_whose_checksum_does_not_match` and `fetch_iso_refuses_a_bad_signature`.
5. **Several VM tests at once exhausting memory.** VM tests take a shared lock in `target/vm/.lock`, so by default one VM runs at a time (`BASTION_OS_VMS=N` raises it). Pinned by Task 3 `vm_tests_take_the_shared_lock`.

---

### Task 1: The pinned, verified Arch ISO

**Files:**
- Create: `tests/support/arch-iso.toml`, `tests/support/archlinux-release-key.asc`, `tests/support/fetch-iso.sh`
- Test: `tests/os_harness.rs`

**Interfaces:**
- Produces:
  - `tests/support/arch-iso.toml` with `version = "2026.11.01"` (or the newest release when the task runs), `file`, `b2sum`, `label = "ARCH_202611"` and `signing_fingerprint = "3E80CA1A8B89F69CBA57D98A76A5EF9054449A5C"`.
  - `tests/support/fetch-iso.sh` puts the verified ISO at `target/third-party/iso/<file>`. `BASTION_ARCH_MIRROR` overrides the mirror (default `https://geo.mirror.pkgbuild.com`). `BASTION_ISO_TOML` and `BASTION_ISO_SOURCE` exist for tests only.

- [ ] **Step 1: Write the failing tests** in `tests/os_harness.rs`:
  - `the_pinned_iso_entry_names_a_version_a_checksum_and_the_release_key`: parses the toml and asserts:
    - every field is present;
    - `b2sum` is 128 hex characters;
    - `signing_fingerprint == "3E80CA1A8B89F69CBA57D98A76A5EF9054449A5C"`;
    - `label == format!("ARCH_{}", version[..7].replace('.', ""))`.
  - `fetch_iso_refuses_a_file_whose_checksum_does_not_match`: a temporary "ISO" of 1 KiB and a toml with a wrong sum, served through `BASTION_ISO_SOURCE=file://…`. The script exits non-zero, its stderr contains `checksum`, and no file is left in the output directory.
  - `fetch_iso_refuses_a_bad_signature`: the sum matches (computed in the test) and the `.sig` is garbage. The script exits non-zero and stderr contains `signature`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_harness`. Expected: FAIL (the files do not exist).
- [ ] **Step 3: Implement.**
  - Fetch the release key once over HTTPS: WKD `pierre@archlinux.org`, or archlinux.org's `pgp-keys` link.
  - Check that its fingerprint is the pinned one, and commit it as `archlinux-release-key.asc`.
  - The script:
    1. downloads `<file>` and `<file>.sig` with `curl --fail --proto =https`;
    2. checks `b2sum`;
    3. checks the signature with `gpgv --keyring <dearmored key>`;
    4. asserts the key's fingerprint against the toml;
    5. moves the ISO into place.

    On any failure it deletes the partial file.
- [ ] **Step 4: Run the tests; expect them to pass.** Run `cargo test --test os_harness`. Expected: the three tests pass.
- [ ] **Step 5: Commit.** `tests: the pinned Arch ISO, checked against its BLAKE2b sum and the release signature`

### Task 2: swtpm, verified and sandboxed

**Files:**
- Modify: `tests/support/fetch.sh` (a package set `swtpm`: swtpm, libtpms and the runtime dependencies the host lacks)
- Create: `tests/support/swtpm.sh`
- Test: `tests/os_harness.rs`

**Interfaces:**
- Produces: `tests/support/swtpm.sh <state-dir> <ctrl-socket>`. It runs `swtpm socket --tpm2 --tpmstate dir=<state-dir> --ctrl type=unixio,path=<ctrl-socket> --flags startup-clear` inside bwrap, with no network, a read-only `/usr`, and only `<state-dir>` and the socket's directory writable. It stays in the foreground.

- [ ] **Step 1: Write the failing test** `swtpm_runs_in_its_sandbox_and_creates_its_socket` (opt-in). It starts the script with a temporary state dir and asserts that the control socket appears within 5 s and that the process is alive after 1 s. Then it kills the process and asserts the state dir holds `tpm2-00.permall`.
- [ ] **Step 2: Run it; expect failure.** Run `BASTION_OS_TESTS=1 cargo test --test os_harness swtpm`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Add the set to `fetch.sh` the same way as the existing sets: SHA-256 from the sync DB, then gpgv against the pacman keyring.
  - `swtpm.sh` sets `LD_LIBRARY_PATH` to the unpacked `usr/lib`, as `private-x11.sh` does for its programs.
- [ ] **Step 4: Run it; expect it to pass.** Run the same command. Expected: PASS. Without the variable, the test prints the skip line.
- [ ] **Step 5: Commit.** `tests: swtpm from verified Arch packages, run in bubblewrap`

### Task 3: `crates/vmtest` — command line, supervision, QMP, serial

**Files:**
- Create: `crates/vmtest/{Cargo.toml,src/lib.rs,src/qemu.rs,src/qmp.rs,src/serial.rs,src/vm.rs,src/lock.rs}`
- Modify: the root `Cargo.toml` (add `crates/vmtest` to the workspace; add a `[dev-dependencies]` entry `vmtest = { path = "crates/vmtest" }`), `deny.toml` only if needed

**Interfaces:**
- Produces, as C5 outlines (the final signatures go into `docs/notes/os-vm-harness.md`):
  - `pub struct VmConfig` with:
    - `name: String`
    - `boot: Boot` (`Disk`, or `Kernel { kernel, initrd, append }`)
    - `cdrom: Option<PathBuf>`
    - `disks: Vec<Disk { path, size_gib, format }>`
    - `tpm: bool`
    - `secure_boot: SecureBoot` (`SetupMode`, `Vars(PathBuf)` or `Off`)
    - `memory_mib: u32` (default 4096) and `cpus: u32` (default 4)
    - `shares: Vec<Share { host, tag, read_only }>`
    - `net: Net` (`User { forwards: Vec<(u16, u16)> }` or `None`)
  - `pub fn qemu_argv(cfg: &VmConfig, runtime: &Runtime) -> Vec<OsString>` (pure).
  - `impl Vm`:
    - `pub fn start(cfg: VmConfig) -> Result<Vm>`
    - `pub fn serial(&mut self) -> &mut Serial`
    - `pub fn screendump(&self, png: &Path) -> Result<()>`
    - `pub fn send_keys(&self, keys: &[&str]) -> Result<()>`
    - `pub fn reboot(&mut self) -> Result<()>`
    - `pub fn wait_poweroff(&mut self, timeout: Duration) -> Result<()>`
    - `pub fn artifacts(&self) -> &Path`
  - `impl Serial`:
    - `pub fn expect(&mut self, pattern: &str, timeout: Duration) -> Result<String>` (a regex)
    - `pub fn login_root(&mut self) -> Result<()>`
    - `pub fn run(&mut self, cmd: &str, timeout: Duration) -> Result<(i32, String)>`
  - `pub fn os_tests_enabled() -> bool`.
  - `pub fn lock() -> Result<LockGuard>`.

- [ ] **Step 1: Write the failing unit tests** in `crates/vmtest`:
  - `the_command_line_has_secure_boot_ovmf_and_the_tpm_socket`: asserts all of
    - `-machine q35,smm=on`
    - `-global driver=cfi.pflash01,property=secure,value=on`
    - a pflash `OVMF_CODE.secboot.4m.fd` with `readonly=on`
    - a pflash on the VM's own variables copy
    - `-tpmdev emulator` with `-device tpm-tis`
    - `-enable-kvm`
  - `kernel_boot_appends_the_serial_console`: `-append` contains `console=ttyS0,115200`.
  - `a_read_only_share_is_passed_as_9p_readonly`: `-virtfs local,…,mount_tag=src,security_model=none,readonly=on`.
  - `qmp_negotiates_capabilities_and_screendumps`: a stand-in QMP server on a `UnixStream` pair answers the greeting. The client sends `qmp_capabilities`, then `screendump` with `"format":"png"`.
  - `run_returns_the_exit_status_between_markers`: a fake serial stream echoes a marker line with status 3; `run` returns `(3, output)`.
  - `expect_times_out_with_the_tail_of_the_output`: the error text holds the last 40 lines.
  - `socket_paths_stay_under_the_unix_limit`: with a 200-character `CARGO_TARGET_DIR`, every socket path is under 100 bytes.
  - `dropping_a_vm_kills_qemu_and_swtpm`: the child is `sleep 600` standing in for qemu; after drop, the process group is gone.
  - `vm_tests_take_the_shared_lock`: two guards with `BASTION_OS_VMS=1`; the second waits until the first drops.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p vmtest`. Expected: FAIL (the crate is empty).
- [ ] **Step 3: Implement the crate.**
  - Serial: a UNIX socket chardev. `run` writes `cmd; printf '\n__VMTEST_%d_%s__\n' $? <nonce>` and reads up to the marker.
  - QMP: `-qmp unix:<path>,server,wait=off`.
  - Display: `-display none -vga virtio` (screendump works headless).
  - Net: `-netdev user,id=n0[,hostfwd=…] -device virtio-net-pci,netdev=n0`.
  - Process groups: `setsid` through `CommandExt::process_group(0)`; kill on drop.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test -p vmtest`. Expected: all pass. `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- [ ] **Step 5: Commit.** `tests: vmtest, a QEMU harness with Secure Boot variables, swtpm, QMP and a serial shell`

### Task 4: Boot the ISO to a root shell; setup mode and the TPM are visible

**Files:**
- Create: `crates/vmtest/src/iso.rs` (`pub fn arch_iso() -> Result<PathBuf>`, `pub fn iso_kernel_boot(iso: &Path) -> Result<Boot>`)
- Test: `tests/os_harness.rs`

**Interfaces:**
- Produces:
  - `arch_iso()` runs `fetch-iso.sh` if the ISO is missing.
  - `iso_kernel_boot()` extracts `arch/boot/x86_64/vmlinuz-linux` and `initramfs-linux.img` with `bsdtar` into `target/third-party/iso/<version>/`. It returns `Boot::Kernel` with `append = "archisobasedir=arch archisolabel=<label> console=ttyS0,115200"`.
  - `Serial::wait_iso_ready()` waits for `systemctl is-system-running --wait` and for `pacman-init.service`.

- [ ] **Step 1: Write the failing opt-in tests:**
  - `vm_the_pinned_arch_iso_boots_to_a_root_shell_over_serial`: `login_root()`, then `run("uname -m")` returns `(0, "x86_64…")`.
  - `vm_secure_boot_setup_mode_and_a_tpm_are_visible_to_the_guest`:
    - the last byte of `/sys/firmware/efi/efivars/SetupMode-8be4df61-93ca-11d2-aa0d-00e098032b8c` is 1;
    - `/sys/class/tpm/tpm0/tpm_version_major` reads `2`.
- [ ] **Step 2: Run them; expect failure.** Run `BASTION_OS_TESTS=1 cargo test --test os_harness vm_ -- --test-threads=1`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Root's password is empty on the ISO, so `login_root` answers `login:` with `root`.
  - Each VM gets its own copy of `OVMF_VARS.4m.fd`, which is empty and so in setup mode.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: both pass. `serial.log` and a screenshot are in `target/vm/<test>/`.
- [ ] **Step 5: Commit.** `tests: the Arch ISO boots under OVMF in setup mode with a TPM2, driven over the serial console`

### Task 5: Shares and caches

**Files:**
- Modify: `crates/vmtest/src/vm.rs` (`Serial::mount_share(tag, at)`, plus the cache shares)
- Test: `tests/os_harness.rs`

**Interfaces:**
- Produces two read-write shares, so that VM runs do not re-download:
  - `target/vm-cache/pacman`, tag `pacman-cache`, for `/var/cache/pacman/pkg`;
  - `target/vm-cache/cargo`, tag `cargo-cache`, for `CARGO_HOME`.

  The repository itself is shared read-only with tag `src`.

- [ ] **Step 1: Write the failing opt-in test** `vm_a_9p_share_is_mounted_read_only`. It mounts `src` at `/mnt/src` and asserts:
  - `test -f /mnt/src/Cargo.toml` passes;
  - `touch /mnt/src/x` fails with "Read-only file system";
  - a file written to `/mnt/pacman-cache` appears on the host.
- [ ] **Step 2: Run it; expect failure.** Run `BASTION_OS_TESTS=1 cargo test --test os_harness vm_a_9p -- --test-threads=1`. Expected: FAIL.
- [ ] **Step 3: Implement.** The mount is `mount -t 9p -o trans=virtio,version=9p2000.L[,ro] <tag> <at>`.
- [ ] **Step 4: Run it; expect it to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `tests: 9p shares for the source tree and the package caches`

### Task 6: The harness notes

**Files:**
- Create: `docs/notes/os-vm-harness.md`

- [ ] **Step 1: Write the notes.**
  - The final public API, with signatures.
  - How to run: `BASTION_OS_TESTS=1 cargo test --test <file> -- --test-threads=1`.
  - What a VM needs: KVM; 4 GiB of RAM by default; disk space under `target/vm/`.
  - The artifact layout and how to read a failure (serial tail, screenshot).
  - What the harness deliberately does not do: no real disks, no host network bridge.
- [ ] **Step 2: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Expected: 0 failed; the VM tests print their skip line.
- [ ] **Step 3: Commit.** `docs: notes on the VM harness for Bastion OS`
