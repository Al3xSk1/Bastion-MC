# Voice chat: a UDP relay for allowlisted game servers — and the proxy fix (B-M3)

Work package "a3-voice", 2026-11-13. Three parts, as asked: (1) user documentation, ready
for the README; (2) what it means for the threat model, residual risks included; (3) what is
open or unverified. Then the detail behind them: how it works, the limits, how B-M3 was
fixed, what was verified and how, and what the integrator needs to know.

Nothing in `README.md`, `TODO.md` or `docs/THREAT_MODEL.md` was edited; the text for them is
below.

Four engineers worked on this package, one after the other (history and state of the work:
`voice-chat-progress.md`). The first wrote the code and this file; the third checked the
file against the code, corrected it, and added what was found and verified afterwards; the
fourth finished the fix for a pre-existing broker-test flake as a patch for the integrator
(section 7, item 4), reviewed the code once more and ran the final verification. Where a
check was made by the first engineer and not repeated, it says so.

---------------------------------------------------------------------------------------------

## 1. User documentation (for the README)

### Voice chat (Simple Voice Chat and similar mods)

Voice-chat mods send sound as UDP datagrams to a port on the game server's machine. In the
default (`filtered`) network mode the game has no UDP at all, so such a mod loads but stays
silent. To let it talk, name the server's voice port **in that server's rule**:

```toml
[network]
servers = [
    "survival.example.org",                              # no voice: nothing changes
    { host = "play.example.org", voice = 24454 },        # Simple Voice Chat's default port
    { host = "lan-box", port = 25570, allow_private = true, udp = [24454, 24455] },
]
```

The same works per instance, in `instances/<id>/instance.toml` under `[network]`.

* `voice = N` and `udp = [N, …]` mean the same thing; use whichever reads better. At most
  four ports per server, sixteen different ones in total, each 1024 or higher.
* The port is the one the *server's* voice chat listens on — ask the server's operator if it
  is not 24454. Some hosts run voice on the game port; then it is `voice = 25565`.
* There is no switch that allows UDP for all servers, `--join` alone does not allow any
  (it allowlists the game connection for one session, nothing more), and a mistyped key in
  a server's table is now an error instead of being ignored.

What the launcher then does, and does not do:

* It relays datagrams to that port **only while a game connection to that server is open**,
  and only **to the address that game connection is using**. Voice cannot go anywhere the
  game is not already connected to, and it stops when you leave the server: as soon as
  either the game or the server hangs up, nothing more is relayed.
* Datagrams are limited to 1400 bytes and to a voice-sized rate (100 a second from you,
  500 a second to you, per connection of the mod; 8 such connections at once). Nothing is
  inspected or decoded: the launcher does not know that it is voice.
* The launch log says what is open (`network: game server play.example.org:25565 … + UDP
  port(s) 24454 …`), logs each voice connection once when it starts and once when it ends
  (`egress: ALLOW udp …`, `egress: done udp … ↑1203 ↓5210 datagrams, 0 refused`), logs
  each kind of refusal once (`egress: DENY udp …`), and gives the totals in one line at the
  end (`egress summary, UDP: …`).
* If voice does not connect, the log tells two cases apart. `egress: DENY udp … (the
  server's address reports that datagrams to this port do not arrive (nothing listens
  there?))`: the rule names a port on which the server does not listen. No `udp` line at
  all: the server announced a port that the rule does not name. Such a datagram is refused
  inside the sandbox (on kernel 7.2 and later the mod's socket gets "Permission denied")
  and never reaches the launcher, which therefore has nothing to log.
* The mod needs sound: `audio = "pulse"` (the default; see "Sound" above for the one-time
  PipeWire setting). **With audio on, the game can record your microphone whether or not
  you use voice chat** — that was already so; voice chat is what makes it worth knowing.
* Not supported: a server that tells clients to use a *different* host for voice than for
  the game (`voice_host` in the server's voice-chat configuration). The launcher sends
  voice where the game connection goes and nowhere else.
* Voice is not sent through `launcher_proxy` (neither is the game connection): the server
  sees your address.

To use the mod itself, add its jar to a curated profile as for any other mod (`profile
lock`, review, `profile sign`). It ships native libraries (Opus, RNNoise, Speex, LAME) that
it unpacks and loads inside the sandbox; `bastion-mc profile audit <id>` lists them.

### Replacement for the proxy sentence in "Limits, proxy"

> `launcher_proxy = "socks5h://127.0.0.1:9050"` under `[network]` sends everything that goes
> to Microsoft and Mojang through a SOCKS5 proxy such as Tor, which also resolves the names:
> sign-in, downloads, and — while the game runs in the default filtered mode — what the
> credential broker forwards, skin textures, and the game's own HTTPS connections to
> Mojang's services. **Not** proxied: connections to multiplayer servers, the lookup of
> their names (your resolver sees those), and voice. With `mode = "open"` nothing the game
> sends is proxied. The launch log states which of these applies.

### One line for the "Status" table

> UDP relay for voice chat | 13 tests in the real sandbox (round trip with the correct source
> address, every refusal, limits, expiry, either side hanging up, Landlock, a real JVM), 21
> unit tests, one generated-input harness. In a real launch of Minecraft 26.3 with Simple
> Voice Chat 2.6.27 the mod loads with its native libraries, and the launcher — confined by
> then — relayed datagrams for a stand-in client placed in the game's network namespace.
> **Not** exercised: a real voice session (needs a server and a signed-in account), capture
> (sound is off on the development machine).

---------------------------------------------------------------------------------------------

## 2. Threat model: rows and changes

### 2.1 What this is, in one paragraph

A datagram channel from the game to hosts it can already exchange arbitrary bytes with over
TCP. It adds **no new host**: a datagram is relayed only to the address of a relayed TCP game
connection that is open at that moment, made under a rule for the same server. It adds **one
new port per rule entry** on that host — the UDP port(s) the user wrote into the rule — and
nothing else. Everything below follows from that.

### 2.2 Row J10 — amend

Defence, replace the sentence about UDP with:

> … and (Landlock ABI 10) UDP only to the launcher's DNS port, from an ephemeral port — plus,
> **only if a server rule names UDP ports**, to the launcher's relay sockets on exactly those
> ports (J18).

Verified by, add: `voice::landlock_lets_the_sandbox_send_to_named_udp_ports_only` (with a
rule: the named port and DNS, `EACCES` for every other port, for the game's TCP port, for
IPv6, and for binding the port; without a rule: `EACCES` for that port as well).

### 2.3 New row J18 (section 4.2)

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| J18 | Use the voice-chat path (UDP) to reach another host, the LAN or the launcher's own services; to flood; or to turn the launcher into a reflector | **Opt-in per server rule** (`voice = N` / `udp = […]`, ports ≥ 1024, ≤ 4 per rule, ≤ 16 in all); without one no UDP socket exists and nothing changes. With one: a datagram is relayed only (a) to a destination that maps to a rule naming that UDP port, (b) while a relayed TCP game connection made under a rule for the same host and TCP port is open — in both directions: the first hang-up by either side ends it —, (c) **to the address that connection uses** — the relay never resolves a name for a datagram, so there is no second lookup to rebind, (d) if that address is public or a rule naming the port says `allow_private`. Each flow has its own host-side socket, `connect`ed to the server: the kernel drops datagrams from anyone else, and answers go only to the flow's own source inside the sandbox, from the address the game sent to (`IP_PKTINFO`). Bounds: 8 flows, 30 s without a datagram *from the game* ends a flow, 1400 bytes per datagram, 100/s to the server and 500/s to the game per flow. Flows end with their game connection. Payloads are never parsed. | `voice::*` (14 tests: 13 in the real sandbox, one of them with a real JVM, and one that confines a process the way the launcher confines itself), `egress::udp::tests::*` (10), `egress::tests::udp_ports_come_from_server_rules_and_from_nowhere_else`, `config::tests::hostile_and_mistaken_udp_rules_are_errors`, `sandbox::netns::tests::*`, `robustness::datagram_relay_decisions` (300 000 generated scripts), §7 E19 (the relay inside a real launch) | See 2.4. |

### 2.4 Residual risks, plainly

1. **A covert channel to an allowlisted server existed already** (section 5.8) and still
   does; it now has a datagram form: sizes, timing and content of up to 100 datagrams a
   second per flow, to the voice port of a server the user allowlisted. The relay cannot
   tell voice from anything else and does not try.
2. **"A game connection is open" is not "the user is playing".** Code in the game can open
   a TCP connection to an allowlisted server by itself and keep it for as long as the
   server lets it idle (the status ping is enough to open one). The tie guarantees *where*
   datagrams can go, not *why*. So: malicious code in the game can send datagrams to the
   named UDP port of an allowlisted server's address whenever it likes, within the limits.
   What it cannot do is outlast the server: when the server hangs up, the path closes even
   if the game holds on to its socket — and when the game hangs up, it closes even if the
   server (or something on the way) never does.
3. **Flooding is bounded, not impossible.** Worst case all 8 flows at the full rate and
   size: 8 × 100 × 1400 bytes ≈ 1.1 MB/s (9 Mbit/s) towards allowlisted servers the game
   holds connections to — whom it can already send to over TCP without any limit. Towards
   the game: 500 datagrams a second per flow are passed on; a hostile *server* can send
   more, which costs the launcher the work of dropping them (as a TCP flood would).
4. **No reflection or amplification through the launcher.** It never sends a datagram to
   an address that did not come from an open TCP connection of its own, never with a
   forged source, and what the server answers goes into the sandbox only. What remains is
   the ordinary exposure of a UDP client: someone who can forge the server's address and
   guess the flow's source port can inject datagrams into a flow (the mod encrypts and
   will discard them; the relay does not know).
5. **The microphone** (D3, section 5.4). With `audio = "pulse"` the game can record,
   voice chat or not. D3 says "exfiltrating it still needs J10": with a voice rule the
   recording can leave as datagrams to that server — which is what voice chat *is*, and
   indistinguishable from it. Without a voice rule the only way out is still the TCP
   connection to an allowlisted server.
6. **DNS steering applies to voice as it does to the game** (N2): whoever controls the
   answer for the server's name when the game connection is made receives the voice as
   well. No more than that: voice cannot be steered separately.
7. **Kernels before Landlock ABI 10**: the second layer for UDP does not exist there (as
   before). The private namespace is then the only barrier; in it, only the launcher's
   sockets exist, so the relay's own checks still decide everything. Not tested here (this
   kernel has ABI 10).
8. **The launcher's own confinement does not restrict UDP** — see 2.6.

### 2.5 Other rows to touch

* **D3**, residual: "… exfiltrating it still needs J10 — or, where a server rule names a
  voice port, J18: recorded sound can go to that server as datagrams, which is what voice
  chat does."
* **Section 5.7**, replace: "Realms servers are handed out as bare IPs and are not
  reachable in filtered mode. UDP exists only as relayed datagrams to the UDP ports that a
  server rule names (voice chat, J18); LAN discovery and everything else over UDP does not
  exist in the sandbox."
* **Section 5.8**, add: "With a voice rule that includes datagrams (J18)."
* **N1** (the proxy), Defence: "… then the proxy resolves the names and carries the bytes —
  for the launcher's own requests and for the connections to Mojang's services that the
  relay passes through for the game — and TLS … is still end to end with the named host."
  Verified by, add: `egress::pass_through_service_connections_go_through_the_socks5_proxy_which_resolves_the_name`,
  `egress::tests::the_proxy_note_says_what_is_and_what_is_not_proxied_in_each_network_mode`,
  `egress::tests::with_a_proxy_the_launcher_itself_needs_no_direct_https`.
  Residual, add: "Through a proxy the relay never sees the address a service name resolved
  to, so its refusal of private addresses (J12) cannot apply to those connections; what the
  proxy will connect to is the proxy's policy. Game servers and voice are never proxied."
  (Tor's manual describes defaults that reject internal addresses, on the client and at
  exits; that was read, not tested, so it is left out of the row.)
* **J12**: unchanged for direct connections; see the N1 residual for proxied ones.
* **Section 6**, the row "Landlock UDP rules for the launcher itself | sandbox only": add
  to the reason "… and the voice relay answers to the game's ephemeral ports in the same
  way, and sends to whatever UDP ports the rules name."
* **Section 7**, experiments worth recording:
  * E16 — A wildcard UDP socket in the game's namespace answers a datagram that was sent
    to a synthetic address (127.64.0.1). Answered plainly: the answer leaves from
    127.0.0.1, and a `connect`ed client socket (Python, and Java's `DatagramChannel`)
    never receives it. Answered with `IP_PKTINFO` naming the address the datagram was sent
    to: it arrives. (Mutation check, repeated by the third engineer: with the control
    message removed from `GameSocket::send`, five tests of `tests/voice.rs` fail, each on
    the address the answer came from — the round trip, the refusals test, the IPv6 test,
    the JVM test and the test with a literal address that is no loopback address.)
  * E17 — Simple Voice Chat 2.6.27 (Fabric, Minecraft 26.3; Modrinth file
    `voicechat-fabric-2.6.27+26.3.jar`, SHA-512 verified against the API's value) in a
    signed profile, demo mode, on the private display: "Loading 51 mods" including
    `voicechat 2.6.27+26.3`; "Initializing Opus … Successfully initialized Opus", likewise
    RNNoise, Speex and LAME — four native libraries unpacked (the JVM's temporary directory
    is the sandbox's `/tmp`) and loaded inside the sandbox; 13 log events from the mod, all
    of level INFO; nothing refused on its behalf, no tripwire. The warnings and errors in
    the whole log are the game's own and have nothing to do with the mod: Realms and
    `/player/attributes` (no account), the narrator (`libflite` is not in the sandbox), the
    sound system (sound is off), `oshi` (hardware probing). The mod attempted no voice
    connection (demo mode has no multiplayer) and sound was off. Done three times with the
    same result: by the first engineer, and twice by the third, the last time on the final
    code. The jar's SHA-512 was compared again with a fresh answer of Modrinth's API.
  * E18 — the pre-fix relay with a proxy configured and a service name under `.invalid`:
    `DENY … (DNS failure …)`, i.e. it had asked the local resolver. After the fix the same
    connection reaches the stand-in service through the stand-in proxy.
  * E19 — the relay inside a real launch. `bastion-mc launch fabvoice --demo`, the
    instance's rule pointing at two stand-ins on the host's loopback: a TCP echo as game
    server that **never closes its end**, and a UDP echo as voice server. While the game
    ran, and after the launcher had confined itself, a process was placed in the game's
    *network namespace* (`nsenter -U -n` on the sandbox's outer bubblewrap; possible for the
    same user, see P7) and did what the mod does. Sockets in the namespace: UDP
    `0.0.0.0:<voice port>` and `127.0.0.1:53`; TCP 443, 80 and the game port; one
    interface, `lo`. A datagram before any game connection: no answer, `DENY udp … (no game
    connection to this server is open …)`. After a TCP connection to the game port (`ALLOW …
    game server → …`): four datagrams, each answered from the address and port it was sent
    to (`ALLOW udp … (flow 1 from …; follows the game connection …)`). After the client had
    closed that connection — the server had not —: no answer, and `done udp … ↑4 ↓4
    datagrams, 0 refused (its game connection closed)`. A second connection: answered again
    (flow 2). At exit: `egress summary, UDP: 10 datagram(s) relayed in 2 flow(s), 2
    refused`; "game stopped on request", exit status 130. The first run of this experiment
    is what found the flaw described in section 6 ("open" used to mean "not yet closed by
    both"): there the datagram after the close was still answered.

### 2.6 The launcher's confinement and this relay

Once the game runs the launcher confines itself with Landlock (`confine.rs`): files, program
execution, TCP ports. **It has no UDP rule** (THREAT_MODEL section 6), and the relay depends
on that: it opens a socket per flow *after* confinement, sends to the UDP ports the rules
name, and answers to the game's ephemeral ports. Consequences:

* Where datagrams may go is enforced by the relay's decision table alone — by ordinary code
  in the launcher, not by the kernel. A bug in it, or a compromised launcher, is not caught
  by a second layer on the launcher's side. (This was already true of UDP in general: a
  compromised launcher could send datagrams anywhere, review finding A-F1 notes it.) The
  table is therefore small, free of I/O, unit-tested and driven by a generated-input
  harness that checks exactly this property.
* The sandbox side is unchanged and kernel-enforced: the game can send datagrams only to
  ports on which only the launcher listens.
* `voice::the_relay_can_still_open_flows_after_the_launcher_has_confined_itself` pins the
  dependency: if the confinement ever gains UDP rules (or a seccomp filter that touches
  `socket`/`bind`/`connect`/`sendto`/`recvmsg`), that test fails rather than voice silently
  breaking. Landlock's UDP rules are per port and per process, so "the relay may send to
  port 24454, the DNS responder and the relay may answer to any port" cannot be expressed
  for one process today.

### 2.7 seccomp

Nothing was added to the allowlist. A JVM's datagram sockets need `socket(AF_INET6 |
AF_INET, SOCK_DGRAM)`, `bind`, `connect`, `sendto`/`sendmsg`, `recvfrom`/`recvmsg`,
`setsockopt`/`getsockopt`, `getsockname`, `poll`/`epoll_*` — all already allowed (`socket` is
filtered by family, not by type). Shown by running it: `voice::a_jvm_in_the_sandbox_talks_…`
does a round trip with `java.net.DatagramSocket` (what Simple Voice Chat uses — checked in
its class files) and with a connected NIO `DatagramChannel`, on Java 27, under the default
seccomp policy with the tripwire on.

---------------------------------------------------------------------------------------------

## 3. Open and unverified

**Not verified, and why**

* **A real voice session.** Needs a server running the mod and a signed-in account; this
  machine has neither. Everything about the relay was tested against stand-ins (a TCP echo
  "game server" and a UDP echo "voice server" on the loopback, reached through rules with
  `allow_private`). Whether the real client is content with the limits (100 datagrams a
  second upstream is twice its 20 ms frame rate, by arithmetic, not by measurement), how it
  behaves when a datagram is dropped, and how it copes with the relay's silence towards
  refused datagrams is not known from experiment.
* **Capture and playback.** Sound is off in the test configuration and had to stay off. The
  mod's audio path (OpenAL through the Pulse socket) did not run: Minecraft itself reported
  "Error starting SoundSystem" as expected with `audio = "none"`.
* **Datagrams sent by the game itself in a real launch.** In the real launches the game
  sent none (demo mode has no multiplayer). The launcher's side of a real launch — the
  sockets in the namespace, the relay after confinement, the log and the summary — was
  exercised with a stand-in client placed in the game's network namespace (E19). That
  client was not inside the game's sandbox, so in a real launch the Landlock UDP rule and
  the seccomp filter never met a datagram socket; those are verified by `tests/voice.rs`,
  which builds its sandboxes from the same `Policy::sandbox_udp_ports` and the default
  seccomp policy.
* **A real SOCKS5 proxy (Tor).** B-M3 is tested against the stand-in proxy only.
* **Kernels without Landlock ABI 10**, and Landlock-less UDP behaviour on them.
* **Other voice mods** (Plasmo Voice and others): not looked at. Anything that sends plain
  UDP to a port of the game server's own address should work the same way; anything that
  uses another host, STUN/ICE or WebRTC will not.
* **The cargo-fuzz target `udp`**: it compiles (`cargo check` in `fuzz/` on the stable
  toolchain); like the other targets it has never run (no nightly toolchain here). The same
  harness passed 300 000 generated scripts under `tests/robustness.rs` (release build;
  repeated by the third engineer — the decision table and the harness have not changed
  since that run).
* **Simple Voice Chat's own network code** was read from its class files (`javap`), not
  run: it opens `new DatagramSocket()`, sends to `InetAddress.getByName(host)` where `host`
  is the server's `voice_host` if set, else the host of the game connection, on the port the
  server announces.

**Open, by design or for lack of a decision**

* **Servers with a separate voice host** (`voice_host` pointing elsewhere, e.g. behind a
  TCP-only proxy such as a DDoS shield) are not supported. Supporting them means relaying
  datagrams to an address that no TCP connection vouches for; that is a different, weaker
  design (a second name resolution, a second destination) and should be a deliberate
  decision, not an extension of this one.
* **The GUI** says nothing about voice. The launch log line and the `egress summary, UDP`
  line reach it as ordinary log lines; the protection panel is unchanged (the network
  shield still reads "filtered"). A line such as "voice: relayed to play.example.org:24454
  while connected" there would be honest; not built (another engineer owns `gui/`).
* **`doctor`** reports the Landlock UDP rule; it does not test the relay.
* **Tests that fail now and then for reasons outside this package** — see section 7.

---------------------------------------------------------------------------------------------

## 4. How it works (for whoever maintains it)

```
 sandbox (private netns)                    launcher (host netns)                 server
 ┌──────────────────────┐        ┌──────────────────────────────────────┐
 │ mod: DatagramSocket  │  UDP   │ wildcard socket 0.0.0.0:24454        │
 │  sendto(127.64.0.1,  │ ─────▶ │  (bound in the netns by the helper)  │
 │         24454)       │        │  recvmsg + IP_PKTINFO → "sent to     │
 │                      │        │  127.64.0.1" → name → rule           │
 │                      │        │        │ Table::game_datagram        │
 │                      │        │        ▼                             │
 │                      │        │ flow socket, connect()ed to          │  UDP
 │                      │        │  <address of the open TCP game       │ ─────▶ 203.0.113.7:24454
 │                      │  UDP   │   connection>:24454                  │ ◀─────
 │  recvfrom → from     │ ◀───── │  sendmsg + IP_PKTINFO(spec_dst =     │
 │  127.64.0.1:24454    │        │   127.64.0.1) on the wildcard socket │
 └──────────────────────┘        └──────────────────────────────────────┘
```

* **Configuration** (`src/config.rs`): `ServerEntry::Full` gained `voice: Option<u16>` and
  `udp: Vec<u16>`; `ServerRule` gained `udp: Vec<u16>` (sorted, deduplicated). `Deserialize`
  for `ServerEntry` is written out by hand: the derived untagged form **silently ignored
  unknown keys in a server table** (`{ host = "x", alow_private = true }` was accepted and
  meant `allow_private = false`), against the file's own "unknown keys are errors". That
  is fixed for every key of the table, not only the new ones. `port = 0` in a table is now
  refused as well (it was accepted and meant "a random port").
* **Namespace helper** (`src/sandbox/netns.rs`): `NetnsSpec.udp_ports`; one wildcard UDP
  socket per port. The order of the sockets handed over is `NetnsSpec::sockets()` — DNS,
  TCP listeners, UDP sockets — and that function is the only place the order exists: the
  helper creates by it, `egress::start` interprets by it, and `sandbox::spawn` checks every
  received descriptor against it (`netns::check_socket`: socket type, bound address, port,
  read back from the kernel). A mismatch stops the launch.
* **Landlock** (`src/launch.rs`): `landlock.udp_send = [53] + the rules' UDP ports` in
  filtered mode (`Policy::sandbox_udp_ports`); `[53]` alone without such a rule.
* **Relay** (`src/egress/udp.rs`): `Table` decides (no sockets, explicit clock; what the
  unit tests and the harness drive), the rest of the file does the I/O. A game connection
  that datagrams can follow — a rule for its host and TCP port names UDP ports
  (`Policy::followed_by_datagrams`) — registers itself in `relay_game` (`udp::anchor`) with
  the address it reached. A flow is created by the first datagram from a source/destination
  pair; its task carries the server's answers back and ends the flow on silence from the
  game or when its game connection closes.
* **When a game connection stops being "open".** At the first hang-up by either side:
  `splice_anchored` in `src/egress/mod.rs` does the two copies and shutdowns that
  `tokio::io::copy_bidirectional` does (the same `tokio::io::copy`, the first error ends
  both) and drops the registration when the *first* direction ends — which ends the flows
  under the table's lock — while the other direction is still carried to its end. Game
  connections that no datagram can follow go through the unchanged `splice` and register
  nothing: for them, and for every launch without a UDP rule, the TCP relay is what it was.
* **IPv4 first.** For a server whose rule names UDP ports, the *game connection* tries the
  name's IPv4 addresses before its IPv6 ones (the Java client itself prefers IPv4; the
  system resolver on a dual-stack network usually does not). Voice follows the game
  connection's address, so this choice is the choice for voice; a voice port is more often
  reachable over IPv4 only than over IPv6 only. Same set of addresses, other order; rules
  without UDP ports are untouched. (`voice::a_server_with_udp_ports_is_reached_over_ipv4_when_it_has_both`.)
* **Statistics and log**: `Stats.udp_flows / udp_relayed / udp_refused`; `egress summary,
  UDP: …` at the end of a launch if there was any UDP at all. Refusals are logged once per
  destination and kind, for at most 16 destinations per kind — per kind, so that a game
  inventing destinations cannot use up the lines that say something about a real server.
  An ICMP error from the server's side ("nothing listens there") is logged the same way,
  once, as what it is — whether the kernel reports it on the flow's next receive or on its
  next send (`unsent_because`). The datagram that drew the error had left and is counted as
  relayed; a datagram that the kernel then refuses to send because the error is pending is
  counted as refused.

### Limits and their values (`egress::udp::Limits::default()`, pinned by a unit test)

| limit | value | why |
|---|---|---|
| UDP ports per server rule | 4 | a voice mod uses one |
| lowest port a rule may name | 1024 | never DNS/NTP/SNMP… of the server's address; and 53 is the sandbox's own resolver |
| different UDP ports over all rules | 16 | each is a socket and a task |
| flows at once | 8 | a voice client needs one; reconnects overlap for 30 s |
| idle time | 30 s without a datagram **from the game** | the client answers each keep-alive of the server, one a second by default (`keep_alive = 1000` in the mod's `ServerConfig`); the server talking does not keep a flow alive |
| datagram size | 1400 bytes, both directions | one Ethernet frame over IPv4 or IPv6. Simple Voice Chat 2.6.27 limits the Opus payload of a packet to the server's `mtu_size` (default 1275, the largest Opus frame there is; a server may set 512 to 2048); with its framing and encryption the largest packet at the default comes to about 1330 bytes towards the server and 1365 towards the game — by a reading of its packet classes, not measured. Ordinary voice frames are a fraction of that. (An earlier version of this file said the mod caps its packets at 1024: that is not what 2.6.27's `ServerConfig` says.) |
| rate, game → server | 100/s, burst 100, per flow | one microphone is 50/s (20 ms frames) plus keep-alives |
| rate, server → game | 500/s, burst 500, per flow | one stream per audible speaker: ten at once |
| refusal log lines | 1 per destination and kind, 16 destinations per kind | |

The task description suggested "around 50 packets per second each way". That is what one
speaker produces; the server, however, forwards one stream **per audible speaker**, so 50/s
towards the game would have broken any conversation of more than one other person. Hence
the asymmetric limits. Datagrams towards the game go only into the sandbox, so the higher
number there is not a flooding concern.

---------------------------------------------------------------------------------------------

## 5. B-M3: pass-through connections and `network.launcher_proxy`

**Finding** (review B): with a proxy configured, `relay_service` — the game's own TLS to a
Mojang service domain, passed through after the SNI check — resolved the name locally and
dialled directly. With `broker.enabled = false` that covered every Mojang API call.

**Verified first**: yes. With the old code, a proxy configured and a service domain under
`.invalid`, the relay logged `DENY … (DNS failure: failed to lookup address information …)`:
it had asked the local resolver.

**Fix**

* `src/net/http.rs`: the SOCKS5 client (`socks5_connect`) takes the port as a parameter,
  and `socks5_dial(proxy, host, port)` — connect to the proxy, CONNECT by name, with the
  existing timeouts — is the one way through a proxy for both users. `HttpClient::connect`
  now calls it; nothing was copied.
* `src/egress/mod.rs`, `relay_service`: with a proxy, the outbound connection is
  `socks5_dial(proxy, name, 443)`; the name goes to the proxy as text and is not looked up
  here. Game servers (`relay_game`) are dialled directly, as designed.
* `Policy::outbound_ports` leaves out 443 when a proxy is configured (unless a game server
  listens there): the launcher, once confined, then *cannot* make a direct HTTPS
  connection — the proxy's port is what it keeps. Signature unchanged.
* Messages: `egress::describe_proxy(proxy, mode)` writes the launch-log line, per network
  mode. In filtered mode it lists what is proxied (sign-in, downloads, broker, textures,
  pass-through HTTPS) and what is not (multiplayer servers, the lookup of their names and
  SRV records, UDP voice). In **open** mode it says that nothing the game sends is proxied —
  the previous message did not, and with the open network the game talks to Mojang
  directly. The comments on `launcher_proxy` in `src/config.rs` and `examples/config.toml`
  say the same.
* A consequence stated in the code and in 2.5: through a proxy the relay cannot apply its
  private-address refusal to these connections, because it never sees the address.

**Test**: `egress::pass_through_service_connections_go_through_the_socks5_proxy_which_resolves_the_name`
(real sandbox, Python client sending a real ClientHello; stand-in proxy shared with
`tests/tls.rs` through `tests/support/socks5.rs`): the connection reaches the stand-in
service; the proxy was asked for `svc.relay-test.invalid:443` by name, once; the service
received the ClientHello unchanged; a ClientHello for another name is refused before the
proxy hears of it; a refusing proxy and an absent proxy both end in a closed connection, with
no direct attempt. With `BASTION_NET_TESTS=1` the control also runs (no proxy: DNS failure);
run that way by the third engineer: `tests/egress.rs` 6 of 6, the two live tests included.

---------------------------------------------------------------------------------------------

## 6. What was verified, and how

All on the development machine (kernel 7.2.14, Landlock ABI 10, bubblewrap, OpenJDK 27).

| claim | how |
|---|---|
| Round trip; the answer comes from the address and port the game sent to | `voice::a_datagram_reaches_the_voice_server_and_the_answer_comes_from_where_it_was_sent`: by name (synthetic address) and by literal address, unconnected and `connect`ed sockets. **Mutation-checked** (first and third engineer): without the `IP_PKTINFO` message on the way back the answer comes from 127.0.0.1 and five tests fail (E16). |
| The same for a literal address that is no loopback address (the helper's alias on `lo`) | `voice::a_literal_address_that_is_no_loopback_address_is_answered_from_that_address`: the stand-ins are bound to this machine's own address on the local network (they answer only this machine; nothing leaves it), the client sends once from a socket bound to 127.0.0.1 — the one case in that test in which the kernel would not pick the right source by itself — and any other address has no route. Skipped on a machine without such an address. **Mutation-checked.** |
| An IPv6 game connection is followed; IPv4 is preferred when both exist | `voice::datagrams_follow_a_game_connection_that_uses_ipv6`, `voice::a_server_with_udp_ports_is_reached_over_ipv4_when_it_has_both` |
| Refused: a port not named for that server; a server without a UDP rule; no open game connection (before one, and after it closed); a literal address that is no server; a synthetic address never handed out; a Mojang service name; an address off the machine | `voice::datagrams_without_a_rule_a_port_or_a_game_connection_are_refused` — for each: no answer in the sandbox, nothing at the voice server, one `DENY` line with the reason, counted |
| Oversize (both directions), rate limit (both directions), flow limit; an empty datagram is a datagram | `voice::size_rate_and_flow_limits_hold_on_real_sockets` (size limit as in production; small rates and 3 flows so that a handful of datagrams reaches them), `egress::udp::tests::size_rate_and_flow_limits_hold_in_both_directions`, `…::a_bucket_gives_its_burst_then_its_rate_and_never_more` |
| Idle flows expire; flows end with their game connection; only the game keeps a flow alive | `voice::a_flow_ends_when_the_game_falls_silent_and_when_its_game_connection_closes` (1 s timeout injected), `egress::udp::tests::only_the_game_keeps_a_flow_alive_…`; the built-in 30 s once, in a real launch (E19, first run) |
| "Open" means open in both directions | `voice::datagrams_stop_as_soon_as_either_side_hangs_up_the_game_connection`: a server that keeps its end after the game has hung up, and a game that keeps its end after the server has. **Written first and seen to fail** on the code as it was: both datagrams were relayed. Also E19. |
| A game connection that datagrams follow carries its bytes like any other | `voice::a_game_connection_that_datagrams_follow_carries_its_bytes_to_the_end`: 512 KiB each way, the game closing its direction first; echoed byte for byte, `done … ↑524293 ↓524293 bytes` |
| A voice port on which nothing listens is said once, and as that | `voice::a_voice_port_on_which_nothing_listens_is_reported_once` (spaced datagrams and twenty without a pause), `egress::udp::tests::a_refused_send_is_the_servers_answer_and_any_other_failure_is_this_machines`. Before the fix the burst was logged as "the operating system could not send it on" in 3 of 3 runs. |
| Private addresses need `allow_private` on the rule that names the port | `egress::udp::tests::private_addresses_need_the_flag_on_the_rule_that_names_the_port` (unit level: the TCP relay does not connect to a private address without the flag, so the case needs two rules) |
| Landlock: named ports and DNS only; nothing changes without a rule | `voice::landlock_lets_the_sandbox_send_to_named_udp_ports_only` |
| Socket order of the helper | `sandbox::netns::tests::*` (order, round trip of the specification, `check_socket` on real sockets), `voice::the_helper_hands_over_its_sockets_in_the_order_the_specification_gives` (the real helper: kernel's view of each descriptor, eight wrong readings refused, and a marked message arriving on each socket) |
| Configuration syntax, hostile values | `config::tests::a_server_rule_can_name_udp_ports`, `…::hostile_and_mistaken_udp_rules_are_errors` (35 inputs), `…::an_instance_takes_the_same_server_rules_and_checks_them_when_it_is_loaded`, `…::the_example_configuration_is_the_defaults` |
| A JVM needs nothing more from seccomp | `voice::a_jvm_in_the_sandbox_talks_to_the_voice_server_with_plain_java_sockets` (Java 27) |
| Works after the launcher confined itself | `voice::the_relay_can_still_open_flows_after_the_launcher_has_confined_itself` |
| The log cannot be flooded or one kind crowded out | in `voice::datagrams_without_…` (40 invented destinations → 16 lines, a later refusal of another kind still logged) |
| The decision table holds under arbitrary event orders | `robustness::datagram_relay_decisions` (harness `fuzzing::udp`), 4 000 scripts per run, 300 000 in a release build (first engineer, and again by the third). **Mutation-checked** by the first engineer, not repeated: dropping the private-address check, and leaking a flow past its game connection, each fail it. |
| B-M3 | section 5. **Mutation-checked** by the first engineer, not repeated: with the old direct dial the test fails. |
| The real mod loads; the relay works inside a real launch | section 2.5, E17 and E19 |

`cargo fmt --all`, `cargo clippy --workspace --all-targets` (no warnings) and `cargo test
--workspace` were run at the end: 354 passed, 0 failed — by the third engineer, and again by
the fourth on the final code (whose first full run that day had flake (1) of section 7 and
nothing else); `BASTION_NET_TESTS=1 cargo test --test egress` 6 of 6 as well. The launcher
crate has 295 tests (205 unit, 90 integration); 38 of them are this branch's (21 unit: 3 in
`config`, 5 in `egress`, 10 in `egress::udp`, 3 in `sandbox::netns`; 17 integration: 15 in
`tests/voice.rs` — 14 tests and the re-executed half of one —, 1 in `tests/egress.rs`, 1 in
`tests/robustness.rs`). `tests/voice.rs` as a whole: 25 of 25 runs green before the last
changes (second engineer), 20 of 20 after them.

### Three things the review found and fixed (third engineer)

1. **The second checkpoint contained a mutation.** The work was interrupted in the middle
   of a mutation check; the checkpoint commit `ae996c2` has the `IP_PKTINFO` message
   removed from `GameSocket::send`. Restored in the next commit. The test it was meant to
   check (`a_literal_address_that_is_no_loopback_address_…`) passed *with* the mutation, so
   it did not test what its name says; it does now (see the table).
2. **A refused send was logged as a fault of this machine.** A connected UDP socket
   reports the ICMP "port unreachable" for one datagram on its next call. If that call was
   the next send — any two datagrams sent back to back — the log said "the operating system
   could not send it on (no socket, or a full buffer)". It now says what it is.
3. **"While a game connection is open" was "until both sides have closed it".** Found in
   the real launch (E19): the stand-in server kept its end, and a datagram sent after the
   client had closed its game connection was still relayed. The reverse held as well: after
   the *server* had hung up, the game could keep the path open by holding on to its socket.
   Neither reached a new destination, and code in the game can open a connection of its own
   anyway; but the server no longer had a say in how long, and the user-visible promise
   ("it stops when you leave the server") depended on the server's manners. Now the first
   hang-up ends it.

---------------------------------------------------------------------------------------------

## 7. For the integrator

* **`Cargo.toml`**: one word — the `net` feature of `nix` (already a dependency; in nix
  0.31 `net = ["socket"]`, and `socket` was enabled before, so no new crate and `Cargo.lock`
  is unchanged — checked with `git diff`). It provides `IP_PKTINFO` as safe wrappers
  (`recvmsg`/`sendmsg` control messages, `SockaddrIn`); the alternative was about sixty
  lines of `unsafe` cmsg handling.
* **Types that grew a field** — a struct literal elsewhere will not compile until it names
  them: `ServerEntry::Full { …, voice: None, udp: vec![] }` (fixed in `tests/broker.rs:529`
  and `tests/egress.rs`), `NetnsSpec { …, ..Default::default() }` (fixed in
  `tests/sandbox.rs`, two places), `ServerRule.udp`, `Stats.udp_*` (built with `default()`).
* **`src/egress/mod.rs`, `relay_game`**: game connections that datagrams can follow are
  spliced by the new private `splice_anchored` instead of `splice` (section 4). Anyone else
  who changes how `relay_game` copies bytes has two functions to keep in step; the others
  (`relay_service`, connections without a UDP rule) are untouched.
* **`src/confine.rs` was not touched** (it is being changed for findings A-F1/A-F2). Its
  header says that the confined launcher still handles "DNS questions, TLS hellos, HTTP
  requests, log lines" from the game; with this package there is one more input: datagrams,
  from the game and from the server's address, relayed without being parsed. One clause to
  add there when the branches are merged.
* **Signatures**: `egress::start`, `Policy::outbound_ports`, `Policy::new` are unchanged.
  New public: `egress::udp::{Limits, Rate, …}`, `Policy::{udp_ports, sandbox_udp_ports,
  with_udp_limits}`, `egress::describe_proxy`, `netns::{SocketRole, check_socket}`,
  `NetnsSpec::sockets`, `fuzzing::udp`.
* **`src/launch.rs`**: three small blocks — the proxy message (now `egress::describe_proxy`),
  `landlock.udp_send` for filtered mode, and the `egress summary, UDP` line.
* **Shared lists**: `fuzzing::ALL` (+ `("udp", udp)`), `tests/robustness.rs` (the `tested`
  array, a `seeds` arm, one `#[test]`, one assertion in
  `the_starting_points_are_accepted_by_their_parsers`), `fuzz/Cargo.toml` (+ one `[[bin]]`),
  the key list in `config::tests::the_example_configuration_is_the_defaults` (+ `"voice = "`).
* **`tests/tls.rs`**: the stand-in SOCKS5 proxy moved, unchanged, to
  `tests/support/socks5.rs` and is included from both test files with `#[path]`.
* **If `confine.rs` changes** (review A-F1 proposes a seccomp filter there): the relay needs
  `socket`, `bind`, `connect`, `sendto`/`sendmsg`, `recvfrom`/`recvmsg` for UDP after
  confinement. `voice::the_relay_can_still_open_flows_…` will say so if that breaks.
* **Documentation to place**: section 1 into the README; section 2 into the threat model;
  `README` "Source map": `src/egress/` gains "`udp.rs` (datagram relay for voice chat)";
  the README's test count changes (this branch adds 38 tests to the launcher crate: 257
  `#[test]` functions at the baseline, 295 now; the workspace has 354).
* **An orphaned process of this work package is still running**: PID 742946 at the time of
  writing, `/usr/bin/bwrap --args 103 -- /bastion/init __sandbox-init 135`, started
  20:53:59 during the second engineer's parallel test runs, re-parented to `init`, no
  children, asleep. It is the inner half of a sandbox whose outer half was killed (see (2)
  below). Its working directory and two of its descriptors point into this
  clone (`readlink /proc/742946/cwd`, `ls -l /proc/742946/fd`), so it belongs to it; its
  command line does not contain the clone's path, and the rule for this machine is to kill
  only processes whose command line does, so it was left alone and is reported here.
  `kill 742946` removes it.
* **`docs/notes/broker-test-race.patch`** — a ready fix for flake (4) below, **not applied**
  on this branch: it changes only `tests/broker.rs` (the stand-in game server echoes) and
  `tests/java/BrokerProbe.java` (the probe waits for the echo, at most 15 s), which belong
  to the broker work. `git apply docs/notes/broker-test-race.patch` after the merge, or hand
  it to whoever owns those files; then delete the patch file.
* **Tests that existed before and fail now and then — four causes, none of them in this
  package's code.** If one of them fails during integration, run it again before looking
  for a reason in a branch.
* (1) `sandbox::resource_limits_kill_a_memory_hog_inside_the_scope` expects exit status 137
  and now and then sees a death by signal (`left: None, right: Some(137)`). It failed in
  one of the third engineer's four full runs of the workspace; the second measured 0 of 15
  alone, 5 of 40 with the two other `resource_limits_*` tests, 4 of 20 for the whole file. Cause, shown
  without any code of this project (`systemd-run --user --scope -p MemoryMax=128M -p
  MemorySwapMax=0 -- bwrap … python3` allocating 600 MiB): the user manager's
  `DefaultOOMPolicy=stop` applies to the scope, so after the kernel has killed the payload
  systemd stops the unit and sends SIGTERM to what is left of it — which races with
  bubblewrap's own `exit(137)`. Measured twice: one at a time 137 in 11 of 12 and 12 of 12
  runs; six at a time SIGTERM in 9 of 24 (second engineer), 12 of 24 and 5 of 24 (third).
  **With `-p OOMPolicy=continue` on the scope: 137 in 48 of 48 runs, six at a time.** That
  one property in `src/sandbox/cgroup.rs` should remove the flake, and it also makes a real
  launch report a memory kill as what it is. Not applied here: the file is not this
  package's, and the launcher's own reporting of that exit was not looked at. (The other
  way — let the test accept either outcome — hides the same race in a real launch.)
* (2) Sandboxes killed at start when many start at once: with four copies of a sandbox test
  binary running, now and then a payload ends with status 137 and no output, or a whole
  test binary is killed. The second engineer reproduced it **with plain bubblewrap and no
  code of this project** (`seq 1920 | xargs -P 48 …`: 2 of 1920; from Python threads 0 of
  480 at 1 and at 6 at a time, 2 of 480 at 24, 4 of 480 at 48). Who sends the signal was not
  found (no kernel log entry; no tracing tools here). A real launch starts one or two
  sandboxes and is far below that level; a sandbox test of any suite can fail this way when
  several suites run at the same moment, which five engineers on one machine do. Details:
  `voice-chat-progress.md`, section 4.
* (3) `tests/sandbox.rs` cannot run next to a second copy of itself:
  `kernel_keyring_is_unreachable_from_the_sandbox` uses the fixed key description
  `bastion-test-key` in the session keyring, so two runs replace and unlink each other's key.
* (4) `broker::a_sandboxed_jvm_holding_only_the_placeholder`
  fails now and then with `join_warm` = 403 instead of 204. Reproduced by the first
  engineer **at the baseline commit**, before any change of this branch: 10 failures in 80
  runs of that test alone (1 in 40 runs of the whole file on this branch). Seen once by the
  third engineer (`tests/broker.rs:649`), then 17 of 17 alone and 14 of 14 as a file; by
  the fourth, 1 of 40 runs alone. Cause: `BrokerProbe.java` opens the game
  connection and asks for the join at once; `new Socket(...)` returns when the kernel has
  queued the connection at the relay's listener, which can be before the relay has accepted
  it, connected to the server and registered it (`gate.opened`). A real client cannot hit
  this: it joins only after the server's reply has come back through that connection.
  **Not applied here** (the two files belong to the broker work of this round); **the fix
  is ready as `docs/notes/broker-test-race.patch`** — `git apply` it after merging: the
  stand-in game server in `tests/broker.rs` echoes what it reads, and the probe waits for
  the echo (at most 15 s) before `join_warm`; the echo can only arrive once the relay is
  splicing, which is after it registered the connection. Verified by widening the race on
  purpose (a 1 s delay injected in `relay_game` just before `gate.opened`, removed again
  afterwards): without the patch 4 of 4 runs failed with `join_warm` = 403, with it 4 of 4
  passed. Without the delay: 40 of 40 runs passed with the patch, 39 of 40 without it.

---------------------------------------------------------------------------------------------

## 8. "Where is the Fabric mod?"

There are two things that question can mean. Neither is in **this branch**; both are being
built in other work packages of the same round (`TODO.md`, the paragraph at the top).

* A **first-party companion Fabric mod** (the old to-do list had one as an in-game interface
  for approvals; `TODO.md` "Decided against / deferred" and THREAT_MODEL section 8 keep the
  reason why enforcement cannot live in it). The round's companion mod is another package.
  This one did not need a mod of ours: voice chat is a matter of the network path.
* **Simple Voice Chat** itself is a third-party mod (henkelmax, on Modrinth). In this branch
  the launcher does not download mods; the user adds the jar to a signed profile. Verified
  installs from Modrinth are another package of the round. **The two meet at one point that
  nobody owns yet:** installing the mod does not open a UDP port, and must not — the port
  belongs to a *server's* rule. Whoever integrates may want the installer to say so when
  it installs a voice mod ("add `voice = 24454` to the rule of the server you play on").
  For the check in E17 the jar was downloaded into this work package's scratch directory
  (`target/scratch/svc-jar/voicechat-fabric-2.6.27+26.3.jar`, SHA-512
  `f2de6676…5f6df27e` as reported by Modrinth's API, compared twice) and installed into a
  test instance `fabvoice` — a copy of `fab` with the jar added, locked and signed with the
  test key — in this clone's own data root
  (`target/e2e/data/bastion-mc/instances/fabvoice`). Both live under `target/`, which is not
  committed, in a clone that is thrown away; the user's real data root was not touched. To
  repeat the check there: copy the jar into a curated instance's `mods/`, `profile lock`,
  `profile sign`, add `voice = 24454` to the server's rule, launch. (For E19 the rule in
  that instance's `instance.toml` pointed at stand-in servers for the length of the run;
  the file is back to what the first engineer wrote. The scripts are in
  `target/scratch/t3/real/`.)
