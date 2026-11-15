# Bastion OS — P1: Image and installer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The mkosi recipe and `build-image.sh` that produce a signed UKI and a verity-protected EROFS `/usr` for A/B slots. The ArchTitus-style `install.sh` that puts them on a whole disk with the owner's keys, a TPM-bound LUKS2 state partition and a recovery key. The image side of updates and fallback. All proven in QEMU.

**Architecture:**
- **Recipe:** `packaging/os/mkosi/` is the recipe; `build-image.sh` is its only entry point (the installer and the update service both call it).
- **Installer:** `install.sh` is bash with whiptail menus, or an answers file for VMs. It works in this order:
  1. checks the machine and erases nothing until every check passed and the owner typed `erase`;
  2. partitions with systemd-repart;
  3. builds the image on the new encrypted partition;
  4. installs slot A with systemd-sysupdate from that build (the same path updates take);
  5. writes the ESP with a signed systemd-boot and authenticated key files that keep Microsoft's certificates;
  6. binds the state partition to the TPM.
- **First boot:** systemd-boot enrols the keys and restarts.

**Tech Stack:** bash, whiptail, mkosi 27.1 (ToolsTree=default), systemd 262 (repart, ukify, cryptenroll, sysupdate, boot, bless-boot), sbsigntools (`sbsiglist`, `sbvarsign`, `sbsign`), cryptsetup ≥ 2.7, age-plugin-tpm, EROFS, the F1 harness.

**Spec:** `docs/superpowers/specs/2026-11-14-bastion-os-design.md`: Goals G1–G4, G9; "Install"; "Boot"; "The file system"; "Updates"; "Repair: the maintenance entry"; "Hardening"; "Verification". Read the index first, in particular Decisions 2, 3, 9, 10 and 11, and C1. The consumed interfaces come from F1 (`docs/notes/os-vm-harness.md`).

## Global Constraints

The index's Global Constraints apply. In addition:
- **Where things run:** every mkosi build and every installer run happens inside an F1 VM, never on the development machine.
- **Profiles:** `full` (default), `minimal` (no Sunshine, no mcpelauncher), and `test`. `test` is `minimal` plus a root shell on `ttyS0` and a documented way to run a command as the player inside the active session. These test hooks exist only in `test`, and a static test proves it.
- **Partition layout (target disk, GPT, Discoverable Partitions types):**
  - ESP of 1 GiB (vfat);
  - `usr` A and B, 6 GiB each, B labelled `_empty`;
  - `usr-verity` A and B, 512 MiB each;
  - `usr-verity-sig` A and B, 1 MiB each;
  - the state partition (type root x86-64, LUKS2, ext4), on the rest of the disk, label `bastion-state`.

  Sizes may grow if the `full` image needs it; record the change.
- **The TPM binding at install:** the signed PCR 11 policy only (`--tpm2-public-key-pcrs=11`). PCR 7 changes when the keys are enrolled at first boot, so PCR 7 is bound by P2's first-boot unit, through the re-binding mechanism of Decision 3.
- **Answers file:** `KEY=VALUE` lines.
  - Keys: `DISK`, `CONFIRM_ERASE` (must be `yes`), `KEYMAP`, `TIMEZONE`, `LOCALE`, `HOSTNAME`, `PLAYER`, `WIFI_SSID`, `SOURCE_URL` (default `https://github.com/Al3xSk1/Bastion-MC`), `COMMIT` (default: the newest `main` commit whose CI run passed), `PROFILE` (default `full`).
  - Any other key is refused.
  - `RECOVERY_KEY_OUT` is accepted only with `BASTION_INSTALL_TEST=1`, and only for a path under `/mnt/test/`.

## Review Focus

1. **The live environment's small RAM overlay (256 MiB) fills up while tools are installed.** The installer enlarges it first and builds on the target. Pinned by Task 5 `the_installer_enlarges_the_live_overlay_before_installing_tools`.
2. **A firmware that ignores the enrolment and leaves Secure Boot off without a word.** Pinned by Task 7 `vm_first_boot_enrols_the_keys_and_secure_boot_is_on`. P2 reports `secure_boot` and P3 shows a red banner when it is off.
3. **A maintenance boot of a slot that is not yet blessed counts down its tries.** Pinned by Task 8 `vm_a_maintenance_boot_does_not_break_a_new_slot`.
4. **An install that failed halfway, run again on the same disk.** It starts over cleanly. Pinned by Task 5 `rerunning_the_installer_starts_over_on_the_same_disk`.
5. **A recovery key the owner did not write down.** The summary shows it again and asks for its last group before restarting. Pinned by Task 6 `the_summary_asks_for_the_recovery_keys_last_group_before_restarting`.

---

### Task 1: The recipe and its static tests

**Files:**
- Create:
  - `packaging/os/mkosi/mkosi.conf` and `mkosi.conf.d/` (`00-base.conf`; `10-graphics.conf`; `20-network.conf`; `90-profile-*.conf` with `[Match] Profiles=`)
  - `mkosi.profiles/{full,minimal,test}/`
  - `mkosi.repart/{10-usr,11-usr-verity,12-usr-verity-sig}.conf`
  - `mkosi.uki-profiles/10-maintenance.conf`
  - `mkosi.extra/usr/lib/sysctl.d/50-bastion-os.conf`
  - `mkosi.extra/usr/lib/systemd/logind.conf.d/50-bastion-os.conf`
  - `mkosi.extra/usr/lib/systemd/coredump.conf.d/50-bastion-os.conf`
  - `mkosi.extra/usr/lib/systemd/system/{bastion-maintenance.target,bastion-maintenance.service}`
  - the masks (`ctrl-alt-del.target`, `debug-shell.service`)
  - `/usr/share/factory/etc` population through `mkosi.extra/usr/lib/tmpfiles.d/`
- Test: `tests/os_recipe.rs`

**Interfaces:**
- Produces the recipe that later tasks and P2/P5 extend through their own `mkosi.conf.d/` fragments and `mkosi.build.d/` scripts. Fragment numbers: 30 is Sunshine (P2), 40 is mcpelauncher (P5), 50 is services (P2).
- `[Content] KernelCommandLine=` holds:
  - `lockdown=integrity`
  - `module.sig_enforce=1`
  - `init_on_alloc=1`
  - `slab_nomerge`
  - `randomize_kstack_offset=on`
  - `vsyscall=none`
  - `debugfs=off`
  - `rd.luks.options=tpm2-device=auto,link-volume-key=<keyring spec>` (the builder fixes the spec's exact syntax for systemd 262)
  - `systemd.image_policy` requiring a signed verity `/usr`
- Packages (`full`; `minimal` drops what P2/P5 add):
  - base: base, linux, linux-firmware, intel-ucode, amd-ucode
  - graphics and session: mesa, vulkan-radeon, vulkan-intel, gamescope, xorg-xwayland, pipewire, pipewire-pulse, wireplumber
  - network and iPhone: networkmanager, wpa_supplicant, usbmuxd, libimobiledevice, ethtool
  - system: nftables, polkit
  - tokens: age, age-plugin-tpm, tpm2-tss
  - launcher needs: firefox (sign-in windows and the phone's browser), ffmpeg (clips), and the JREs the launcher's `[java] match` needs for Minecraft 1.20.1–26.x (check with `pacman -Si`: 17, 21, 25)
  - for updates on the machine: mkosi, git, python

- [ ] **Step 1: Write the failing tests** in `tests/os_recipe.rs`. They parse the INI files with a small parser in the test (sections, `Key=value`, indented continuation lines).
  - `the_kernel_is_linux_and_not_linux_hardened`.
  - `no_remote_shell_terminal_or_privilege_tool_is_installed`: none of openssh, dropbear, telnet, sudo, doas, xterm, foot, kitty, alacritty, konsole, gnome-terminal, plasma-desktop or gnome-shell.
  - `pacman_requires_signatures_and_uses_only_core_extra_multilib`.
  - `usr_is_erofs_with_verity_data_hash_and_signature`.
  - `the_uki_bootloader_and_verity_are_signed_and_pcr_11_is_predicted`.
  - `the_maintenance_profile_is_outside_the_signed_pcr_policy`: `SignExpectedPcr=no`, and `Cmdline=` contains `systemd.unit=bastion-maintenance.target`.
  - `the_sysctls_harden_without_turning_off_user_namespaces`. Required values:
    - `kernel.yama.ptrace_scope` ≥ 2
    - `kernel.kptr_restrict=2`
    - `kernel.dmesg_restrict=1`
    - `kernel.unprivileged_bpf_disabled=1`
    - `net.core.bpf_jit_harden=2`
    - `fs.protected_symlinks=1`, `fs.protected_hardlinks=1`, `fs.protected_fifos=2`, `fs.protected_regular=2`
    - `kernel.sysrq=0`

    Neither `user.max_user_namespaces=0` nor `kernel.unprivileged_userns_clone=0` may appear.
  - `the_kernel_command_line_locks_down_and_requires_a_signed_usr`.
  - `only_tty1_is_used_and_ctrl_alt_del_and_the_debug_shell_are_masked`: `NAutoVTs=0`, `ReserveVT=0`.
  - `coredumps_are_off`.
  - `profiles_full_minimal_and_test_exist_and_full_is_the_default`.
  - `the_test_hooks_exist_only_in_the_test_profile`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_recipe`. Expected: FAIL (no recipe).
- [ ] **Step 3: Implement the recipe.** Follow ParticleOS's layout for the repart definitions and the UKI. The maintenance service runs `sulogin --force` on tty1 with `Environment=SYSTEMD_SULOGIN_FORCE=1` and conflicts with `bastion-session.service`.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test --test os_recipe`. Expected: PASS.
- [ ] **Step 5: Commit.** `os: the mkosi recipe (verity /usr, signed UKI with a maintenance profile, hardening) and its static tests`

### Task 2: Pinned sources

**Files:**
- Create: `packaging/os/sources.toml` (header comment and format, no entries yet), `packaging/os/mkosi/fetch-pinned`
- Test: `tests/os_recipe.rs`

**Interfaces:**
- Produces:
  - Entries of the form `[[source]] name, for, version, url, sha256, licence`.
  - `fetch-pinned <name> <dest-dir>`:
    1. downloads with `curl --fail --proto =https` into `$PINNED_CACHE` (the build cache);
    2. checks SHA-256;
    3. on a mismatch, deletes the file and exits 1 with `pinned source <name>: checksum mismatch`;
    4. prints the path.
  - P2 (Sunshine) and P5 (mcpelauncher) add entries.

- [ ] **Step 1: Write the failing tests:**
  - `every_pinned_source_has_a_name_url_sha256_and_licence`.
  - `pinned_urls_are_https`.
  - `fetch_pinned_refuses_a_mismatched_file`: `file://` is accepted only with `BASTION_FETCH_PINNED_TEST=1`.
  - `fetch_pinned_reuses_a_verified_cached_file`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_recipe pinned`. Expected: FAIL.
- [ ] **Step 3: Implement** in bash. Parse the toml with `python3 -I -c` (python is in the tools tree and on the ISO).
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: sources the recipe builds from, pinned by SHA-256`

### Task 3: bastion-mc built into the image

**Files:**
- Create: `packaging/os/mkosi/mkosi.build.d/10-bastion-mc.chroot`, `packaging/os/mkosi/mkosi.postinst.chroot`, `packaging/os/mkosi/record-component`, `packaging/os/player-config.toml`
- Test: `tests/os_recipe.rs`

**Interfaces:**
- Produces:
  - `/usr/bin/bastion-mc` and `/usr/bin/bastion-mc-gui`, plus `/usr/lib/bastion-os/bastion-os` when `crates/os` exists (P2), all built with `cargo auditable build --release --locked` from the source tree that `build-image.sh` passes in. `packaging/check-hardening.sh` must pass on each binary, or the build fails.
  - `/usr/share/bastion-os/player-config.toml` (C1).
  - `/usr/lib/os-release` with C1's fields.
  - `/usr/lib/bastion-os/components.json` (`{}` unless P2/P5 add to it).
  - `record-component <name> <version> <sha256>…`, which P2/P5 call from their build scripts.

- [ ] **Step 1: Write the failing tests:**
  - `the_build_script_uses_the_locked_tree_and_checks_hardening`.
  - `os_release_gets_id_bastion_os_the_version_and_the_commit`: runs the postinst against a fake root.
  - `the_player_config_template_matches_the_contract`: parses it and compares it with C1.
  - `record_component_writes_valid_json`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_recipe build`. Expected: FAIL.
- [ ] **Step 3: Implement.** Use the PKGBUILD's flags and its `check-hardening.sh` call (packaging/arch/PKGBUILD). The build gets network (`WithNetwork=yes`) for `cargo fetch --locked` only.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: build bastion-mc into the image from the locked tree, with the hardening check`

### Task 4: `build-image.sh`, keys, and a VM build

**Files:**
- Create: `packaging/os/build-image.sh`, `packaging/os/install/lib/keys.sh` (`make_keys <dir>`)
- Test: `tests/os_install.rs` (`vm_build_image_makes_signed_verifiable_artifacts`)

**Interfaces:**
- Produces:
  - `build-image.sh --workspace DIR --keys DIR --source DIR --commit SHA --version V [--profile full|minimal|test] --out DIR`. It checks first:
    - at least 30 GiB free under `--workspace`;
    - `--keys` is root-only (0700) and holds the expected files;
    - `--source` is a clean git checkout whose `HEAD` is `--commit`;
    - `V` matches `^[0-9]{8}\.[0-9]+$`.
  - Outputs in `--out`, named as the sysupdate transfers in Task 8 expect. Record the final names in the notes:
    - the UKI (both profiles);
    - `usr`, `usr-verity` and `usr-verity-sig` partition images;
    - `systemd-bootx64.efi.signed`, taken from the built `/usr` and signed with `db`;
    - `SHA256SUMS`.
  - `make_keys <dir>` creates `PK`, `KEK`, `db`, `verity` and `pcr` keys and certificates with `openssl` (RSA 4096 or EC P-256, recorded in the notes), with files 0600 and the directory 0700. `pcr` has its public key as PEM for cryptenroll.

- [ ] **Step 1: Write the failing opt-in test** `vm_build_image_makes_signed_verifiable_artifacts`. In an ISO VM (8 GiB RAM, 8 CPUs, a 64 GiB scratch disk, the cache shares) it:
  1. installs `mkosi git sbsigntools systemd-ukify` with pacman;
  2. clones the shared repository at `HEAD`;
  3. runs `make_keys`, then `build-image.sh --profile minimal`.

  It then asserts, inside the VM:
  - `sbverify --cert db.crt` accepts the UKI and the signed systemd-boot;
  - `ukify inspect` shows the main and maintenance profiles;
  - `veritysetup verify` of the usr image with the root hash from the UKI's `usrhash=` succeeds;
  - the verity signature verifies against `verity.crt`.
- [ ] **Step 2: Run it; expect failure.** Run `BASTION_OS_TESTS=1 cargo test --test os_install vm_build -- --test-threads=1`. Expected: FAIL.
- [ ] **Step 3: Implement.** Pass the keys through mkosi's options (`SecureBootKey=`/`SecureBootCertificate=`, `SignExpectedPcrKey=`/`SignExpectedPcrCertificate=`, `VerityKey=`/`VerityCertificate=`) and `SplitArtifacts=uki,partitions`.
- [ ] **Step 4: Run it; expect it to pass.** Run the same command. Expected: PASS, with the artifacts and the build log in `target/vm/vm_build_image_makes_signed_verifiable_artifacts/`.
- [ ] **Step 5: Commit.** `os: build-image.sh, the machine's keys, and a VM build whose signatures and verity check out`

### Task 5: The installer's entry, menus, answers and checks

**Files:**
- Create: `packaging/os/install.sh`, `packaging/os/install/lib/{common,checks,menus,answers}.sh`
- Test: `tests/os_install.rs` (static tests that source the functions with `BASTION_INSTALL_ROOT=<fake root>` and stub commands first on `PATH`)

**Interfaces:**
- Produces: `install.sh [--answers FILE] [--source URL] [--commit SHA] [--profile P]`.
  - It runs with `set -Eeuo pipefail`.
  - It logs to `/tmp/bastion-install.log`, which is copied to `/var/log/bastion-install.log` on the target.
  - On error it says which step failed and where the log is.
  - Check order: UEFI on x86_64 → network → TPM2 → Secure Boot setup mode → disks. Nothing is written before every check passed and the confirmation was given.
  - The confirmation names the disk's model, size and path, and requires typing `erase`.

- [ ] **Step 1: Write the failing tests:**
  - `a_bios_boot_is_refused`.
  - `a_non_x86_64_machine_is_refused`.
  - `without_network_the_installer_says_so_before_anything_else`.
  - `without_a_tpm2_the_installer_refuses_with_the_reason`.
  - `outside_setup_mode_the_installer_explains_how_to_reset_the_keys`: the message names "Reset to Setup Mode" or "Clear Secure Boot keys", keeping Secure Boot on, and starting the installer again.
  - `the_disk_the_iso_runs_from_is_never_offered`: from a stub `lsblk --json` fixture.
  - `the_confirmation_names_the_model_and_size_and_needs_the_word_erase`.
  - `an_answers_file_with_an_unknown_key_is_refused`.
  - `an_answers_file_without_confirm_erase_is_refused`.
  - `the_installer_enlarges_the_live_overlay_before_installing_tools`.
  - `rerunning_the_installer_starts_over_on_the_same_disk`: the disk step wipes signatures and the old GPT first, shown with stubs.
  - `shellcheck_finds_nothing`: skipped if shellcheck is not installed.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_install`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Interactive mode uses whiptail menus. Answers mode prints plain text.
  - The setup-mode check reads the last byte of `SetupMode-8be4df61-93ca-11d2-aa0d-00e098032b8c`.
  - The TPM check reads `/sys/class/tpm/tpm0/tpm_version_major`.
  - The disks come from `lsblk --json -b -o NAME,PATH,MODEL,SIZE,TYPE,TRAN,MOUNTPOINTS`, excluding the disk under `/run/archiso/bootmnt`.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test --test os_install`. Expected: PASS.
- [ ] **Step 5: Commit.** `installer: checks that refuse with their reason, menus, an answers file, and a typed confirmation`

### Task 6: The installer's steps — disk, keys, image, boot, TPM, system

**Files:**
- Create:
  - `packaging/os/install/lib/{disk,image,boot,tpm,system,finish}.sh`
  - `packaging/os/repart/{00-esp,10-usr-a,11-usr-a-verity,12-usr-a-verity-sig,20-usr-b,21-usr-b-verity,22-usr-b-verity-sig,30-state}.conf`
  - `packaging/os/loader.conf`
  - `packaging/os/secureboot/microsoft/{MicCorUEFCA2011.der, microsoft-uefi-ca-2023.der, microsoft-option-rom-uefi-ca-2023.der, MicCorKEKCA2011.der, microsoft-kek-2k-ca-2023.der, MANIFEST}` (names as downloaded; `MANIFEST` lines: `sha256  file  fwlink  subject`)
- Test: `tests/os_install.rs`

**Interfaces:**
- Consumes: Task 4's `build-image.sh` and `make_keys`; Task 3's `player-config.toml`.
- Produces, on the target:
  - **State layout:**
    - `/var/lib/bastion-os/{keys,build,src}`;
    - `/etc/bastion-os/update.conf` (keys `source_url`, `api_url` (= `https://api.github.com`), `repo` (= `Al3xSk1/Bastion-MC`), `branch` (= `main`), `workflow` (= `ci.yml`), `profile`; root 0644);
    - `/etc/bastion-mc/age-recipients` (root 0644);
    - the player's home with `.config/bastion-mc/config.toml` and `.local/share/bastion-mc/vault/tpm-identity.txt` (1000:1000, 0600);
    - `/etc/sysusers.d/bastion-player.conf`;
    - `/var/log/bastion-install.log`.
  - **ESP:**
    - the signed systemd-boot at `/EFI/systemd/systemd-bootx64.efi` and `/EFI/BOOT/BOOTX64.EFI`;
    - the UKI in `/EFI/Linux/` (from sysupdate);
    - `loader.conf` with `secure-boot-enroll force`, `secure-boot-enroll-action reboot`, `editor no`, `timeout 3`;
    - `/loader/keys/bastion/{PK,KEK,db}.auth`.
  - **LUKS key slots:** one TPM2 slot (signed PCR 11 policy) and one recovery key. The temporary key file the disk was created with is wiped.

- [ ] **Step 1: Write the failing tests:**
  - `the_microsoft_certificates_match_their_pinned_hashes_and_subjects`. The subjects:
    - `Microsoft Corporation UEFI CA 2011`
    - `Microsoft UEFI CA 2023`
    - `Microsoft Option ROM UEFI CA 2023`
    - `Microsoft Corporation KEK CA 2011`
    - `Microsoft Corporation KEK 2K CA 2023`

    The subject half is skipped without openssl.
  - `the_auth_files_put_the_microsoft_certificates_in_db_and_kek_under_their_owner_guid`: runs `make_auth_files` with stub `sbsiglist`/`sbvarsign` that record their arguments.
  - `the_repart_definitions_make_an_esp_two_usr_slots_and_an_encrypted_state`.
  - `loader_conf_enrols_with_force_reboots_and_has_no_editor`.
  - `the_tpm_is_bound_to_the_signed_pcr_11_policy_only_at_install`: the cryptenroll arguments, recorded by a stub.
  - `the_recovery_key_is_written_to_a_file_only_in_test_mode`.
  - `the_summary_asks_for_the_recovery_keys_last_group_before_restarting`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_install`. Expected: FAIL.
- [ ] **Step 3: Implement the steps in this order:**
  1. `prepare_live`: `mount -o remount,size=4G /run/archiso/cowspace`, then `pacman -Sy --needed mkosi git sbsigntools efitools age age-plugin-tpm tpm2-tools`.
  2. `partition_disk`: wipefs, then `systemd-repart --empty=force` with a temporary key file on tmpfs.
  3. `open_state`, and mount it.
  4. `make_keys`, then `make_auth_files`:
     - sbsiglist with this machine's own GUID for its certificate and Microsoft's GUID for theirs;
     - sbvarsign: PK signs PK and KEK, KEK signs db.
  5. `fetch_source`: a git clone, and the commit check. The default `COMMIT` comes from the same GitHub API query as P2's updater; its JSON is parsed with `python3 -I`.
  6. `build_image`: runs the checkout's own `build-image.sh`, with version `<today>.1`.
  7. `install_slot_a`: `systemd-sysupdate --image=<disk> --transfer-source=<out> update`, or the equivalent the builder establishes; recorded in the notes.
  8. `install_boot`.
  9. `bind_tpm`: TPM2 plus the recovery key, then wipe the temporary slot.
  10. `write_system`: hostname, keymap (including in the initrd, through a credential or the builder's equivalent), locale, time zone, sysusers, the player's home and config, the age identity and its recipient, an optional Wi-Fi profile, and the log.
  11. `finish`: the summary, with the recovery key again.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test --test os_install`. Expected: PASS.
- [ ] **Step 5: Commit.** `installer: partition, keys with Microsoft's certificates kept, build, slot A, boot files, TPM and recovery key`

### Task 7: Install and first boot, in VMs

**Files:**
- Test: `tests/os_install.rs`
- Modify: `crates/vmtest`, only for helpers a test needs (an ESL parser `vmtest::esl::certificates(&[u8]) -> Vec<Vec<u8>>`; raw sparse disk copies). Record them in the harness notes.

**Interfaces:**
- Consumes: F1 (`Vm`, `Serial`, the shares); Tasks 1–6.
- Produces: a cached installed disk per profile, `target/vm/installed-<profile>.raw`, with its OVMF variables and TPM state. Later VM tests copy it (sparse or reflink) instead of installing again.

Runtime checks use the `test` profile. The production-shape checks use `minimal`.

- [ ] **Step 1: Write the failing opt-in tests:**
  - `vm_install_from_the_iso_completes`: the answers file with `PROFILE=test` and `SOURCE_URL=file:///mnt/srcgit`, a bare clone of `HEAD` made by the test.
  - `vm_first_boot_enrols_the_keys_and_secure_boot_is_on`:
    - systemd-boot's enrolment notice is seen, followed by one restart;
    - `bootctl status` shows Secure Boot enabled (user);
    - db holds this machine's certificate and the three Microsoft db certificates, compared by SHA-256 with `MANIFEST`;
    - KEK holds the two Microsoft KEKs.
  - `vm_the_state_partition_unlocks_by_tpm_without_a_prompt`: no passphrase prompt on the serial console. `systemd-cryptenroll` lists exactly a tpm2 slot and a recovery slot.
  - `vm_a_changed_tpm_state_asks_for_the_recovery_key`: a fresh swtpm state, then the prompt. Typing the recovery key boots.
  - `vm_usr_is_verity_protected_and_a_flipped_byte_stops_the_boot`: the test flips one byte inside usr A on the host. The boot never reaches multi-user, and the console names the verity failure.
  - `vm_the_maintenance_entry_asks_for_the_recovery_key_and_opens_a_root_shell`: the test picks the maintenance profile in systemd-boot with QMP keys. The prompt appears, the recovery key opens a root shell (`id -u` is 0), and `bastion-session.service` is inactive.
  - `vm_no_other_terminals_no_root_login_and_no_setuid_files`: profile `minimal`.
    - No login prompt on `ttyS0` within 60 s.
    - Then the disk is checked offline from an ISO VM: the state is opened with the recovery key and usr A is mounted through verity.
    - `find /usr -perm -4000 -o -perm -2000` gives only the allowed list, recorded in the notes (ideally none).
    - root's password is locked, and `getty@tty2` is not enabled.
- [ ] **Step 2: Run them; expect failure.** Run `BASTION_OS_TESTS=1 cargo test --test os_install vm_ -- --test-threads=1`. Expected: FAIL.
- [ ] **Step 3: Fix what the runs show.** These are integration tests of Tasks 1–6; each fix goes into the task's files, with its static test where one applies.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: all pass; the artifacts are kept.
- [ ] **Step 5: Commit.** `os: install, first boot, verity, TPM, recovery and maintenance checked in VMs`

### Task 8: Updates and fallback, on the image side

**Files:**
- Create: `packaging/os/mkosi/mkosi.extra/usr/lib/sysupdate.d/{10-uki,20-usr,21-usr-verity,22-usr-verity-sig}.transfer`
- Test: `tests/os_recipe.rs` (static); `tests/os_install.rs` (VM)

**Interfaces:**
- Produces transfers modelled on ParticleOS's `mkosi.sysupdate/`:
  - partitions matched by type and `@v` labels, `ReadOnly=1`;
  - the UKI into `/EFI/Linux` with `TriesLeft=3 TriesDone=0`;
  - `InstancesMax=2` and `ProtectVersion=%A`;
  - default local source `/var/lib/bastion-os/build/out`, overridden with `--transfer-source`.

  P2's `bastion-os update` runs `/usr/lib/systemd/systemd-sysupdate --transfer-source=<out> update`.

- [ ] **Step 1: Write the failing tests:**
  - Static:
    - `the_transfers_cover_the_uki_and_the_three_usr_partitions`;
    - `new_ukis_start_with_three_tries`;
    - `the_booted_version_is_protected`.
  - VM, test profile:
    - `vm_an_update_installs_into_slot_b_and_boots_it`: build `<today>.2` inside the VM, run `systemd-sysupdate update`, and reboot. `IMAGE_VERSION` is the new one, and the entry is blessed after boot-complete.
    - `vm_a_broken_slot_falls_back_to_a_by_itself`: install `.3`, then flip a byte in its usr. After the tries run out, `.2` boots without help.
    - `vm_an_interrupted_update_leaves_slot_a_bootable`: QMP `quit` while sysupdate writes. The next boot is the current version, and the partial slot is not offered.
    - `vm_a_maintenance_boot_does_not_break_a_new_slot`: install `.4` and boot maintenance once, then boot normally. `.4` boots and is blessed.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_recipe transfer`, then `BASTION_OS_TESTS=1 cargo test --test os_install vm_ -- --test-threads=1`. Expected: FAIL.
- [ ] **Step 3: Implement the transfers,** and fix what the VM runs show.
- [ ] **Step 4: Run them; expect them to pass.** Run the same commands. Expected: PASS.
- [ ] **Step 5: Commit.** `os: A/B updates through systemd-sysupdate with boot counting and fallback, checked in VMs`

### Task 9: The notes

**Files:**
- Create: `docs/notes/os-p1.md`

- [ ] **Step 1: Write the notes.**
  - The final artifact names and `build-image.sh` options.
  - The keys' algorithms.
  - The sizes.
  - The test profile's hooks.
  - The allowed setuid list.
  - Every writable path on the installed system, and why it is writable (the spec asks for this; `/tmp` is a tmpfs).
  - The keymap mechanism.
  - Every `Ruling:` this package made.
  - Measured times for a minimal and a full build in the VM.
  - User-facing text for docs/OS.md: the install steps, the confirmation, the first-boot notice, and keeping the recovery key.
- [ ] **Step 2: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Expected: 0 failed; VM tests skipped without the variable.
- [ ] **Step 3: Commit.** `docs: notes on the Bastion OS image and installer`
