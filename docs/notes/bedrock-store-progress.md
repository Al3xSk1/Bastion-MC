# Bedrock store package: hand-over

Owner: store package (round 5). Clone: `target/agents/r5-store`, branch `work`.
Notes for the integrator and the user documentation: `bedrock-store.md`.

## State: done, nothing in progress

- [x] Research. Sources read: client `CMakeLists.txt`, `main.cpp`, `xal_webview_qt.cpp`
      (in `target/research/`); fetched read-only copies (in this clone's `target/r5-research/`,
      untracked) of mcpelauncher-common `path_helper.cpp`, file-util `EnvPathUtil.cpp`,
      mcpelauncher-core `minecraft_utils.cpp`, mcpelauncher-extract `minecraft_extract_utils.h`
      and `zip_extractor.cpp`, mcpelauncher-apkinfo `apkinfo.cpp`, axml-parser, the AUR
      PKGBUILDs (mcpelauncher-linux, mcpelauncher-ui 1.8.7), the upstream AppImage build script
      (`-DXAL_WEBVIEW_QT_PATH=.`) and the deb/rpm workflow.
- [x] `bedrock::runtime`: `discover`, `describe`, `Client::installed_webviews` (+ 14 tests).
- [x] `bedrock::store`: `import`, `open`, `verify`, `list`, `remove`, `ui_version_dirs`,
      `listing_lines`, `instance_version`, `write_stand_in_version`; `store/manifest.rs`,
      `store/axml.rs`, `store/elf.rs` (+ 19 store tests, 10 module unit tests).
- [x] CLI block (`bedrock client|import|versions [--verify]|remove`), `bedrock_store` harness,
      robustness test, `tests/bedrock_store.rs` (3 end-to-end tests).
- [x] Notes; fmt, clippy (no warnings), `cargo test --workspace`, `cargo deny`.

## How to re-run what was run

```sh
C=/home/ski/RustroverProjects/bastion-mc/target/agents/r5-store; T=$C/target
cd $C
cargo test --lib bedrock                      # runtime, store, manifest, axml, elf
cargo test --test bedrock_store               # the CLI end to end (isolated XDG + HOME)
BASTION_FUZZ_ITERATIONS=2000000 BASTION_FUZZ_SEED=7 cargo test --test robustness bedrock
env -u DISPLAY -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS -u XAUTHORITY \
  XDG_CONFIG_HOME=$T/xdg-test/config XDG_DATA_HOME=$T/xdg-test/data \
  XDG_CACHE_HOME=$T/xdg-test/cache XDG_STATE_HOME=$T/xdg-test/state cargo test --workspace
/home/ski/RustroverProjects/bastion-mc/target/tools/bin/cargo-deny deny --offline --locked check
```

SHA-256 speed (the figure in `store.rs`): a scratch crate in `target/r5-scratch/shabench`
(sha2 =0.11.1, `cargo run --release --offline`): 2180 MiB/s; debug build 77 MiB/s.

## If this is picked up again

Nothing is half-done. Possible next steps are the "Open" list in `bedrock-store.md` §5 (real
version measurements by the user, `--events` for `bedrock`, parallel verification).
