# Bedrock network — the network package of round 5

Branch `work` in `target/agents/r5-net`. What a Bedrock session may reach, how RakNet reaches the
servers the user listed, how Realms and event-server joins are brokered and approved outside the
game, and what is refused and said. The code: `src/bedrock/net.rs` (the plan and the entry point),
`src/egress/bedrock.rs` (the profile's rules), `src/egress/raknet.rs` (RakNet's offline messages),
`src/egress/udp.rs` (flows anchored to server rules, the join pass), `src/egress/services.rs`
(TLS termination of Realms and the gatherings service). Tests: `tests/bedrock_net.rs`, the unit
tests of those modules, three harnesses in `src/fuzzing.rs`.

**Honest limits, first.** There is no `mcpelauncher-client`, no game file, no Google Play purchase
and no Microsoft account on this machine, and none may be installed. Nothing here has met the real
game, a real Bedrock server, the real Realms API or the real gatherings service. Every behaviour
below was verified against stand-ins written for the tests (section 3 says which, and what was
not verified). The endpoints, paths and JSON shapes of Realms and the gatherings service come from
reading gophertunnel (commit e2251a6, a third-party Go client), not from observing the game.

---

## 1. User documentation (for the README and docs/BEDROCK.md)

### What a Bedrock session can reach

A Bedrock session always uses the filtered network: the game runs in a network namespace with no
route anywhere, and everything it reaches goes through the launcher.

- **HTTPS, by name only**, to:
  - exactly `login.live.com`, `multiplayer.minecraft.net` and `pocket.realms.minecraft.net`;
  - anything below `.xboxlive.com`, `.minecraft-services.net` and `.playfabapi.com` (a suffix
    matches sub-domains only: `user.auth.xboxlive.com`, never `xboxlive.com` or
    `evilxboxlive.com`);
  - Microsoft's telemetry (`vortex.data.microsoft.com`, below `.events.data.microsoft.com`) —
    only with `[bedrock] telemetry = true`, the default (whether a challenge cape registers
    without telemetry is not known).
  - Never, whatever a suffix says: the Java services (`api.minecraftservices.com`,
    `sessionserver.mojang.com`, `textures.minecraft.net`, the Java Realms hosts) and NetherNet's
    signalling (`signal.franchise.minecraft-services.net`).
  The TLS connection must name exactly the host that was looked up; it passes through untouched
  (the launcher does not see inside), except for the four hosts of Realms and event-server joins
  (below).
- **Bedrock servers, over RakNet (UDP)**, that you list — nothing else. There is no TCP to a game
  server in Bedrock.
- **Realms and event servers**, one join at a time, each approved by you outside the game.
- **Not in this version:** LAN games (they are found by broadcast, which has no way out of the
  sandbox), NetherNet (WebRTC: friends' worlds and NetherNet Realms), and servers on a private or
  loopback address (a LAN server, a server on this computer).

### Listing servers

```toml
[bedrock]
servers = [
    { host = "play.example.org" },              # port 19132, Bedrock's default
    { host = "mc.example.net", port = 19133 },
    { host = "203.0.113.9", port = 25000 },     # an IPv4 address works too
]
```

An instance adds its own in its `instance.toml`, under `[bedrock] servers`, in the same form.

- The name is looked up by the launcher once per session (it retries a failed lookup a few
  times, for about eight minutes, then gives up — the log says each outcome); the game is given a
  synthetic address, and its datagrams go to the address the launcher found and nowhere else.
- Ports from 1024 up; at most 64 servers; at most 16 different UDP ports in all; no IPv6
  addresses (a name may resolve to IPv6); no private, loopback or link-local addresses (the
  launch log and each refused datagram say so).
- Only listed servers can be reached at all: a name that is not listed does not resolve inside
  the sandbox, and the log says what to add. (How the game shows such a server was not seen: no
  game was run.)

### Realms and event servers

`[realms] approve = "code"` brokers joins; `"off"` (the default) refuses them, and the launch log
says so. (`"totp"` acts as `"code"` in a Bedrock session: the TOTP secret lives in the token store,
which a Bedrock session never opens.)

1. You choose a Realm, or an event server, in the game.
2. The launcher asks Realms (or the gatherings service) where its server is, and **holds the
   answer**. The game is told to wait.
3. The launcher shows, outside the game — in the terminal, or in the front end — the Realm's
   name (or the event's), the server's address, and a six-digit code.
4. You answer with that code (the terminal, or the front end's prompt). `cancel` cancels.
5. The game is given the address of the **join pass** (`127.127.255.253:19132`) instead of the
   server's. Through it the game reaches that one server: one join, at most four connections, all
   within thirty seconds of the first.

If the game stops waiting before you answer, answer anyway and join again: an approved join is
handed to the game at its next request for that join (until you cancel it), and the game must
then use the pass within two minutes. Approve only a join you started: the prompt shows where the
game would go; it cannot know whether you wanted to go there. Limits: an unanswered prompt lives
two minutes and takes three tries; one prompt at a time; twenty per session.

A Realm reached over NetherNet is refused, and the game is told why.

### Reading the log

Each refusal is logged once, phrased so that you know what to add:

```
egress: DENY  name lookup "hive.example.org" (not allowlisted for Bedrock; if it is a server you want to join, add { host = "hive.example.org" } (and its port, if it is not 19132) to [bedrock] servers in config.toml or to the instance's [bedrock] servers)
egress: DENY  udp play.example.org:19133 (this server is listed with another port: list it with this port as well in [bedrock] servers)
egress: DENY  name lookup "signal.franchise.minecraft-services.net" (NetherNet signalling: friends' worlds and NetherNet Realms (WebRTC) are not relayed in this version)
egress: ALLOW udp play.example.org:19132 → 203.0.113.5:19132 (flow 1 from 127.0.0.1:41000; RakNet ping (server list); Bedrock server play.example.org:19132)
```

A server typed into the game as an IP address that is not listed never reaches the launcher (the
game's datagrams have no route), so there is nothing to log for it: list it.

### With `network.launcher_proxy`

Through the proxy (which also resolves the names): the game's HTTPS that the relay passes through
and the launcher's requests to Realms and the gatherings service. Not through it: RakNet to
servers and approved joins, and the lookups of the servers' names (`bedrock::net::describe_proxy`
writes this into the launch log).

---

## 2. Threat model (rows for docs/THREAT_MODEL.md, Bedrock network)

| ID | Threat | What stops it | What remains |
|---|---|---|---|
| BN1 | The game reaches other HTTPS hosts | Namespace without a route; the resolver answers only listed names (exact, suffix, minus the deny list) with synthetic addresses; the ClientHello must name the looked-up host; Landlock: TCP to 443 only | Everything below `.xboxlive.com`, `.minecraft-services.net`, `.playfabapi.com` is reachable — services that hold the account's Xbox identity, through which a hostile game can exfiltrate (research §5.2). No allowlist prevents that. |
| BN2 | The game sends UDP elsewhere | No route; datagrams reach only the relay's sockets (Landlock ABI 10: only DNS, the listed ports and the pass's); a flow goes only to the address resolved for a listed server or to an approved join; a flow opens only with a RakNet ping or connection request; 32 flows, 1472-byte datagrams, 500/s to and 4000/s from a server per flow, 30 s idle | A listed server receives whatever the game sends within those limits. Without Landlock ABI 10 the game may send to any port inside its namespace — where the relay's sockets are the only listeners. |
| BN3 | A poisoned name lookup turns the relay against the LAN | Non-public addresses refused for Bedrock servers and approved joins; one lookup per name per session, before any datagram is relayed | — |
| BN4 | The game joins a Realm or event server without the user (an attacker's Realm, say) | Join answers held; approval outside the game, by the code shown with that prompt, typed into a trusted input the game cannot reach; the pass leads to that server only, one join (four flows within 30 s) | The user must read the prompt. A game can raise up to 20 prompts per session (a nuisance). In `totp` the code mode applies. |
| BN5 | A bug in the TLS termination | Four hosts only; rustls (TLS 1.3) and hyper; bodies bounded and budgeted; no credential held; join answers parsed strictly (size, depth, fields), fuzzed | The broker holds the game's Realms and Minecraft-services tokens in memory while it forwards them (never logged); a compromised launcher could use them — the game holds them anyway. |
| BN6 | The per-launch authority is used against other hosts | Name constraints: the four hosts; created in memory per launch, never written; only in the sandbox's trust store | Name constraints also admit names *below* a permitted host (none is issued, none exists). |
| BN7 | Tokens in the log | Header values, bodies and query strings are never logged; upstream error lines are stripped of the query | — |
| BN8 | NetherNet or LAN around the relay | NetherNet signalling denied at the name; NETHERNET join answers refused with the reason; broadcasts have no route | — |
| BN9 | The confined launcher | The relay starts after step 9; it needs TCP 443 (in the confinement) and name lookups; UDP is not restricted by the confinement | A compromised launcher can send UDP anywhere (as with voice chat). |
| BN10 | Supply chain of the client's HTTP stack (adversary C) | — (finding for the risk package) | mcpelauncher's manifest builds curl 8.21.0 from curl.se **without a checksum** (`ext/curl.cmake`, `USE_OWN_CURL=ON` by default), and the AUR PKGBUILD (mcpelauncher-linux 1.8.7-1) keeps that default: the client's HTTP stack is a static curl fetched at build time. |

---

## 3. What was verified, by running, against what — and what was not

### Verified on this machine (kernel 7.2.14, Landlock ABI 10, bubblewrap)

- **Unit tests** (in `cargo test --workspace`): the SNI rules (suffixes match sub-domains only and
  label-exactly; the deny list wins; malformed lists refused); plan validation (every refused
  entry named); routing (no TCP game servers, ports 443 only; the namespace, Landlock and
  confinement ports); the RakNet parser (exact lengths, the magic, zero padding, MTU 1492/1200/576
  and nothing larger opening a flow, round trips); the relay's table for Bedrock (flows to the
  resolved address only, the RakNet opening check, private addresses refused as LAN, the pass
  spending nothing on refused datagrams, the limits); the service broker (join classification in
  any spelling of the letters, strict Realms and gatherings answers, NetherNet/host name/unknown
  protocol refusals, rewrite to the pass, header forwarding, the authority's name constraints
  with rustls's verifier, the four-flow grant); the entry point (`prepare` with and without
  approvals, `totp` → code, `answer_join`).
- **End to end** (`tests/bedrock_net.rs`, 7 tests, all passing): a sandboxed client (Python for
  RakNet, the system `curl` 8.23.0 with OpenSSL 3.6.5 for HTTPS) in the real namespace with
  Landlock from `Prepared`, the real resolver, relay and broker, a stand-in RakNet server
  (go-raknet's handshake, written for the test with this module's encoders), a stand-in Realms API
  and gatherings service over TLS with a throw-away CA; approvals answered through `answer_join`.
  Shown: ping/pong and both handshake steps reach the listed server (by name and by literal), the
  1464-byte MTU probe and a 1472-byte datagram pass intact, 1473 bytes do not; answers come from
  the address the game sent to; a first datagram that is no RakNet opening opens nothing; UDP to
  an unlisted port fails with `EACCES` (Landlock ABI 10); names that are not listed, Java services
  and NetherNet signalling get NXDOMAIN with the reason in the log; `curl` accepts the per-launch
  authority from the generated trust store (`/etc/ssl/certs/ca-certificates.crt`, Arch's compiled
  default); a Realms join waits (503), is approved by its code, and the game is given the pass,
  whose flows reach exactly the approved server (four, not five); the game's `Authorization`
  header reaches the service unchanged and never appears in the log; NETHERNET, private-address
  and host-name answers are refused with the reason, an address from a non-join endpoint is
  withheld; an event-server join through the gatherings service works the same way; without
  approvals there is no pass and nothing is terminated; and **the whole flow works after the
  launcher has confined itself** (a child process: sandbox, then `confine::apply(Demand::Whole)`,
  then `start` — name lookup, flows, the broker's upstream connection and the pass all work, and
  TCP to an unlisted port fails with `EACCES`).
- **Robustness**: harnesses `raknet` (the parser and the Bedrock relay's decisions),
  `bedrock_realms`, `bedrock_gatherings`: 4000 mutated inputs each in every test run, and
  60 000 each with three seeds in release mode, no failure. The cargo-fuzz targets compile
  (`cargo check` of `fuzz/`); they were not run (cargo-fuzz needs a nightly toolchain).
- **Java unchanged**: every existing test passes; the Java DNS refusal text is byte-identical
  (`(not allowlisted)`); `egress::start` refuses a Bedrock policy and `start_bedrock` a Java one.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets` (no warnings),
  `cargo deny --offline --locked check` (advisories, bans, licenses, sources ok; the one warning
  is an unused licence allowance already in `deny.toml`). No dependency was added.

### Read in source, not run

- **The client's HTTP and TLS** (mcpelauncher-client 7890498): all the game's HTTP and WebSocket
  traffic is `curl_easy_*` in `src/jni/lib_http_client.cpp` and `lib_http_client_websocket.cpp`;
  no `CURLOPT_CAINFO`, `CAPATH`, `SSL_VERIFYPEER/HOST`, `PINNEDPUBLICKEY` or `SSL_CTX_FUNCTION`
  anywhere in the client; the Java-side certificate classes the game may call
  (`src/jni/cert_manager.cpp`: `CertificateFactory`, `TrustManagerFactory`,
  `StrictHostnameVerifier`) are empty stubs. So TLS trust is curl's compiled-in store with
  verification on, and the client pins nothing. **But it is not the system libcurl by default**:
  the manifest builds curl 8.21.0 statically (`USE_OWN_CURL=ON`, `ext/curl.cmake`) and the AUR
  PKGBUILD keeps that; curl 8.21.0's CMake looks for `/etc/ssl/certs/ca-certificates.crt` first
  on Linux (its `CMakeLists.txt`, fetched and read) — one of the paths the sandbox's generated
  trust store is mounted at. A pin inside the game's own native code cannot be ruled out from the
  client's source; it would show as a failed handshake through the broker.
- RakNet facts from go-raknet 0d1fd09 (gophertunnel's pin), fetched to
  `target/r5net-research/go-raknet/` and read, not run.

### Not verified (only the user can, or not on this machine)

- The real client and game in the sandbox: whether its HTTP stack accepts the authority (shown
  with Arch's curl/OpenSSL, the same library family as its static curl and the system OpenSSL,
  not with the client); whether it binds fixed UDP ports (19132/19133 when hosting a world —
  Landlock allows only ephemeral binds, and the relay's own socket holds 19132 when a server or
  the pass uses it; expected effect: no LAN hosting, which is off anyway); whether it pings an
  approved server from one socket and connects from another (why an approval opens up to four
  flows); whether its RakNet first datagram is always a ping or a connection request.
- The real Realms API and gatherings service: their paths and answers (from gophertunnel);
  whether the game polls a join answered 503 (gophertunnel's client does, every 3 s) and what it
  shows meanwhile; the request headers the services need (the broker forwards all but
  hop-by-hop headers, so nothing should be missing); whether they negotiate TLS 1.3 with the
  launcher's client (it insists on 1.3); which gatherings host the game's discovery names (only
  the two known names are terminated — a third would pass through unrewritten, and its joins
  would fail for lack of a route).
- An approved join to an IPv6 address (parsed and tested in units; no IPv6 stand-in run).
- The integration into the launch (the launch package wires it; section 5).
- The opt-in `BASTION_NET_TESTS=1` suites (the Java relay against Mojang's live services, which
  exercise the shared pass-through code): not run — tests may not contact Mojang or Microsoft
  services from this machine. The Java relay's offline tests (`tests/egress.rs`, `voice.rs`,
  `realms.rs`, `broker.rs`) all pass.

---

## 4. Open

- **Wiring** (integrator): the launch calls `prepare` and `start`; the sign-in package's reader
  calls `answer_join` on `answer realms <text>`; the front end's prompt for a Bedrock join must
  say "answer with the code here", not "type it into the phone" (section 5).
- **LAN servers**: a per-server `allow_private` needs a field in the shared `BedrockServer`
  (section 6).
- **Featured servers** come by host name from the gatherings service's discovery answer; the user
  must list them (the refusal line says how). The broker could learn their names from that answer
  and say so; not built.
- **Unlisted literal addresses** typed into the game never reach the launcher (no route), so they
  cannot be reported. A local route for all addresses in the namespace would let the relay see
  (and refuse, and report) them; that is a change to `sandbox::netns`, not made.
- **A new join endpoint** at Realms or the gatherings service would be withheld (502, logged as
  `[SECURITY]`): fail closed, then add it to `services::classify`.
- **Measure with the real client** (user): the items in "Not verified".

---

## 5. For the integrator

### The entry point (two calls)

```rust
// src/bedrock/net.rs
pub struct Options<'a> {
    pub cfg: &'a BedrockConfig,               // [bedrock]
    pub instance_servers: &'a [BedrockServer], // the instance's [bedrock] servers
    pub approve: realms::Approve,             // [realms] approve
    pub proxied: bool,                        // network.launcher_proxy.is_some()
    pub announce: realms::approval::Announcer, // shows prompts outside the game
}
pub fn prepare(options: Options<'_>) -> Result<Prepared>;
impl Prepared {
    pub fn netns_spec(&self) -> NetnsSpec;                       // Network::Filtered(..)
    pub fn sandbox_tcp_ports(&self) -> Vec<u16>;                 // TcpPolicy::Ports(..)
    pub fn sandbox_udp_ports(&self) -> Vec<u16>;                 // landlock.udp_send = Some(..)
    pub fn trust_anchor(&self) -> Option<(&'static str, &[u8])>; // TrustStore::build(..)
    pub fn trust_pem(&self) -> Option<String>;
    pub fn outbound_tcp_ports(&self) -> Vec<u16>;
    pub fn confinement_needs(&self, keep: &mut Confinement);     // step 9
    pub fn approvals(&self) -> Option<Arc<Approvals>>;           // for the trusted input
    pub fn describe(&self) -> Vec<String>;                       // launch log
    pub fn start(self, spec: &NetnsSpec, sockets: Vec<OwnedFd>, http: HttpClient, log: egress::Logger)
        -> Result<Running>;                                      // step 10
}
impl Running {
    pub fn stats(&self) -> &Arc<egress::Stats>;
    pub fn approvals(&self) -> Option<&Arc<Approvals>>;
    pub fn summary(&self) -> Vec<String>;                        // end of session
    pub fn shutdown(self);
}
pub fn answer_join(approvals: &Approvals, text: &str) -> String; // "answer realms <text>"
pub fn describe_proxy(proxy: SocketAddr) -> String;
```

Wiring, in the launch package's Bedrock dispatch (the order matters):

```rust
// step 6, before the sandbox exists
let net = bedrock::net::prepare(bedrock::net::Options {
    cfg: &cfg.bedrock, instance_servers: &bedrock_instance.servers,
    approve: cfg.realms.approve, proxied: cfg.network.launcher_proxy.is_some(),
    announce, // the same closure launch::prepare_realms builds: log line + Event::RealmsApproval
})?;
for line in net.describe() { say(&line); }
if let Some(proxy) = http.socks5() { say(&bedrock::net::describe_proxy(proxy)); }
let spec = net.netns_spec();            // Network::Filtered(spec.clone()), Resolv::Loopback
landlock.tcp = TcpPolicy::Ports(net.sandbox_tcp_ports());
landlock.udp_send = Some(net.sandbox_udp_ports());
let trust = TrustStore::build(net.trust_anchor())?;
let approvals = net.approvals();        // wire the trusted input now (sign-in package's reader)
// step 8: spawn; step 9: confine
net.confinement_needs(&mut keep);       // plus the proxy's port, as the Java launch adds it
// step 10, after confinement, where Java calls egress::start
let running_net = net.start(&spec, std::mem::take(&mut running.egress), http.clone(), logger)?;
// on "answer realms <text>" from the terminal or the front end (never the game):
//     if let Some(a) = &approvals { let said = bedrock::net::answer_join(a, text); /* show it */ }
// at the end: log running_net.summary(); running_net.shutdown();
```

Why two calls, not one before step 9: the authority must be in the sandbox's trust store, so it
exists before the sandbox; the relay reads what the game sends, so — as in the Java launch — it
starts only after the launcher has confined itself. Nothing it needs is lost at step 9
(`the_bedrock_relay_works_after_the_launcher_has_confined_itself`).

### Shared files touched (small, local)

- `src/egress/mod.rs`: `pub mod bedrock; pub mod raknet; pub mod services;`; a `bedrock` field in
  `Policy` and `services` in `Shared`; `Policy::bedrock`, `is_bedrock`, `bedrock_pass`,
  `allowing_private_bedrock_servers_for_tests`; Bedrock branches in `is_service`, `resolvable`,
  `tcp_ports`, `udp_ports`, `udp_rules`, `netns_spec`, `outbound_ports`, `describe` (each a no-op
  for Java); the responder's refusal text through `name_refusal` (Java's is unchanged);
  `start_bedrock` beside `start` (which refuses a Bedrock policy); one branch in `handle` for the
  service broker; tests at the end.
- `src/egress/udp.rs`: `Limits::raknet`, `Refusal::{NotRakNet, Pass}`, Bedrock wording,
  `Table::pass_datagram`, `game_datagram` gained an `opens` argument (internal), standing anchors
  (`stand`), the Bedrock harness, tests.
- `src/egress/broker.rs`: `Authority::generate_constrained` (and `generate_for` now calls it — the
  same certificate as before), `certificate_pem`; `pub(super)` on `Budget`, `Room`,
  `ReplyBody::{local, held}`, `issue`, `query_is_plain` and five constants, for the service broker.
- `src/realms/approval.rs`: `Approvals::note_name` (and `note_names` shares its bounds) — the
  event server's name for its prompt.
- `src/fuzzing.rs`: three entries at the end of `ALL` and three functions; `tests/robustness.rs`:
  three names, seeds, four tests; `fuzz/Cargo.toml` and three targets.
- Not touched: `Cargo.toml` (no new dependency), `src/config.rs` (no new key), `src/cli.rs`,
  `src/launch.rs`, `src/main.rs`, `src/lib.rs`, `examples/config.toml`, `crates/proto`, `gui/`.

---

## 6. Where the contract did not fit

1. *"its HTTP goes through the system libcurl"* — not by default: the client links a static curl
   8.21.0 that its manifest builds (`USE_OWN_CURL=ON`; the AUR package keeps it), downloaded
   without a checksum. Its compiled-in CA bundle on an Arch build is
   `/etc/ssl/certs/ca-certificates.crt`, which the trust store covers, so the design holds.
2. *"One entry point the launch calls before step 9"* — two calls (section 5): `prepare` before the
   sandbox, `start` after step 9 like `egress::start`.
3. *"a one-time pass"* — one approval is one join, but up to four flows to the same server within
   30 s of the first (a RakNet client may ping and connect from different sockets; unverified with
   the real client).
4. *`[realms] approve`* — `totp` acts as `code` for Bedrock (invariant 3: the TOTP secret is in the
   token store).
5. *The base's `DENIED`* — refined (Java Realms hosts, NetherNet signalling; two of them lie under
   `.minecraft-services.net`), and the base test "never reached by a suffix" became "denied even
   where a suffix would reach them" (the deny list wins in the relay).
6. *`BedrockServer`* has no `allow_private`: LAN and same-machine servers are refused in this
   version. Allowing them needs `#[serde(default)] allow_private: bool` in the shared struct.
7. *Terminated hosts* — besides the Realms API host the contract names, `pocket.realms.minecraft.net`
   and the two known gatherings hosts (the real one comes from the game's discovery at run time).
