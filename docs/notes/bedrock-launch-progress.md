# bedrock-launch: hand-over (launch package, round 5)

Resume from here if a session was interrupted. The user text, threat-model rows, what was
verified and what is open: `docs/notes/bedrock-launch.md`.

## State: done, verified, committed on `work`

- `sandbox/policy.rs`: `SeccompPolicy.profile` (`Jvm` default, `BedrockClient`); the six
  existing construction sites keep the JVM's (`..Default::default()`).
- `sandbox/seccomp.rs`: `BEDROCK_ALLOW`, `BEDROCK_ALWAYS_DENIED`, `build_bedrock` (log mode
  keeps the tripwire and the always-refused filter); unit tests.
- `sandbox/mod.rs`: `Mount::BindFd` (read-write bind of an O_PATH descriptor).
- `config.rs` `InstanceConfig.kind` / `.bedrock` (+ validation call); `instance.rs`
  `Kind::Bedrock`, `create_bedrock`, `load`; `bedrock/launch/instance.rs`.
- `cli.rs`: `instance new <id> --bedrock <version>`; `install`, `profile …`, `pack …` refuse a
  Bedrock instance (`instance::java_only`); `instance list` shows it; help line.
- `bedrock/launch/profile.rs`: data split (per-session data root, kept dirs, instance lock,
  sweep of earlier sessions, `session_dirs` for scram), stub mounts, the profile.
- `bedrock/launch.rs`: the launch (dispatch from `launch::launch`), `--demo` accepted with a
  note, `--burner/--join/--repair` refused, integration points `NetworkAdditions` /
  `network_additions` / `relay_policy` (network) and `SignIn` (sign-in).
- `launch.rs`: dispatch block; `prepare` refuses Bedrock (install); `Log`, `relay_output`,
  `shields` are `pub(crate)`.
- `[bedrock] extra_syscalls` (+ `examples/config.toml`).
- Tests: `tests/bedrock_launch.rs` (+ `tests/support/bedrock_standin.rs`, compiled by the test
  with rustc), `tests/bedrock_sandbox.rs`, `tests/bedrock_cli.rs`, unit tests in the modules.
- Notes: `docs/notes/bedrock-launch.md`.

## How to re-run what matters

```sh
cd target/agents/r5-launch
E="env -u DBUS_SESSION_BUS_ADDRESS -u WAYLAND_DISPLAY DISPLAY=:77 XAUTHORITY=/dev/null \
   BASTION_TEST_RUNTIME=/run/user/1000/bmc-r5launch XDG_CONFIG_HOME=$PWD/target/xdg/config \
   XDG_DATA_HOME=$PWD/target/xdg/data XDG_CACHE_HOME=$PWD/target/xdg/cache XDG_STATE_HOME=$PWD/target/xdg/state"
$E cargo test --lib bedrock
$E cargo test --test bedrock_launch --test bedrock_sandbox --test bedrock_cli
BASTION_TEST_VERBOSE=1 $E cargo test --test bedrock_launch -- --nocapture   # every probe line
clippy-driver --edition 2024 -o target/standin-try/standin-clippy tests/support/bedrock_standin.rs
```

## Next (for whoever continues)

- Integration with the sign-in, network and store packages (see "Open items" in the notes).
- The front end: Play for a `bedrock` instance should not need a Java sign-in or demo mode.
