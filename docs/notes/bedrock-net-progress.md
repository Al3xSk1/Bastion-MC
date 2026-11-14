# bedrock-net — hand-over file (network package, round 5)

Resume from here if a session was interrupted. The finished write-up is `bedrock-net.md`.

## State (2026-11-14, end of session)

Done, committed on `work`, all verified (see `bedrock-net.md` §3):

- [x] RakNet offline-message parser, encoders, harness (`src/egress/raknet.rs`).
- [x] Bedrock profile in `egress::Policy` (`src/egress/bedrock.rs`): exact/suffix/deny SNI, RakNet
      server rules, the join pass; Java paths unchanged.
- [x] UDP relay generalised (`src/egress/udp.rs`): standing anchors per server rule (names
      resolved on the host, retried), pass flows (four per approval within 30 s), the RakNet
      opening check, `Limits::raknet`, Bedrock wording, a decisions harness.
- [x] Service broker (`src/egress/services.rs`): Realms + gatherings TLS termination, strict join
      parsers, rewrite to the pass, approvals, NetherNet/private/host-name refusals, withheld
      look-alikes, no tokens or queries in the log.
- [x] Entry point `bedrock::net::{prepare, Prepared::start, answer_join, describe_proxy}`.
- [x] `tests/bedrock_net.rs` (7 end-to-end tests incl. a confined child); unit tests; harnesses
      `raknet`, `bedrock_realms`, `bedrock_gatherings` (+ robustness tests, fuzz targets).
- [x] `cargo fmt`, `cargo clippy --workspace --all-targets` clean, `cargo test --workspace`
      817 passed / 0 failed, `cargo deny` ok, notes written.

Next (for whoever continues): the integrator wires `prepare`/`start` into the launch package's
Bedrock dispatch and `answer_join` into the sign-in package's `answer realms` reader
(`bedrock-net.md` §5); everything in §3 "Not verified" needs the user's real client and account.

## Research findings (with sources)

- RakNet (go-raknet 0d1fd09, fetched to `target/r5net-research/go-raknet/`, read only): protocol
  version 11; offline magic `00 ff ff 00 fe fe fe fe fd fd fd fd 12 34 56 78`; MTU probes
  1492, 1200, 576 (`dial.go` `mtuSizes`); OCR1 payload = MTU − 28 (`open_connection_request_1.go`),
  so the largest datagram of an MTU discovery is 1464 bytes; a client accepts a server MTU up to
  1500 (`dial.go` discoverMTU) → largest data datagram 1472.
- Bedrock Realms (gophertunnel `minecraft/realms`): base `https://bedrock.frontendlegacy.realms.minecraft-services.net`,
  `GET /worlds` → `{"servers":[{"id","name",…}]}`, `GET /worlds/{id}/join` → `{"address":"IP:port",
  "networkProtocol":"DEFAULT"|"NETHERNET"|"NETHERNET_JSONRPC","pendingUpdate",
  "sessionRegionData":{…}}`, 503 while the Realm starts (client polls every 3 s), auth
  `Authorization: XBL3.0 x=…` for RP `https://pocket.realms.minecraft.net/` (the game's own token).
- Gatherings (gophertunnel `minecraft/service/gatherings`): default host
  `gatherings-secondary.franchise.minecraft-services.net` (real one from discovery),
  `POST /api/v2.0/join/experience` `{"experienceId": uuid}` and `POST /api/v2.0/join/world/client`
  → `{"result":{"networkProtocol":"Default"|"NetherNet"|"NetherNet_JsonRpc","ipV4Address","port",
  "destinationInfo":{…,"experienceName",…}}}`; auth `Authorization: MCToken …`.
- Client TLS (mcpelauncher-client 7890498): all HTTP and WebSocket traffic is `curl_easy_*` in
  `src/jni/lib_http_client.cpp` / `lib_http_client_websocket.cpp`, no CAINFO/CAPATH/VERIFY*/PINNED
  option anywhere; the Java-side certificate classes (`src/jni/cert_manager.cpp`) are empty stubs.
  NOT the system libcurl by default: the manifest builds curl 8.21.0 statically (`USE_OWN_CURL=ON`,
  `ext/curl.cmake`, downloaded without a checksum) and the AUR PKGBUILD mcpelauncher-linux 1.8.7-1
  does not turn that off; curl 8.21.0's CMake autodetects the CA bundle `/etc/ssl/certs/ca-certificates.crt`
  first (fetched CMakeLists, lines 1465–1490) — a path the sandbox's generated trust store covers.

## Decisions so far

- Bedrock RakNet flows anchor to a server rule (standing anchor once the name resolved on the host,
  or the literal), never to TCP; Realms/event joins get a UDP pass `127.127.255.253:19132`.
- No private/loopback Bedrock servers in this version (BedrockServer has no `allow_private`).
- `[realms] approve = "totp"` acts as `"code"` for Bedrock (the TOTP secret is in the token store,
  which a Bedrock session never opens).
