# Running Minecraft Bedrock on Linux for bastion-mc: research report (2026-11-14)

**Method and rules kept.** Everything here comes from reading code and public pages. I built nothing, ran nothing, installed nothing (no pip) and signed in nowhere. Everything was written under `target/research/`. The read-only clones are under `/home/ski/RustroverProjects/bastion-mc/target/research/`:

| Directory | Repository | Commit |
|---|---|---|
| `bol` | BedrockOnLinux | e6c3965, 2026-11-12 |
| `xodus` | Xodus | 49453a4 |
| `mcpe-mcpelauncher-client` | mcpelauncher client | 7890498 |
| `mcpe-mcpelauncher-ui-qt` | mcpelauncher UI | 3f955bf |
| `mcpe-mcpelauncher-ui-manifest` | mcpelauncher UI manifest | 2e9fdaf |
| `mcpe-mcpelauncher-manifest` | mcpelauncher manifest | fb58aa2 |
| `mcpe-mcpelauncher-gdk-dependencies` | GDK replacement DLLs | add3219 |
| `gophertunnel` | Bedrock protocol library | e2251a6 |
| `go-nethernet` | NetherNet library | 4c06b55 |
| `xgameruntime` | Xodus xgameruntime | f03484d |

Fetched pages are in `web/`, and the one helper script I wrote is `scripts/ghmeta.py`.

**Labels used below:**
- **[code]** — read in source at the commit above.
- **[official]** — a minecraft.net, Microsoft or Mojang page.
- **[wiki]** — minecraft.wiki.
- **[3rd]** — another third-party source.
- **[inferred]** — my reasoning; not observed.

This is not legal advice.

---

## 0. Decision summary

1. **How BedrockOnLinux (BoL) works.** It runs the Windows (GDK) build, and it depends on two things.
   - **Xodus gets and decrypts the game.** Xodus registers an emulated "Microsoft Store device" on your account, fetches the title licence and decrypts the encrypted package. It decrypts the game executable again into memory at every start, so the game cannot start offline.
   - **The launcher signs the game in, and the game gets everything.** The launcher does the Xbox sign-in using **Minecraft-for-Android's client ID**. It then puts the account's **MSA refresh token, the Xbox device private key and every XSTS token** where the game process reads them.
   - **It isolates nothing.** The Steam runtime container it uses shares `$HOME` by default. Only the Flatpak build is partly sandboxed.
2. **DRM.**
   - **The Windows/GDK route is DRM circumvention.** It does not defeat the licence check: Microsoft issues the licence only to an owning account, and it is fetched at every start. But Xodus recovers keys that Microsoft protects by obfuscation and decrypts outside Microsoft's protected path. The same binary also has a command that writes the content keys to files.
   - **Microsoft's terms forbid it.** The Microsoft Services Agreement (published 2026-07-30) says you may not "circumvent or bypass any technological protection measures" or "decrypt … emulate". Under 17 U.S.C. §1201 (US law), decrypting without the owner's authority is the textbook definition of circumvention.
   - **WineGDK also faked the licence.** Upstream WineGDK (the Wine fork BoL builds on) answered the game's licence query with a hard-coded "active" licence. BoL removes that fake.
   - **Conclusion:** this route is incompatible with "nothing may defeat licence checks / no circumvention", unless the user decides otherwise.
3. **Android route (mcpelauncher).**
   - **No encryption is broken.** The game files come from Google Play's delivery service, for an account that **bought Minecraft on Google Play**. That is a separate purchase from the PC bundle.
   - **The game's own licence query is stubbed.** The game's Google Play licence query is answered "verified" by a stub, after mcpelauncher's UI has checked ownership with Google itself. That is a licence-check *substitution* the user must knowingly accept.
4. **Without any circumvention.** The clean options are a genuine Bedrock device (phone, console, Windows PC or Windows VM), or **Waydroid** with real Google Play. bastion-mc cannot sandbox Waydroid: it needs a root container service and binder in the host kernel.
5. **Capes.**
   - **Bedrock-only gates keep recurring.** Since 2023 Mojang has put cross-edition capes behind actions only possible in Bedrock:
     - event servers: Cherry Blossom 2023, MCC 15th Year 2024, Yearn 2025;
     - sign-in: the Common Cape, 2025-05-06 → 2026-05-05;
     - in-game challenges: Copper (2025-10) and Zombie Horse (2025-12).
   - **They reach Java.** Official pages say these capes are wearable on Java too. That they then appear in `GET /minecraft/profile` `capes` (where `bastion-mc cape set` can equip them) is how Java capes work, but I **did not verify it with an account**.
   - **Many capes need no Bedrock at all:** codes at minecraft.net/redeem, or the Dungeons II login.
   - **Currently open (2026-11-14):** only the Hero Cape, which needs Dungeons I + II, not Bedrock.
6. **Security, whatever build runs.**
   - **The game holds the account's Xbox identity.** On the Android route that is an MSA refresh token for client `0000000048183522` with Xbox user-auth scope. Nothing like the Java credential broker is possible.
   - **What bastion-mc can still enforce:**
     - the filesystem;
     - a network allowlist;
     - display, audio and GPU exposure;
     - resource limits;
     - its own confinement;
     - wiping the game's token store after each session;
     - keeping the Java tokens out of a Bedrock session entirely.
7. **Recommendation.**
   - **(A) For capes alone:** use a genuine Bedrock device for the minutes an event needs, then `bastion-mc cape set` on Java.
   - **(B) If Bedrock must run on this machine under bastion-mc:** add a `bedrock` instance kind.
     - It runs an **installed, root-owned `mcpelauncher-client`** in bastion's existing sandbox, the same policy as the JDK.
     - The game files are the ones the user downloaded with mcpelauncher-ui from their own Google Play purchase.
     - This is conditional on the user accepting point 3.
   - **(C) Do not wrap or reimplement BoL/Xodus.**

---

## 1. BedrockOnLinux in detail

### 1.1 Repository facts [code / GitHub API]

**Project.**
- https://github.com/Wyze3306/BedrockOnLinux, at e6c3965 (2026-11-12); version 2.3.1.
- Created 2026-05-16; ~1.2k stars, 16 open issues, commits almost daily.
- Python 3 with a PySide6 GUI; 272 files.
- Three prebuilt Windows helper executables are committed (`bol/injector.exe`, `fullscreen-nudge.exe`, `desktop-bridge.exe`); their sources are in `src/`.

**Licence.**
- `LICENSE` is MIT, plus a notice that "GDK-Proton, umu-launcher, ProxyPass and Eclipse Temurin are downloaded at runtime".
- ProxyPass and Temurin are referenced nowhere in the code, so the notice is stale.

**The README contradicts the code on sign-in.**
- The README says "You sign in to Microsoft from inside Minecraft".
- The code does the opposite: the launcher signs in (device-code flow) and **patches the in-game sign-in link out**. The comment says "the in-game button reaches a sign-in the engine does not implement" (`bol/launch.py`, `bol/fixups.py`).

### 1.2 How it obtains the game: Xodus

**What Xodus is.**
- https://github.com/xodus-gaming/xodus — **GPL-3.0**, Rust, created 2025-11-11, ~1.9k stars.
- It describes itself as aiming "to bring XBOX PC games to Linux".
- BoL pins commit `64d39eb8` plus 11 of its own patches (rev `64d39eb87a56-p11`).
- BoL builds it in CI, publishes the binary as a release asset and downloads it on first use, SHA-256-pinned (`bol/xodus.py` `ensure_cli`, `bol/config.py`).
- BoL never links it; it only executes `xodus-cli`.

**What it does with your Store account.** Sources: [code] `xodus/docs/xodus/{device,clep,licenses}.md`, `crates/xodus/src`, `crates/xodus-cli/src/commands/{run,license}.rs`, `bol/xodus.py`, `bol/third_party/xodus/README.md`.

- **Device registration.**
  - It POSTs to `login.live.com/ppsecure/deviceaddcredential.srf` with a random member name and password, plus hardware "components": the obfuscated CLEP challenge blobs (device-fingerprint data Windows sends), and SMBIOS version/serial/UUID if readable.
  - On Linux it reads SMBIOS with **`pkexec cat /sys/firmware/dmi/entries/1-0/raw`** (`crates/xodus/src/hardware.rs:76`). That is the "password prompt" in the README; cancelling registers the device without it.
  - **Every registration adds a Store device to your account, and the limit is 10.** BoL issue #198: lost keyrings used them up. BoL then points users to account.microsoft.com/devices/content.
- **Device key extraction.**
  - The reply is a licence blob (an `SPLicenseBlock`) carrying `ClepSignState`: the device's RSA private key, AES-encrypted.
  - Xodus "reads the decryption key back out of four fixed offsets in key_schedule" (`docs/xodus/clep.md`). That is the obfuscation the "white-box" wording above refers to.
  - It signs its token requests to `RST2.srf` with that key and gets a device token valid for 60 days.
- **User sign-in.**
  - `xodus-cli login` opens `login.live.com` `InlineLogin.srf` in a wry/tao (WebKitGTK) window. It imitates Windows' token broker by sending its CloudExperienceHost headers.
  - It stores legacy Microsoft-account (RPS) tokens: `user-DA` and `user-tokens` (90 days, renewed in their last week).
- **Licence.**
  - It POSTs to `licensing.mp.microsoft.com/v7.0/licenses/content` with the device token, user token and content id, and gets another licence blob.
  - The device licence decrypts to an `EncryptedDeviceKey`, which unwraps the packed content keys (`docs/xodus/licenses.md`).
  - A refusal means "not owned".
- **Download.**
  - It resolves the product through `displaycatalog.mp.microsoft.com`: Minecraft for Windows is `9NBLGGH2JHXJ` (content id `7792d9ce-…`); Preview is `9P5X4QVLC2XR` (content id `98bd2335-…`).
  - `packagespc.xboxlive.com` returns the current build only.
  - It streams the MSIXVC package over **plain HTTP** from `assets*.xboxlive.com` and AES-XTS-decrypts it on the fly into the game directory, with incremental updates.
  - **Older builds** come from a third-party URL index (`raw.githubusercontent.com/MinecraftBedrockArchiver/GdkLinks/master/urls.json`). BoL accepts only entries on `*.xboxlive.com`, ending in `.msixvc`, with the right content id. A licence is still required.
- **Every start.**
  - Segments flagged `KEEP_ENCRYPTED_ON_DISK` — the game executable — stay encrypted inside `.xodus-streaming.msixvc` beside the game.
  - `xodus-cli run <game_dir> <wrapper>` fetches the licence again, decrypts those segments into memfds, sets `WINE_DLL_FILE_MAP` and execs the wrapper (`run.rs:23,242`).
- **Content-key dump.** The same binary has `xodus-cli license … <dir>`, described as "Dump CIKs for use with XvdTool" (it writes the content keys to files). BoL does not call it.
- **Keyring.** Tokens go to the file `$HOME/.xodus-keyring.ron` (built with the `key-chain-file` feature); BoL sets `HOME=<data>/xodus-home`.

**Does this cross into DRM circumvention?** Yes, technically.
- **It is not a licence-check bypass.** Ownership is verified by Microsoft at download and at every start.
- **But it decrypts with keys extracted from obfuscated licence blobs, outside the Windows licensing stack.** 17 U.S.C. §1201(a)(3)(A) defines circumvention as "to decrypt an encrypted work … without the authority of the copyright owner" (https://www.law.cornell.edu/uscode/text/17/1201). The interoperability exception §1201(f) is narrow and untested for this use.
- **The EU equivalents** are Art. 6 of Directive 2001/29/EC and Art. 7(1)(c) of Directive 2009/24/EC (from knowledge, not fetched).
- **Microsoft's terms forbid it explicitly.** The Microsoft Services Agreement (https://www.microsoft.com/en-us/servicesagreement, "Published: July 30, 2026") says you may not "i. circumvent or bypass any technological protection measures in or relating to the software or Services; ii. disassemble, decompile, decrypt, hack, emulate, exploit, or reverse engineer…".
- **Shipping it adds a second exposure.** For a project that *ships* such an integration, §1201(a)(2)/(b) (offering circumvention tools) is an additional exposure.
- **Operationally it is fragile.** On 2026-10-12, a Microsoft change to its sign-in tickets broke every launch and download until Xodus patch 0006.

### 1.3 Wine/Proton build and components [code: `bol/config.py`, `docs/BUILD.md`, `bol/{proton,winegdk,vkd3d,fixups,gameinput,leftovers}.py`, `third_party/*/README.md`]

**Engine.**
- The managed engine is `wow64-archs-native22`.
- It is Weather-OS/**GDK-Proton** `release10-32` (a GE-Proton derivative), overlaid with:
  - **WineGDK** at commit `75637b67` plus BoL patches 0001–0013;
  - **vkd3d-proton v3.0.1**, rebuilt with an NVIDIA graphics feature restored and an occlusion-latency patch;
  - **DXVK 3.0.1**, swapped in for the base's 2.7.1.
- Downloaded from the BoL GitHub release `engine-native22`, SHA-256-pinned, built reproducibly in CI, with attestations.

**Running it.**
- **umu-launcher 1.4.3** (pinned) starts it.
- umu downloads the **Steam Linux Runtime 3.0 "sniper"** from `repo.steampowered.com`, checked only against `SHA256SUMS` from the same host (`umu/umu_runtime.py`).
- `PROTON_USE_WOW64=1`; ntsync is used; Wine's Wayland driver is opt-in.

**DLLs swapped into the game.**
- `libHttpClient.GDK.dll` and `XCurl.dll` from minecraft-linux/mcpelauncher-gdk-dependencies v0.0.0 (MIT, ChristopherHX), SHA-pinned.
- An "OpenSSL XCurl set" (the msys2 curl/OpenSSL closure plus BoL's CA shim), SHA-pinned. It exists because Azure resets TLS from Wine's GnuTLS.
- A `cryptbase.dll` stub (provides `RtlGenRandom`).

**Runtimes the game needs.**
- **Visual C++ runtimes:** none installed; there is no winetricks or vcredist anywhere.
- **GameInput:** the native Microsoft redistributable, extracted in pure Python from the game package's `Installers/GameInputRedist.msi`. Wine's built-in `gameinput` is disabled when it is present.
- **WebView2:** ships inside the game package and runs under Wine (`msedgewebview2.exe`, `MicrosoftEdgeUpdate.exe`; BoL kills the updater after exit). It is **not used for sign-in**.
- **WindowsAppRuntime:** cannot install under Wine; its bootstrap UI is muted. The WinRT file pickers are implemented in the WineGDK patches.

**Binary patches to Wine.**
- `combase.RoOriginateErrorW` → returns success. The comment says it "aborts at the package-identity check".
- `ntdll` stub funnels → `STATUS_NOT_IMPLEMENTED`.

**Patches to game files.**
- The Ore UI (HBUI) JavaScript: the Microsoft-account gate on Servers/Realms is neutralised, and the in-game Sign-in link removed.
- A **partner server "Linesia"** (`play.linesia.net:19132`) is injected as a *Featured Server* and seeded into the user's server list (`bol/branding.py`, `prefix.py`).
- The title splashes are replaced.
- The PE stack reserve of the executable is raised.

**The game's licence query.**
- At WineGDK `75637b67`, `XStoreQueryGameLicense` completed with a **hard-coded active licence for `9NBLGGH2JHXJ`**.
- BoL patch `0006-xgameruntime-stop-faking-xstore-answers.patch` makes it return `E_GAMESTORE_NETWORK_ERROR` instead. The game then runs "with the store disabled".

**State of WineGDK.** The upstream repository is now **archived**. It was split into xodus-gaming/xgameruntime (LGPL-2.1; at f03484d it only contains threading code) and xodus-gaming/wine.

### 1.4 How Xbox sign-in works under Wine [code: `bol/auth.py`, `bol/launch.py`, WineGDK patch notes]

**Two sign-ins, same account.**
1. The Store sign-in (Xodus, §1.2).
2. For online play: a **launcher-side Microsoft device-code flow** at `login.live.com/oauth20_connect.srf`.
   - `client_id=0000000048183522` is Minecraft for Android's client ID (title 1739947436 per `gophertunnel/minecraft/auth/xbox.go`).
   - Scope: `service::user.auth.xboxlive.com::MBI_SSL`.
   - The refresh token is kept in `<data>/msa/token.json` (mode 0600).

**At each PLAY, the launcher mints all the tokens on the host.**
- It refreshes the MSA token and runs the whole Xbox chain itself in host Python (`cryptography`). The comment says this is because "Azure TCP-RSTs every *.auth.xboxlive.com / sisu call made through Wine's GnuTLS".
- `device.auth.xboxlive.com`: with an EC P-256 proof key, `DeviceType "Win32"`.
- `user.auth.xboxlive.com`: with the access token (`t=<access token>`).
- `sisu.xboxlive.com/authorize` with `AppId 0000000048183522`, falling back to plain XSTS, for these relying parties:
  - `http://xboxlive.com`;
  - `https://b980a380.minecraft.playfabapi.com/`;
  - `https://multiplayer.minecraft.net/`;
  - `https://pocket.realms.minecraft.net/`;
  - `http://licensing.xboxlive.com`.
- Plus a user-only XSTS token for achievements.

**Where the tokens end up.**
- **`device.json`:** everything goes to `<data>/winegdk-preauth/device.json` (0600), **including the device private key** as a Windows key blob. The environment variable `WINEGDK_PREAUTH_DEVICE=Z:\…\device.json` makes the game's Xbox runtime (`xgameruntime`) use it instead of making its own HTTP calls.
- **The Wine registry:** the **MSA refresh token** is also written into `HKLM\Software\Wine\WineGDK\RefreshToken` in plain-text `system.reg`, where WineGDK's sign-in code (XUser) reads it.
- **Consequence:** any code running in the Wine prefix can read the long-lived refresh token, the device key and all XSTS tokens. That includes the game, an exploited add-on or server bug, and injected DLLs.

**The launcher uses the tokens itself too.** It sends Xbox presence heartbeats to `userpresence.xboxlive.com` every 240 s and reads `peoplehub`.

**Known gaps, in BoL's own words.**
- Achievements never unlock.
- The Xbox services layer (XSAPI) "does not come up under Wine"; presence is written by the launcher.
- An attempt to unlock Profiles/Skins in the UI showed they "genuinely need XSAPI social/persona, which does not work under Wine" (`fixups.py` comment; may be dated). The CHANGELOG shows the Dressing Room opening.
- **Nowhere is a cape claim shown working.**

### 1.5 Processes [code: `launch.py` builds the command outermost-first]

```
bedrock-on-linux (python3; PySide6 GUI or CLI)             your UID, no sandbox
├─ threads: Xbox presence heartbeat (HTTPS), Discord IPC, GitHub update check
├─ xodus-cli login (own session, HOME=<data>/xodus-home)
│   ├─ WebKitWebProcess, WebKitNetworkProcess
│   └─ pkexec cat /sys/firmware/dmi/entries/1-0/raw        root; first registration only
├─ xodus-cli streaming <product|url> <dir>  (in a pty)      download + decrypt
└─ PLAY: systemd-run --user --scope -p MemorySwapMax=0 --   (if delegated cgroup works)
     [gamescope … --]                                        (opt-in BOL_GAMESCOPE)
      xodus-cli run <game_dir> <data>/run/xodus-launch-wrapper.py   licence + memfd decrypt
       └exec python3 wrapper: copies memfds → /dev/shm/bol-* (0600), loader unlinks on open
         └exec python3 umu-run <exe> → Steam Linux Runtime / pressure-vessel (bwrap)
            └ proton → wineserver, wine64: Minecraft.Windows.exe, services.exe,
              winedevice.exe, explorer.exe, msedgewebview2.exe, MicrosoftEdgeUpdate.exe
              (+ optional injector.exe / fullscreen-nudge.exe / desktop-bridge.exe)
```

### 1.6 What it writes where [code]

Default data directory `<data>` = `~/.local/share/bedrock-on-linux`, overridable with `BOL_HOME` or a pointer file at `~/.config/bedrock-on-linux/install_location`.

**Inside `<data>`:**

| Path | What it holds |
|---|---|
| `proton/GDK-Proton-xuser` | the engine |
| `umu/` | `umu-run` and the downloaded Steam runtime |
| `steamcompat/` | fallback Steam compatibility directory |
| `compatdata/pfx` | the Wine prefix; worlds under `drive_c/users/steamuser/AppData/Roaming/Minecraft Bedrock`; the registry holds the refresh token |
| `games/<build>/` | decrypted files, encrypted executable, `.xodus-streaming.msixvc` |
| `content` | symlink to the active build |
| `msa/token.json` | MSA refresh token |
| `winegdk-preauth/` | `device.json`, `device-key.pem`, `device-id.txt`, `.account-epoch` |
| `xodus/` | `xodus-cli` binary |
| `xodus-home/` | `.xodus-keyring.ron` and the WebKit state |
| `xodus-webview/`, `xodus-xcurl/` | bundled WebKitGTK runtime and OpenSSL XCurl set |
| `cache/` | archives, `cacert.pem`, `gdk-links.json`, `injector.exe`, auto-inject DLLs |
| `logs/` | `minecraft.log`, `store-login.log`, `store-download.log`, … |
| `run/` | `xodus-launch-wrapper.py` |
| `settings.json`, `profiles/`, `optiscaler/` | settings, isolated profiles, optional OptiScaler |

**Outside `<data>`:**
- `/dev/shm/bol-*` (under Flatpak: `$XDG_RUNTIME_DIR/bedrock-on-linux/`) — the decrypted executable, for milliseconds.
- `~/.local/share/applications/bedrock-on-linux-*.desktop` and Steam shortcuts (only when asked).
- `~/.local/lib/python3*/site-packages`, via **`pip install --user [--break-system-packages]`** when `cryptography`, PySide6 or python-xlib are missing (`bol/deps.py`).
- It removes an empty `~/.steam/steam` and migrates `~/.xodus-keyring.ron`.

**In the game directory:** `XCurl.dll` (+ `.bol-orig` backups), `libHttpClient.GDK.dll`, `etc/ssl/certs/ca-bundle.crt` (downloaded from curl.se), patched HBUI bundles, `splashes.json`, partner images, and optional BetterRTX materials.

### 1.7 What it downloads at run time [code]

| What | From | Integrity |
|---|---|---|
| Engine, xodus-cli, WebKitGTK runtime, OpenSSL XCurl set | github.com/Wyze3306/BedrockOnLinux releases | SHA-256 pinned |
| Launcher self-update | same releases | `SHA256SUMS` from the same release; the published attestations are **not** checked by `update.py` |
| umu-launcher | github.com/Open-Wine-Components/umu-launcher 1.4.3 | pinned |
| Steam Linux Runtime sniper | repo.steampowered.com (fetched by umu) | `SHA256SUMS` from the same host |
| GDK replacement DLLs | github.com/minecraft-linux/mcpelauncher-gdk-dependencies | pinned |
| MinGW curl | mirror.msys2.org | pinned |
| `cacert.pem` → game's CA bundle | curl.se/ca/cacert.pem | **not pinned** |
| Build index | raw.githubusercontent.com (GdkLinks) | entries validated, not pinned |
| The game | displaycatalog.mp.microsoft.com, packagespc.xboxlive.com, assets*.xboxlive.com (HTTP), licensing.mp.microsoft.com (every start) | content keys |
| Sign-in | login.live.com, *.auth.xboxlive.com, sisu.xboxlive.com | — |
| Changelogs | feedback.minecraft.net Zendesk API | — |
| Python dependencies | PyPI via pip | versions pinned, no hashes |
| Optional | OptiScaler nightly (GitHub, pinned); BetterRTX presets (bedrock.graphics/api, **unpinned**); any DLL URL the user types for auto-injection (**unpinned**) | — |

### 1.8 What it isolates

**Effectively nothing, in the .deb, .rpm, AppImage, .pyz and Nix builds.**
- The game runs as you.
- pressure-vessel's documented default is `--share-home` (https://gitlab.steamos.cloud/steamrt/steam-runtime-tools/-/blob/main/pressure-vessel/wrap.1.md). On top of that is Wine's `Z:` drive.
- The game is a client of the session X display unless gamescope is opted in.
- It gets the full PulseAudio socket and full network.

**Flatpak build.** No home access (its own data, plus read-only access to the legacy data path). But it has `--share=network`, `--device=all`, `--allow=devel` (ptrace), X11 + Wayland + PulseAudio, and the Discord sockets.

**What it does well.**
- Secrets files are 0600 and their directories 0700.
- Tokens are carefully kept out of logs.
- Downloads are pinned; builds are reproducible and attested.
- Tar extraction is done safely.
- Xodus's `HOME` is moved out of `~`.

### 1.9 Files and their roles

**`bol/` modules:**

| Module | Role |
|---|---|
| `__main__.py`, `cli.py` | entry point and command dispatch |
| `gui.py` | PySide6 GUI (4.4k lines) |
| `navigation.py`, `gamepad.py` | controller-driven UI; reads evdev |
| `config.py` | paths, pins, URLs |
| `xodus.py` | Xodus: sign-in, download, encrypted launch wrapper |
| `webview.py` | bundled WebKitGTK for xodus-cli |
| `auth.py` | device-code sign-in, host-side Xbox token chain, registry token |
| `presence.py` | Xbox presence heartbeat |
| `network.py` | network diagnostics |
| `launch.py` | PLAY pipeline |
| `prefix.py` | Wine prefix and umu lifecycle, options, server-list seeding |
| `proton.py` | GDK-Proton download and binary patches |
| `winegdk.py` | engine install and validation |
| `vkd3d.py` | vkd3d build and its manifest pins |
| `fixups.py` | DLL swaps, HBUI patches, cryptbase, stack reserve |
| `gameinput.py` | GameInput MSI/CAB extraction |
| `wine_registry.py` | offline registry writes |
| `winehelper.py` | runs the `.exe` helpers |
| `inject.py` | DLL injection (`CreateRemoteThread`) |
| `fullscreen_fix.py`, `virtual_desktop.py`, `x11.py`, `waylanddrv.py` | windowing |
| `gpus.py`, `gpu_safety.py`, `dgc.py`, `raytracing.py`, `ntsync.py`, `perfcheck.py` | GPU and performance checks |
| `swapguard.py` | systemd scope with `MemorySwapMax=0` |
| `supervision.py`, `leftovers.py` | session end, killing WebView2 leftovers |
| `saves.py`, `content.py`, `packcache.py`, `brarchive.py` | worlds and imports |
| `games.py`, `gamesetup.py` | builds and setup orchestration |
| `profiles.py` | isolated profiles and shortcuts |
| `relocation.py`, `xdg_migration.py` | data moves |
| `update.py` | self-update |
| `deps.py` | pip bootstrap |
| `branding.py` | splashes and partner server |
| `betterrtx.py`, `optiscaler.py` | optional graphics add-ons |
| `discord.py` | Rich Presence over IPC |
| `hostarch.py` | ARM (muvm/FEX) |
| `doctor.py` | health checks |
| `log.py`, `util.py`, `archive.py`, `pe.py`, `engine_lock.py`, `qrcodegen.py` | helpers |
| `hidraw.py` | keeps non-controller HID devices out of Wine |

**Everything else:**

| Path | Role |
|---|---|
| `src/injector.c`, `fullscreen-nudge.c`, `desktop-bridge.c` | sources of the committed `.exe` helpers |
| `src/cryptbase-stub.c`, `src/xcurl-cashim.c` | cryptbase stub; CA shim that also rewrites empty `xuid()` URLs to `/users/me/` |
| `third_party/winegdk-*` | WineGDK patches and source hashes |
| `third_party/xodus/patches` | 11 Xodus fixes |
| `third_party/vkd3d-proton-universal`, `third_party/linux-uapi/ntsync.h`, `xcurl-msys2.lock` | build inputs |
| `scripts/*.sh` | build, package, reproduce, verify |
| `.github/workflows/*` | CI: engine, Xodus, vkd3d, XCurl, app, release, CodeQL, pages |
| `flatpak/*`, `flake.nix` | packaging |
| `site/*` | project website |
| `tests/*` | ~105 pytest files |
| `tools/*` | probes for achievements, DXR, WinHTTP/RTA |
| `docs/BUILD.md`, `docs/BEDROCKMCP.md`, `docs/OPTISCALER.md` | documentation |

### 1.10 Licences of what it bundles or downloads

| Component | Licence |
|---|---|
| BoL | MIT (+ stale notice) |
| xodus-cli | GPL-3.0 (source tarball published beside the binary) |
| Wine / WineGDK | LGPL-2.1+; the WineGDK author's own code CC0 per its README |
| GDK-Proton base | Proton/GE scripts and bundled components under their own licences; the repository declares none |
| DXVK | zlib |
| vkd3d-proton | LGPL-2.1 |
| umu-launcher | GPL-3.0 |
| Steam Linux Runtime | per-package licences (Valve container of distribution packages) |
| mcpelauncher-gdk-dependencies | MIT |
| XCurl set | curl licence, OpenSSL Apache-2.0, MIT/BSD/zlib libraries, LGPL libidn2/libunistring/gettext/libiconv; BoL shims MIT |
| cacert.pem | MPL-2.0 |
| WebKitGTK runtime | LGPL/BSD (Debian packages) |
| Python stack | CPython PSF, PySide6-Essentials/shiboken6 LGPL-3.0, cryptography Apache-2.0/BSD, python-xlib LGPL-2.1+, cffi MIT, pycparser BSD, certifi MPL-2.0, six MIT |
| GameInput, WebView2, the game | Microsoft proprietary (from the game package) |
| OptiScaler, BetterRTX presets | not checked |

---

## 2. The alternatives

### 2.1 mcpelauncher: the Android build [code, GitHub API, versiondb]

**Project and packaging.**
- minecraft-linux/mcpelauncher-manifest: GPL-3.0, since 2018, ~1.7k stars, 1222 open issues.
- Latest release **v1.8.7, 2026-10-29**.
- Flathub `io.mrarm.mcpelauncher`: KDE runtime 6.10 plus the QtWebEngine base app. Its permissions are network, X11/Wayland, PulseAudio, `--device=all`, IPC, and no home directory.
- AUR `mcpelauncher-linux` and `mcpelauncher-ui` are source builds.
- The UI (`mcpelauncher-ui-qt`) is GPL-3.0. The client repository has no licence file of its own.

**How the game files are obtained.**
- The UI signs in to Google (`playdl-signin-ui-qt`, a QtWebEngine window).
- It calls Google Play's **delivery** service for `com.mojang.minecraftpe`. Ownership counts as verified when Play returns a download URL (`googleplayapi.cpp:176` `validateLicense`).
- It downloads the APK splits for your CPU (x86_64 exists) and extracts libraries and assets into its `versions/` directory.
- **No decryption is involved**: APKs are plain archives.
- The Microsoft or Java/PC bundle does not count; the project's site says: "Ownership on the Google Play Store, not any other Store, is a hard requirement."

**Licence handling** (the point to put to the user).
- If ownership cannot be verified, the UI starts the client with `--free-only`, which refuses every non-trial build (`main.cpp:591`).
- Otherwise the client answers the game's own Google Play licence query with true: `StoreFactory::hasVerifiedGooglePlayStoreLicense = true` (`src/jni/store.cpp:87`).
- In **32-bit x86 builds only**, it also patches the UI middleware's licence-key check (`cohtml::VerifiyLicenseKey` → true; `hbui_patch.cpp`, inside `#ifdef __i386__`).
- The UI's ownership check is a CMake option (`LAUNCHER_ENABLE_GOOGLE_PLAY_LICENCE_CHECK`, default ON).
- In-app purchases always fail (`Store::purchase`), so no real-money purchases are possible.

**Xbox Live.**
- The game's own sign-in library (XAL) does the sign-in, inside the game.
- For the sign-in page, the client execs an external program **`mcpelauncher-webview <starturl> <endurlprefix>`** and reads the final URL from its stdout (`xal_webview_qt.cpp`). A CLI build instead asks the user to paste the final URL. **This is a clean integration point** (§5).
- The game's HTTP goes through the **host's libcurl** with no CA override (`lib_http_client.cpp`), and the game's own certificate checks are stubbed (`cert_manager.cpp`). So its TLS trust is the system CA store, which inside bastion's sandbox is the synthetic `/etc`.
- Known limits from the README: you cannot receive Xbox game invites, and others "may be unable to join worlds hosted by this launcher".

**Bedrock versions supported.**
- The version database on `master` (used by v1.8.7) lists x86_64 builds up to **1.26.62.1** (release) and **1.26.70.24** (beta) as of 2026-11-14.
- The website ("up to 1.21.5x") and the README table ("1.21.73") are stale.

**Other.**
- It loads native mods from `<data>/mods` (not with `--free-only`).
- It maps Android's `/data/data/com.mojang.minecraftpe` to its data directory.

### 2.2 Other ways to run Bedrock on Linux

| Option | Game files | DRM / licence | Maturity | Licence |
|---|---|---|---|---|
| **BedrockOnLinux** (GDK/Wine) | Xodus, from your Microsoft Store entitlement | circumvention by decryption (§1.2) | 6 months old, very active, breaks when Microsoft changes things | MIT + GPL-3.0 Xodus |
| **mcpelauncher** (Android) | Google Play delivery for your Google Play purchase | game's licence query stubbed after the UI's Play ownership check; nothing decrypted | 8 years, "unstable unofficial" | GPL-3.0 |
| **Waydroid** + GApps | the real Play Store | genuine licence check | maintained (12.5k stars, pushed 2026-10) | GPL-3.0; Google apps proprietary |
| **Xodus's own stack** (xodus-service, xgameruntime, wine) | as BoL | as BoL | early: "When can I play my Minecraft Bedrock? … soon™" | GPL-3.0 / LGPL-2.1 |
| **Windows** (real PC, dual boot, VM), console, phone | Microsoft Store / Xbox app / other stores | genuine | — | — |
| **Xbox Cloud Gaming** | — | genuine | I could **not confirm** Minecraft (Bedrock) is in the catalogue | — |

Notes on the rows:
- **Waydroid:** a 2024 guide (pancakes.codeberg.page) reports it working with cross-play, limited to 32 chunks and no RTX. It needs Google device certification and binder in the host kernel, and runs a root LXC container service. Xbox sign-in on Waydroid in 2026 was **not verified**.
- **Windows VM:** without GPU passthrough it would rely on software rendering; performance is untested.

---

## 3. Bedrock cape events (2023–2026)

### 3.1 The capes and what each one required

| Cape | When | What the player had to do | Needs Bedrock? | Reaches Java? | Source |
|---|---|---|---|---|---|
| Cherry Blossom | Oct 2023 | Java: vote; Bedrock: join the Mob Vote event server | yes, for the Bedrock reward | yes | [wiki] |
| 15th Anniversary | 2024-05-17 → 06-21 | click "claim" on minecraft.net | no | yes | [wiki] |
| Purple Heart / Follower's | May–Jun 2024 | Twitch/TikTok watch time → code → minecraft.net/redeem | no | yes | [wiki] |
| MCC 15th Year | 2024-07-22 → 08-05 | 15 challenges on the MCC event server | **yes** | yes | [wiki] |
| Home / Menace | 2025-03-18 → 04-06 | watch time → code | no | yes | [wiki] |
| **Yearn** | 2025-03-25 → 04-14 | all 6 quests on the "A Minecraft Movie" Live Event server | **yes** | yes | [wiki] |
| Founder's / Pan | 2025-05-08 | Bedrock-only capes, granted to Java players | — | yes | [wiki] |
| **Common** | 2025-05-06 → **2026-05-05** | sign in with a Microsoft account on any Bedrock platform; granted automatically | **yes** | yes | [official] |
| **Copper** | 2025-10-13 → 10-20 | place a copper golem statue in Bedrock, **or** a watch-time code, **or** a code card | one of three paths | yes | [official] |
| **Zombie Horse** | 2025-12-12 → 12-15 | defeat five mobs with a spear in Survival, Bedrock Edition | **yes** | yes | [official] |
| Moonlight Trail, Crafter, Builder | 2026 | in-person event or watch-time codes | no | yes | [wiki] |
| Aurora | 2026-09-29 → 10-15 (codes until 10-31) | watch Dungeons II streams → code | no | yes | [wiki] / [official] |
| **Hero** (open until **2026-12-31**) | 2026 | play Dungeons I, log into Dungeons II, then "log into Minecraft: Java and/or Bedrock Edition to unlock it there as well" | no | yes | [official] |
| Twisted | 2026 | Dungeons II pre-order bonus | no | yes | [official] |
| Progress Pride | — | the only Bedrock-exclusive cape | yes | no | [wiki] |

Official wording for the key rows:
- **Common Cape** (minecraft.net/en-us/article/new-cape-alert, 2025-05-06): "can be donned on both Bedrock and Java Edition after logging in to the game with your Microsoft account on any Bedrock Edition device"; "As soon as the sign-in flow completes, the cape is automatically granted".
- **Copper Cape** (…/the-copper-cape-quest-is-almost-over): "Place a copper statue in-game (Minecraft: Bedrock Edition only)"; the cape arrives "within 30 days … in the Dressing Room".
- **Zombie Horse Cape** (…/mounts-of-mayhem-challenges): "Reward: Zombie Horse Cape – available both in Bedrock and Java!"
- **Hero Cape** (…/minecraft-dungeons-ii-capes-promos): quoted in the table.

Two smaller sources:
- A tweet search result for the Common Cape (x.com/Minecraft/status/1919800944172400904).
- The wiki: "capes added by live events have been announced for and immediately granted to both editions upon completion of the requirements."

**The pattern for the user:** Mojang keeps gating cross-edition capes behind Bedrock actions — game-drop challenges in Bedrock Survival, Bedrock event servers, Bedrock sign-in. So the need recurs, but irregularly. **No Bedrock-gated promotion was open on 2026-11-14.**

### 3.2 Does a claimed cape reach the Java profile?

**Confirmed.**
- Official pages say these capes are wearable on Java.
- The Java profile API lists owned capes: `GET /minecraft/profile` returns `capes[{id,state,url,alias}]`. `PUT /minecraft/profile/capes/active {capeId}` equips one; `DELETE` hides it. ([wiki] https://minecraft.wiki/w/Mojang_API; already used by `src/auth/minecraft.rs`.)
- The wiki gives the Java aliases, e.g. `Common`, `Copper`, `Zombie Horse`, `Yearn`.

**Second-hand.**
- A Microsoft Q&A moderator (2026-05-31): promotional capes "show up in Bedrock's Dressing Room first", then you "manually select the cape in the Java skin editor"; community reports say up to ~24 h. He cites help.minecraft.net article 31550001045773.
- Search snippets of help.minecraft.net article 37619868918029 ("Activate Promotional Capes and Character Creator Items on Your Minecraft: Java Edition Profile") say: "If you want to activate your cape on your Minecraft: Java Edition profile, you must first sign into Minecraft: Bedrock Edition and claim it."
- **I could not load either help article.** The help site renders client-side, and a further attempt to read its scripts was refused by the permission classifier.

**Inferred, not verified.** That such a cape then appears in `/minecraft/profile` `capes`, so `bastion-mc cape set` can equip it.

### 3.3 What I could not confirm
- Whether a Bedrock sign-in through mcpelauncher (genuine Android client code) or BoL (Android client ID, Win32 device type) triggers sign-in grants. I would expect it does for mcpelauncher; for BoL it is unknown.
- **How Mojang tracks challenge completion** (spear kills, statue placement). It is probably the game's event or telemetry pipelines. Blocking telemetry in an allowlist might prevent the grant, and BoL's engine does not implement Xbox in-game events at all. Unknown.
- The terms page https://www.minecraft.net/en-us/terms/account-sign-in-cape was not read.

---

## 4. Network: what Bedrock needs

### 4.1 Hosts

| Purpose | Hosts | Protocol | Status |
|---|---|---|---|
| Microsoft sign-in (game's XAL token refresh; the sign-in page) | `login.live.com`; page assets on `aadcdn.msauth.net` (official, Education doc); `logincdn`/`acctcdn.msauth.net` and `account.live.com` | HTTPS | [code], [official]; the asset hosts beyond aadcdn are [inferred] |
| Xbox authentication | `user.auth.xboxlive.com`, `device.auth.xboxlive.com`, `title.auth.xboxlive.com`, `xsts.auth.xboxlive.com`, `sisu.xboxlive.com` | HTTPS | [code: BoL, gophertunnel] |
| Xbox services | `*.xboxlive.com`: `peoplehub`, `social`, `userpresence`, `sessiondirectory` (multiplayer sessions), `rta` (WebSocket), `profile`, `privacy`, `achievements`, … | HTTPS/WSS | [code: BoL comments]; the full list is [inferred] |
| Minecraft services | `client.discovery.minecraft-services.net` (service discovery), `authorization.franchise.minecraft-services.net` (Minecraft service token), `signal.franchise.minecraft-services.net` (WebRTC signaling), `bedrock.frontendlegacy.realms.minecraft-services.net` (Realms API); others such as gatherings and store come from discovery under `*.minecraft-services.net` | HTTPS/WSS | [official: Education IT guide], [code: gophertunnel] |
| Server login token chain | `multiplayer.minecraft.net/authentication` | HTTPS | [code] |
| Realms relying party | `pocket.realms.minecraft.net` | HTTPS | [code] |
| PlayFab | `<titleId>.playfabapi.com` (BoL: `b980a380`; the title id comes from discovery) | HTTPS | [code] |
| Marketplace / content | `xforge.xboxlive.com`, `xforgeassets001.xboxlive.com`, `xforgeassets002.xboxlive.com` | HTTPS | [official] |
| Telemetry | `*.events.data.microsoft.com`, `vortex.data.microsoft.com` | HTTPS | [official], [code]; deniable, but see §3.3 |
| Game servers | any `host:port`; defaults UDP **19132** (IPv4) / **19133** (IPv6) | RakNet over UDP | [wiki: Bedrock server defaults] |
| Realms servers | `IP:port` from the Realms join API; `networkProtocol` is `DEFAULT` (RakNet) **or `NETHERNET` / `NETHERNET_JSONRPC`** | UDP | [code: `gophertunnel/minecraft/realms`] |
| Event and Featured servers | from the gatherings service; experiences resolve their address at join | RakNet over UDP | [code] |
| Friends' worlds (NetherNet, i.e. WebRTC) | signaling `wss://signal.franchise.minecraft-services.net`; STUN/TURN `turn.azure.com` / `world.relay.skype.com`, **20.202.0.0/16, TCP 443 + UDP 3478–3481**; then peer-to-peer on ephemeral UDP | WebRTC | [official] for Education; consumer Bedrock [inferred] from gophertunnel/go-nethernet |
| LAN | RakNet broadcast to UDP 19132; NetherNet discovery broadcast **UDP 7551** | UDP | [code: go-nethernet] |
| GDK route only | `licensing.mp.microsoft.com` (every start), `displaycatalog.mp.microsoft.com`, `packagespc.xboxlive.com`, `assets*.xboxlive.com` (HTTP); `MicrosoftEdgeUpdate` (deny) | — | [code] |
| Not needed by Bedrock | `api.minecraftservices.com`, `textures.minecraft.net` (the Java services) | — | — |

### 4.2 What an allowlisting relay would have to permit
- **HTTPS by server name (SNI), without TLS termination:**
  - suffixes `.xboxlive.com`, `.minecraft-services.net`, `.playfabapi.com`;
  - exact names `login.live.com`, `multiplayer.minecraft.net`, `pocket.realms.minecraft.net`.
  - Deny `api.minecraftservices.com` and, by default, telemetry.
- **RakNet over UDP to allowlisted servers.** Names map to the relay's fake IPs exactly as for TCP today, with one host-side socket per flow.
- **What a name-based relay cannot carry:**
  - Realms and event servers whose join address is an **IP literal** inside a TLS response. Like bastion's Java Realms broker, this needs TLS termination of the Realms/gatherings API to learn and rewrite the address.
  - NetherNet friends' worlds and NetherNet Realms: ICE candidates are IP literals, so this needs either unrestricted UDP or a "relay-only" variant that permits only 20.202.0.0/16 and lets ICE fall back to Microsoft's relay (unverified).
  - LAN.

---

## 5. Security design for bastion-mc

### 5.1 What changes in the threat model

**New asset: the account's Xbox identity as held by the Bedrock game.**
- What it consists of:
  - Android route: an MSA refresh token for client `0000000048183522`, scope `service::user.auth.xboxlive.com::MBI_SSL`, long-lived.
  - The device key.
  - XSTS tokens (lifetime "hours", per BoL).
  - The Minecraft service token and the PlayFab session.
- What its holder can do:
  - join any server as you;
  - use Realms, including owner operations if you own a Realm;
  - spend your **Minecoins** on Marketplace items;
  - change your Bedrock persona and cape;
  - act on your Xbox social graph and presence.
- **Likely but unverified:** mint an XSTS token for `rp://api.minecraftservices.com/` and so a Java access token (skin, cape, name). Mojang might refuse that client ID at `login_with_xbox`; it cannot be tested without an account.

**New adversaries.**
- **S:** a hostile Bedrock server or content. Bedrock is native C++ with a scripting engine for add-ons.
- **M:** native mods, through mcpelauncher's mod loader. Disable it.
- **C:** supply chain — the mcpelauncher-client build and the imported game files.

### 5.2 What can and cannot be brokered

**Cannot be brokered.**
- The game's sign-in library (XAL, Android route) lives inside `libminecraftpe.so`. It needs the refresh token to refresh, and the device key to sign every Xbox call.
- XSTS tokens, the Minecraft service token and the server login chain are all presented by the game itself over end-to-end TLS.
- **What follows:** a compromised Bedrock client has the account's Xbox identity until the refresh token is revoked. It can exfiltrate that through any reachable server, Realm or Xbox endpoint. **No allowlist prevents that**; it only narrows the paths.

**Partly possible in the GDK route only.** There, token acquisition is Wine code the launcher controls. A design could pre-mint only short-lived per-relying-party XSTS tokens and **withhold the refresh token**. BoL does not do this (it writes the token into the registry). Whether WineGDK works without it is unverified, and the route has the DRM problem.

**What bastion-mc can enforce.**
1. **Java tokens never exist during a Bedrock session.** Skip the auth step. The sandbox has no keyring (seccomp denies `keyctl`) and no path to the token store (Landlock).
2. **Filesystem, network, display, audio, GPU, cgroup limits and self-confinement** — all existing mechanisms.
3. **Token storage is ephemeral by default.** The game's data directory is scratch, deleted at exit, so a Bedrock token lives only for one session. Worlds can be persisted on opt-in; token stores never.
4. **Validated sign-in bridge** (§5.3).
5. **Detection afterwards.**
   - Run the profile canary after a Bedrock session. A newly owned cape *will* trip it, as expected, and `canary accept` records it.
   - Add a step to the runbook: Microsoft account "sign out everywhere". That revokes the Bedrock token, and bastion's own token too.
   - Extend `scram` to delete the Bedrock data and token stores.

### 5.3 Sandbox profile (mcpelauncher-client in bubblewrap)

**Namespaces and filesystem.**
- Namespaces as for Java: user, pid, ipc, uts, cgroup, net.
- An empty tmpfs `/`; read-only `/usr`.
- A synthetic `/etc` with the player's passwd entry, `resolv.conf` pointing at the launcher's DNS, and the system CA bundle (plus a launcher CA only for WP-B4b).

**Mounts.**

| Mount | Contents | Access |
|---|---|---|
| `/bastion/game` | the imported version directory, bound by the descriptor it was hashed from | read-only |
| `/bastion/data` | the game's data directory; per-session scratch by default | read-write |
| `/bastion/cache` | cache | tmpfs |
| `/bastion/bin/mcpelauncher-webview` | the bridge stub | read-only, executable |

- No `$HOME`.
- Nothing from `/run/user` except the game's own display socket and the audio socket.
- No `mods/` directory.

**Exec rights (Landlock).**
- The `mcpelauncher-client` binary and its private library directory, the dynamic loader and the stub; nothing else.
- The game's Android `.so` files are mapped by mcpelauncher's own linker with `mmap`, which Landlock's execute right does not govern. So the game directory needs read access only.

**Devices.**
- Granted: `/dev/dri` render node (+ its sysfs directory), `null`, `zero`, `urandom`.
- **Not** granted: `/dev/input`, hidraw, `/dev/snd` (unless `audio = "alsa"`).

**seccomp.**
- A new allowlist for this program; the tripwire stays identical.
- Namespace creation stays forbidden. That is why no Chromium (with its own namespace sandbox) can run inside, and why the sign-in page must run outside.
- The allowlist is profiled in log mode on the user's machine with the real game (user-only).

**Display, audio, environment.**
- Display: the Wayland security-context socket, or nested gamescope on X11 (SDL3 supports both); never the session X display.
- Audio: PulseAudio after the existing module-loading check, or none.
- Environment built from scratch.
- Arguments: `-dg /bastion/game -dd /bastion/data -dc /bastion/cache [-f]`.

**Sign-in bridge.**
- The stub sends `(starturl, endurlprefix)` over an inherited socket.
- The launcher accepts only:
  - `https://login.live.com/oauth20_authorize.srf`;
  - `client_id=0000000048183522`;
  - `redirect_uri=ms-xal-0000000048183522://auth`;
  - and an end prefix equal to that redirect.
- It opens the page in bastion's own sandboxed browser helper on a visible window, or shows the URL for the user's browser and accepts the pasted final URL.
- Only the final URL goes back to the game. With PKCE, the code in it is useless without the game's verifier.
- This keeps the password, two-factor steps and cookies out of the game, and stops a hostile game from phishing through the launcher.

**Network.** As in §4.2: SNI allowlist, RakNet per server, LAN and NetherNet off in the first version, and a log-only mode that lists one session's names and UDP destinations so the user can decide.

**GDK variant (only if the user overrides the recommendation).**
- A root-owned Wine/GDK-Proton tree instead of downloads; no umu or pressure-vessel (bwrap *is* the container).
- A Wine prefix per instance with `Z:` and the other drive links removed.
- A socket directory for wineserver, `/dev/ntsync`, Vulkan ICDs.
- The withheld-refresh-token design from §5.2.
- **Plus the per-start decryption step**, which bastion-mc itself would have to perform. That is the circumvention line; I have not planned it.

### 5.4 Residual risks, stated plainly
- **The account's Xbox identity is in the game**, which is hostile by assumption. Any reachable server, Realm or Xbox API can carry it away. Minecoins and Realms are at stake, and possibly the Java profile.
- Path-level policy (blocking purchases, gamertag or privacy changes) needs TLS termination (WP-B4b), and that grows the trusted code base.
- The same-UID memory read of the game (THREAT_MODEL P7) applies unchanged.
- GPU driver attack surface.
- The game's licence query is answered by a stub.
- **Account enforcement:** possible action by Google or Microsoft against unofficial clients is unknown. mcpelauncher-ui holds a **Google credential** outside bastion.
  - Advice: buy Minecraft on Google Play with a **dedicated Google account**. The Xbox identity is separate from it.
  - Optionally sandbox the UI too (WP-B7).
- Denying telemetry may forfeit challenge credit.
- `mcpelauncher-client` is unaudited native code that patches the game.
- If the user plays through mcpelauncher-ui directly, none of this applies.

### 5.5 Wrap or implement: recommendation

**Do not wrap BoL.**
- Its runtime downloads from about ten sources, including pip, user-typed DLL URLs and an unpinned CA bundle.
- It needs pkexec, a nested pressure-vessel bwrap that bastion's seccomp forbids, systemd-run and Discord IPC.
- It hands the game the refresh token and device key.
- It modifies the game, including a third-party server advertisement.
- Its every start requires decryption.
- Putting it in a sandbox as one unit would put a networked launcher inside the allowlist and gain little.

**Do not reimplement it.** That would be circumvention in bastion-mc's own code.

**For the Android route: wrap the installed, root-owned `mcpelauncher-client`.**
- That follows the JDK policy: "never downloads one".
- Keep mcpelauncher-ui outside bastion as the user's Google Play download tool, which is where the ownership check lives.
- Implement bastion-mc's thin parts itself: the instance kind, import into the store, the sandbox profile, the relay extensions, the sign-in bridge, and the account-risk controls.
- Do not build a Google Play client into bastion-mc: it would put a Google master token into the trusted code base, through an unofficial API.

---

## 6. Implementation plan (Android route, if the user accepts §0.3)

### 6.1 Work packages

| WP | Deliverable | Engineers can verify (no account needed) | Only the user can test |
|---|---|---|---|
| **B0 Decision & documentation** (lead) | THREAT_MODEL "Bedrock" section with rows; README section stating the licence substitution plainly; a config gate (e.g. `bedrock.accept_licence_substitution = true`) without which launch refuses; doctor lines | doc review; doctor output; refusal test | — |
| **B1 Runtime discovery** | find `mcpelauncher-client` and its library directory from the installed package; root-owned, no writable path component (as `java.rs` does); refuse Flatpak/AppImage installs; optionally a pinned PKGBUILD later | unit tests on fake trees (ownership, symlinks, writable directories) | installing the AUR package |
| **B2 Import into the store** | `bastion-mc bedrock import` from mcpelauncher-ui's `versions/<v>` (native or Flatpak path); refuses symlinks, hardlinks, devices, escapes and oversize trees; per-file SHA-256 manifest; bound by descriptor; never accepts arbitrary APKs; if APKs are kept, checks the APK signature against a pinned Mojang certificate digest | synthetic trees, tamper and escape tests, a generated-input harness for the metadata parser (project rule) | the real import; pinning the signer from their genuine download |
| **B3 Sandbox profile & `Kind::Bedrock` launch** | the policy for the client (Landlock, seccomp profile, devices, environment, arguments); scratch data directory, persistent worlds opt-in; auth step skipped; GUI protection read-out | a **stand-in client** (EGL/GLES, SDL3 window on nested/Wayland, Pulse, curl HTTPS via the relay, UDP to a stand-in RakNet server, execs the stub); tripwire tests | the real game start; collecting the seccomp log; playability |
| **B4a Relay: Bedrock profile** | SNI suffix allowlist per instance kind; deny list; **UDP RakNet relay per server rule** (a generalisation of `egress/udp.rs`, anchored to a name rule, gameplay-sized rate limits, MTU-discovery sizes); LAN and NetherNet off; log-only discovery mode | decision-table unit tests, a fuzz harness, a stand-in RakNet server (unconnected ping/pong, open-connection request), stand-in HTTPS hosts | real session logs; joining real servers |
| **B4b Realms/gatherings broker** (after B4a) | TLS termination with the launcher CA in the sandbox trust store; rewrites join answers to one-time pass addresses with out-of-game approval, reusing the Java Realms broker; optional path policy (block purchases, `accounts.*`, `api.minecraftservices.com`) | stand-in Realms/gatherings over TLS | checking that the game does not pin certificates; a live Realm or event join |
| **B5 Sign-in bridge** | static `mcpelauncher-webview` stub plus the launcher side: validation, rate limit, browser helper or paste-back, nothing but the final URL returned | protocol tests; refusal tests (foreign host, plain HTTP, other client id, overlong input); a stand-in login page redirecting to `ms-xal-…` | the real Microsoft sign-in, two-factor, passkey |
| **B6 Account-risk controls** | `scram` deletes Bedrock data and token stores; canary prompt after a session ("a new cape is expected after an event"); SCRAM_RUNBOOK gains "sign out everywhere" | scram tests (never a real `scram --yes`) | canary after a real session; `cape list/set` with a Bedrock-claimed cape |
| **B7 (optional) Sandbox for the UI** | runs mcpelauncher-ui in its own bwrap with network only to Google Play hosts (learned in log mode); the Google credential stays in its own directory | stand-in | the Google sign-in and download |

### 6.2 Parallel streams (five, matching the user's agent preference)
1. B1 + B2
2. B3
3. B4a, then B4b
4. B5
5. B0 + B6

Fix two interfaces before work starts: the store and runtime structures (B1/B2 → B3), and the stub's file-descriptor contract (B3 ↔ B5).

### 6.3 Only the user can test
- Google Play ownership and the mcpelauncher-ui download.
- The real game in the sandbox.
- The Microsoft sign-in.
- The Dressing Room showing capes.
- Joining a real server, Realm or event server.
- Whether a challenge registers with telemetry denied or allowed — only when Mojang next runs one.
- That the cape appears in `GET /minecraft/profile` and `cape set` equips it. This also needs bastion's own Java sign-in, which has **never run live** yet.

---

## Sources

**Repositories and documentation:**
- https://github.com/Wyze3306/BedrockOnLinux
- https://github.com/xodus-gaming/xodus
- https://github.com/Weather-OS/WineGDK (README: split notice)
- https://github.com/Weather-OS/GDK-Proton
- https://github.com/xodus-gaming/xgameruntime
- https://github.com/Open-Wine-Components/umu-launcher (`umu/umu_runtime.py` at 1.4.3)
- https://gitlab.steamos.cloud/steamrt/steam-runtime-tools/-/blob/main/pressure-vessel/wrap.1.md
- https://github.com/minecraft-linux/mcpelauncher-manifest
- https://github.com/minecraft-linux/mcpelauncher-client
- https://github.com/minecraft-linux/mcpelauncher-ui-qt (v1.8.7 pins 62fba0d)
- https://github.com/minecraft-linux/mcpelauncher-versiondb
- https://minecraft-linux.github.io/
- https://github.com/flathub/io.mrarm.mcpelauncher
- https://github.com/minecraft-linux/mcpelauncher-gdk-dependencies
- https://github.com/waydroid/waydroid
- https://pancakes.codeberg.page/minecraft-bedrock-linux/index.html
- https://github.com/Sandertv/gophertunnel
- https://github.com/df-mc/go-nethernet

**Official Minecraft and Microsoft pages:**
- https://edusupport.minecraft.net/hc/en-us/articles/4404784181524-URLs-Used-by-Minecraft-Education
- https://edusupport.minecraft.net/hc/en-us/articles/360047118992-FAQ-IT-Admin-Guide
- https://www.minecraft.net/en-us/article/new-cape-alert
- https://www.minecraft.net/en-us/article/the-copper-cape-quest-is-almost-over
- https://www.minecraft.net/en-us/article/mounts-of-mayhem-challenges
- https://www.minecraft.net/en-us/article/minecraft-dungeons-ii-capes-promos
- https://www.microsoft.com/en-us/servicesagreement
- https://www.minecraft.net/en-us/eula (its mod permission covers Java Edition only)
- https://www.law.cornell.edu/uscode/text/17/1201

**Cape history and the Java API:**
- https://minecraft.wiki/w/Cape (and the individual cape pages)
- https://minecraft.wiki/w/Mojang_API

**Second-hand / could not load:**
- https://learn.microsoft.com/en-us/answers/questions/5906810/minecraft-cape-is-not-showing
- https://help.minecraft.net/hc/en-us/articles/37619868918029 — could not render; quoted from a search snippet only.