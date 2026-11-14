# Fuzzing

Every parser that reads bytes the launcher does not control has a harness in
`src/fuzzing.rs`: the settings sanitisers (NBT, `options.txt`, text, JSON), the TLS
ClientHello reader, the DNS responder, the credential broker's request classification and
join body, the Xauthority reducer, the Wayland event reader, Mojang's profile reply and
skin PNG files, the zip directory reader behind `pack import`, and the companion channel's
line reader — and, although it parses nothing, the datagram relay's decision table (`udp`:
scripts of game connections, datagrams and time). Each harness also checks properties (a
sanitiser's output is a fixed point; a forwarded join is byte for byte the broker's own
object; an archive the strict zip reader accepts is listed identically by a second zip
implementation; a datagram is relayed only to the address of an open game connection under
a rule that names the port; …), not only "does not crash".

Two ways to run them:

**Every test run, stable toolchain** — `tests/robustness.rs` feeds each harness its valid
example inputs, every prefix of them, and a few thousand mutations:

    cargo test --test robustness
    BASTION_FUZZ_ITERATIONS=300000 BASTION_FUZZ_SEED=7 cargo test --release --test robustness

**Coverage-guided, nightly toolchain** — the targets in this directory:

    rustup toolchain install nightly
    cargo install cargo-fuzz
    cargo +nightly fuzz list
    cargo +nightly fuzz run persist -- -max_len=4096 -max_total_time=600
    cargo +nightly fuzz run broker  -- -max_len=4096 -max_total_time=600

A crash is written to `fuzz/artifacts/<target>/`; add the minimised input as a case to the
unit tests of the parser concerned (that is where regressions are kept, not here).

The log4j folding of the graphical front end has its own generated-input test in
`gui/src/console.rs` (the front end is not a dependency of this crate).

State on 2026-11-13: the stable suite passes with 300 000 mutations per harness. The
libFuzzer targets compile but have **not** been run: the development machine has only the
stable toolchain.
