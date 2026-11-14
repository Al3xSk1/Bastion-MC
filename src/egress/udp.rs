//! Datagram relay for allowlisted game servers ("voice chat").
//!
//! Voice-chat mods carry sound as UDP datagrams from the client to the game server's host,
//! on a port of their own (Simple Voice Chat: 24454 unless the server says otherwise). In
//! filtered mode the sandbox has no UDP except DNS to the launcher, so such a mod stays
//! silent. This module is the narrowest path that lets it work; it is not general UDP
//! egress, and it does not know or care what the datagrams contain.
//!
//! * **Opt-in per server rule.** A rule may name UDP ports (`voice = 24454`). For each named
//!   port the namespace helper binds one wildcard UDP socket inside the sandbox's network
//!   namespace and hands it to the launcher; Landlock (ABI 10) lets the sandbox send to those
//!   ports and to DNS, nothing else. No rule, no socket, no change from before.
//! * **No new destinations.** A datagram is relayed only while a relayed TCP game connection
//!   made under the same rule (same host, same TCP port) is open, and it is sent to the
//!   address *that connection* is using, on the UDP port the game addressed. The relay never
//!   resolves a name for a datagram: there is no second lookup that a network attacker could
//!   answer differently from the first (DNS rebinding between game and voice), and no way to
//!   send to a host the game has no connection to. The private-address rule of the TCP relay
//!   is applied again, with the `allow_private` of a rule that names the port.
//! * **Open means open in both directions.** Once either the game or the server has hung
//!   up, nothing follows that connection any more, however long the other side keeps its
//!   end (`splice_anchored` in the parent module): a server that never closes does not keep
//!   the path open after the player has left, and the game cannot keep it open after the
//!   server has hung up by holding on to a socket.
//! * **Replies only, and only to the asker.** Every flow — one source address and port in
//!   the sandbox, one destination — gets a socket of its own on the host side, `connect`ed
//!   to the server, so the kernel discards datagrams from any other address. What arrives
//!   on it goes back to that flow's source and to nobody else, from the address the game
//!   had sent to: the launcher learns that address from `IP_PKTINFO` when a datagram comes
//!   in and names it the same way when one goes back, so the game sees its answer come from
//!   where it sent the question.
//! * **Bounded** ([`Limits`]): few flows, an idle timeout counted from the game's last
//!   datagram, a maximum datagram size, and a rate limit per flow and direction that leaves
//!   room for voice (50 datagrams a second per speaker) and is useless for flooding.
//! * **Logged like the TCP relay**: `ALLOW` when a flow opens, `done` with its counts when
//!   it ends, `DENY` once per kind of refusal and destination — never per datagram. Every
//!   datagram is counted in the statistics, relayed or refused.
//!
//! What this is **not**: proof that the user is playing. "A game connection is open" is
//! something code in the game can arrange by itself for as long as the server lets a
//! connection idle. The tie guarantees *where* datagrams can go — an address the game
//! already exchanges arbitrary bytes with over TCP — not *why* they are sent. See
//! `docs/notes/voice-chat.md` for the threat-model consequences.
//!
//! **Bedrock** (a [`Policy::bedrock`] session) has no TCP game connection to tie flows to: the
//! game speaks RakNet over UDP to the server itself. There the anchor is the server rule: once
//! the launcher has resolved the rule's name (or at once, for a literal address), a *standing*
//! anchor holds that address for the session, and flows to the rule's port go there and
//! nowhere else — still no lookup per datagram, still one host-side socket per flow,
//! `connect`ed, replies from the address the game sent to. Three things are added: a flow opens
//! only with a datagram that starts a RakNet exchange (`raknet::opens_flow`); the limits are
//! RakNet's ([`Limits::raknet`]: datagrams of an MTU discovery, gameplay rates); and a flow to
//! the **join pass** (`bedrock::PASS`) goes to the one server whose Realms or event-server join
//! the user approved outside the game — the first such flow spends the approval
//! (`realms::approval::Approvals::take_pass`), and at most three more flows of the same join, to
//! the same server and within 30 s, follow it (`services::Services::take_pass`). Without Landlock
//! ABI 10 the sandbox may send UDP to any port of any address inside its namespace, but the
//! only sockets there besides its own are these; the namespace still has no route out.
//!
//! Decisions and I/O are kept apart. [`Table`] decides, from addresses, sizes and times
//! alone, and is what the unit tests and the generated-input harness drive; the functions
//! below it own the sockets and do what the table said.

use std::{
    collections::{HashMap, HashSet},
    io::{IoSlice, IoSliceMut},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4},
    os::fd::{AsRawFd, OwnedFd},
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

use nix::sys::socket::{
    ControlMessage, ControlMessageOwned, MsgFlags, SockaddrIn, recvmsg, send, sendmsg, setsockopt, sockopt,
};
use tokio::{
    io::Interest,
    net::UdpSocket,
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::timeout,
};

use super::{Dest, FakeIps, Policy, Shared, is_public, raknet};
use crate::{
    config::{HostSpec, ServerRule},
    error::{Context, Result},
};

/// Most UDP ports all server rules together may name: each is a socket and a task.
pub const MAX_PORTS: usize = 16;
/// Refusals are logged once per destination and kind, and for each kind for this many
/// destinations at most. Per kind, because for two of them the game chooses the destination
/// freely (any address that is no server): a common budget would let it use up, with noise,
/// the lines that say something about a real server.
const MAX_NOTED_PER_KIND: usize = 16;

// ---------------------------------------------------------------- limits

/// How much the relay carries. [`Limits::default`] is what every launch runs with; nothing
/// in a configuration file changes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Flows that may exist at once, over all ports. A voice client needs one.
    pub max_flows: usize,
    /// A flow on which the game has sent nothing for this long is closed. Datagrams from
    /// the server do not keep a flow alive: a flow exists because the game uses it.
    pub idle: Duration,
    /// Largest datagram relayed, in bytes of UDP payload, in either direction.
    pub max_datagram: usize,
    /// Per flow, from the game to the server.
    pub to_server: Rate,
    /// Per flow, from the server to the game.
    pub to_game: Rate,
}

/// A token bucket: `per_second` datagrams sustained, `burst` at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub per_second: u32,
    pub burst: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_flows: 8,
            idle: Duration::from_secs(30),
            // Fits one Ethernet frame over IPv4 and IPv6 with room to spare, and is larger
            // than what voice mods send. Simple Voice Chat 2.6.27 limits the Opus payload of
            // a packet to the server's `mtu_size` — 1275 bytes unless the server says
            // otherwise, which is the largest Opus frame there is — and its framing and
            // encryption add well under a hundred (read from its class files, not
            // measured). An ordinary voice frame is a fraction of that.
            max_datagram: 1400,
            // One microphone is 50 datagrams a second (20 ms frames) plus keep-alives:
            // twice that. At the size limit this is 140 kB/s per flow, 1.1 MB/s over all
            // eight flows — to a server the game may already send to over TCP without limit.
            to_server: Rate { per_second: 100, burst: 100 },
            // The server forwards one stream per audible speaker: ten of them at once.
            to_game: Rate { per_second: 500, burst: 500 },
        }
    }
}

impl Limits {
    /// What a Bedrock session's relay carries: RakNet to the servers the user listed. Like
    /// [`Limits::default`], nothing in a configuration file changes it.
    pub fn raknet() -> Self {
        Limits {
            // The game pings every server on its list (one flow per server and socket) besides
            // the one it plays on; a list of a few dozen servers fits.
            max_flows: 32,
            // A RakNet connection is never silent that long: both sides ping every few seconds.
            idle: Duration::from_secs(30),
            // Datagrams of an MTU discovery and what a server that negotiated MTU 1500 sends.
            max_datagram: raknet::MAX_DATAGRAM,
            // The client sends its input every tick (20 a second), acknowledgements for what it
            // receives (RakNet's update loop, at most about a hundred a second) and the odd
            // larger packet: 500 a second leaves room for all of it. At the size limit that is
            // 736 kB/s — to a server the user listed, which the game may tell anything anyway.
            to_server: Rate { per_second: 500, burst: 500 },
            // A server pours out chunks when the game joins: 4000 datagrams a second (5.9 MB/s at
            // the size limit) per flow. More is dropped, and RakNet's own congestion control
            // resends it: slower, not broken.
            to_game: Rate { per_second: 4000, burst: 4000 },
        }
    }
}

struct Bucket {
    tokens: u32,
    stamp: Instant,
    /// Earned but not yet a whole token, in token-nanoseconds.
    carry: u64,
}

impl Bucket {
    fn full(rate: Rate, now: Instant) -> Self {
        Bucket { tokens: rate.burst, stamp: now, carry: 0 }
    }

    /// Take one token if there is one. A clock that does not advance earns nothing.
    fn take(&mut self, rate: Rate, now: Instant) -> bool {
        const NANOS: u128 = 1_000_000_000;
        let elapsed = now.saturating_duration_since(self.stamp);
        self.stamp = self.stamp.max(now);
        let credit = elapsed.as_nanos().saturating_mul(u128::from(rate.per_second)) + u128::from(self.carry);
        let tokens = u128::from(self.tokens) + credit / NANOS;
        if tokens >= u128::from(rate.burst) {
            // A full bucket earns nothing: an idle second is worth one burst, not more.
            (self.tokens, self.carry) = (rate.burst, 0);
        } else {
            (self.tokens, self.carry) = (tokens as u32, (credit % NANOS) as u64);
        }
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }
}

// ---------------------------------------------------------------- decisions

/// Why a datagram was not relayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// Sent to a synthetic address that the resolver never handed out for any name.
    Unassigned,
    /// The destination is no allowlisted game server: a Mojang service name, or a literal
    /// address that no rule names.
    NotAServer,
    /// The server is allowlisted, but no rule for it names a UDP port.
    NoUdpRule,
    /// Rules for this server name UDP ports, but not this one.
    PortNotListed,
    /// No relayed game connection made under a rule that names this port is open.
    NoGameConnection,
    /// The game connection uses a non-public address and the rule lacks `allow_private`.
    PrivateAddress,
    /// Larger than [`Limits::max_datagram`].
    Oversize,
    /// [`Limits::max_flows`] flows exist already.
    FlowLimit,
    /// The flow's rate limit towards the server.
    RateToServer,
    /// The flow's rate limit towards the game.
    RateToGame,
    /// The operating system would not: no socket could be opened, or the datagram could not
    /// be queued.
    Io,
    /// Not a refusal by the relay: the server's side answered a datagram that had been sent
    /// on with an ICMP error (nothing listens on that port, or the host cannot be reached).
    /// Logged like one, once. The kernel reports such an error on the flow's next socket
    /// call, whichever that is. On a receive, nothing more is lost and nothing is counted:
    /// the datagram was counted when it left. On a send, that send fails and its datagram
    /// stays here: that one is counted as refused (see [`unsent_because`]).
    Unreachable,
    /// From the server, for a flow that has ended in the meantime. Counted, never logged.
    Stale,
    /// Bedrock: the first datagram of a flow was not a RakNet ping or open connection request.
    NotRakNet,
    /// Bedrock: to the join pass, which no approved join has armed, or whose join was used or
    /// ran out (the reason is the approval's own).
    Pass(&'static str),
}

impl Refusal {
    fn text(self, limits: &Limits, bedrock: bool) -> String {
        if bedrock && let Some(text) = self.bedrock_text() {
            return text.into();
        }
        match self {
            Refusal::Unassigned => "address was never handed out by the resolver".into(),
            Refusal::NotAServer => "not an allowlisted game server".into(),
            Refusal::NoUdpRule => "the rule for this server names no UDP port".into(),
            Refusal::PortNotListed => "this UDP port is not named by a rule for this server".into(),
            Refusal::NoGameConnection => {
                "no game connection to this server is open; datagrams follow a game connection, they never precede one"
                    .into()
            }
            Refusal::PrivateAddress => {
                "the game connection uses a non-public address; set allow_private on the rule that names the UDP port"
                    .into()
            }
            Refusal::NotRakNet => {
                "the first datagram of a flow must be a RakNet ping or open connection request".into()
            }
            Refusal::Pass(why) => format!("join pass: {why}"),
            Refusal::Oversize => format!("datagram larger than {} bytes", limits.max_datagram),
            Refusal::FlowLimit => format!("{} flows are open already", limits.max_flows),
            Refusal::RateToServer => format!(
                "rate limit: more than {} datagrams a second from the game on one flow",
                limits.to_server.per_second
            ),
            Refusal::RateToGame => {
                format!(
                    "rate limit: more than {} datagrams a second from the server on one flow",
                    limits.to_game.per_second
                )
            }
            Refusal::Io => "the operating system could not send it on (no socket, or a full buffer)".into(),
            Refusal::Unreachable => {
                "the server's address reports that datagrams to this port do not arrive (nothing listens there?)".into()
            }
            Refusal::Stale => "the flow has ended".into(),
        }
    }

    /// How a Bedrock session says it, where that differs: phrased so that the user knows what to
    /// add to `[bedrock] servers`, or that nothing can be added.
    fn bedrock_text(self) -> Option<&'static str> {
        Some(match self {
            Refusal::NotAServer => {
                "not a Bedrock server you listed: if it is one, add { host = \"…\", port = … } to [bedrock] servers"
            }
            Refusal::PortNotListed => {
                "this server is listed with another port: list it with this port as well in [bedrock] servers"
            }
            Refusal::NoGameConnection => {
                "the launcher has not resolved this server's name (yet): see the line about its lookup"
            }
            Refusal::PrivateAddress => {
                "the server is at a non-public address: LAN servers are not supported for Bedrock in this version"
            }
            _ => return None,
        })
    }
}

/// The anchor id of a flow to the join pass: no anchor (such a flow ends on its own).
const PASS_ANCHOR: u64 = u64::MAX;

/// One flow: a socket of the game (address and port inside the sandbox) and where it sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowKey {
    pub game: SocketAddrV4,
    pub dest: SocketAddrV4,
}

/// An open relayed game connection: what a flow is tied to.
struct Anchored {
    host: HostSpec,
    tcp_port: u16,
    /// The address the game connection is really using (after SRV and name resolution).
    peer: SocketAddr,
    label: String,
    /// Closed when the connection ends.
    closed: watch::Receiver<()>,
}

struct Flow<U> {
    id: u64,
    anchor: u64,
    label: Arc<str>,
    target: SocketAddr,
    upstream: U,
    /// When the game last sent a datagram that was relayed.
    heard: Instant,
    to_server: Bucket,
    to_game: Bucket,
    up: u64,
    down: u64,
    refused: u64,
    /// A flow to the join pass: what its task waits on, dropped when the flow ends.
    _pass: Option<watch::Sender<()>>,
}

/// A flow that was just opened.
pub struct Opened {
    pub id: u64,
    pub label: Arc<str>,
    /// Where the game connection it is tied to goes, as logged for that connection.
    pub tied_to: String,
    pub target: SocketAddr,
    closed: watch::Receiver<()>,
}

/// A flow that has ended, with what it carried.
#[derive(Debug, PartialEq, Eq)]
pub struct Ended {
    pub label: Arc<str>,
    pub up: u64,
    pub down: u64,
    pub refused: u64,
}

/// What to do with a datagram from the game.
pub enum Verdict<U> {
    /// Send it through `upstream`. `opened` is set for the first datagram of a flow.
    Relay {
        upstream: U,
        flow: u64,
        label: Arc<str>,
        opened: Option<Opened>,
    },
    Refuse {
        label: Arc<str>,
        why: Refusal,
    },
}

/// Every decision of the relay, with no socket in sight: the caller says what arrived, from
/// where, how large and when. `U` is whatever sends towards the server — a socket in the
/// launcher, the bare target address in the tests.
pub struct Table<U> {
    anchors: HashMap<u64, Anchored>,
    flows: HashMap<FlowKey, Flow<U>>,
    /// Game connections registered so far; the newest has the highest number.
    anchors_opened: u64,
    /// Flows opened so far. A flow's number is never reused, so whatever still refers to a
    /// flow that has ended (its task, a late datagram) cannot touch a later one that has
    /// the same addresses.
    flows_opened: u64,
}

impl<U> Default for Table<U> {
    fn default() -> Self {
        Table { anchors: HashMap::new(), flows: HashMap::new(), anchors_opened: 0, flows_opened: 0 }
    }
}

impl<U: Clone> Table<U> {
    /// Register a relayed game connection made under a rule for `host`, TCP port `tcp_port`,
    /// that reached `peer`. Returns its id and the handle whose drop marks it closed.
    pub fn open_anchor(
        &mut self,
        host: HostSpec,
        tcp_port: u16,
        peer: SocketAddr,
        label: &str,
    ) -> (u64, watch::Sender<()>) {
        let (open, closed) = watch::channel(());
        self.anchors_opened += 1;
        self.anchors.insert(self.anchors_opened, Anchored { host, tcp_port, peer, label: label.to_string(), closed });
        (self.anchors_opened, open)
    }

    /// The game connection ended: so do the flows that were tied to it.
    pub fn close_anchor(&mut self, id: u64) -> Vec<Ended> {
        self.anchors.remove(&id);
        let tied: Vec<FlowKey> = self.flows.iter().filter(|(_, f)| f.anchor == id).map(|(k, _)| *k).collect();
        tied.into_iter().filter_map(|key| self.flows.remove(&key)).map(Flow::ended).collect()
    }

    /// A datagram of `len` bytes on the flow `key`, if that flow exists: within the size and rate
    /// limits it goes through. A flow that exists was admitted under the policy, and its anchor
    /// is open: closing that anchor removes the flow under the same lock.
    fn on_flow(&mut self, now: Instant, limits: &Limits, key: &FlowKey, len: usize) -> Option<Verdict<U>> {
        let flow = self.flows.get_mut(key)?;
        let refuse = |flow: &mut Flow<U>, why| {
            flow.refused += 1;
            Verdict::Refuse { label: flow.label.clone(), why }
        };
        if len > limits.max_datagram {
            return Some(refuse(flow, Refusal::Oversize));
        }
        if !flow.to_server.take(limits.to_server, now) {
            return Some(refuse(flow, Refusal::RateToServer));
        }
        flow.heard = now;
        flow.up += 1;
        Some(Verdict::Relay { upstream: flow.upstream.clone(), flow: flow.id, label: flow.label.clone(), opened: None })
    }

    /// A datagram of `len` bytes from the game. `dest_of` says what a destination address
    /// stands for (see `Shared::dest_of`); `open` creates the way to the server for a new flow.
    /// `opens`: this datagram may start a flow (always for Java; for Bedrock only a RakNet ping
    /// or open connection request — `raknet::opens_flow`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn game_datagram(
        &mut self,
        now: Instant,
        limits: &Limits,
        policy: &Policy,
        key: FlowKey,
        len: usize,
        opens: bool,
        dest_of: impl FnOnce(Ipv4Addr) -> Option<Dest>,
        open: impl FnOnce(SocketAddr) -> std::io::Result<U>,
    ) -> Verdict<U> {
        let oversize = len > limits.max_datagram;
        if let Some(verdict) = self.on_flow(now, limits, &key, len) {
            return verdict;
        }

        // No flow yet. May there be one?
        let port = key.dest.port();
        let Some(dest) = dest_of(*key.dest.ip()) else {
            return Verdict::Refuse { label: key.dest.to_string().into(), why: Refusal::Unassigned };
        };
        let label: Arc<str> = match &dest {
            Dest::Name(name) => format!("{name}:{port}"),
            Dest::Ip(ip) => format!("{ip}:{port}"),
        }
        .into();
        let refuse = |why| Verdict::Refuse { label: label.clone(), why };
        let rules = match policy.udp_rules(&dest, port) {
            Ok(rules) => rules,
            Err(why) => return refuse(why),
        };
        // The newest open game connection made under one of those rules, to an address the
        // rule permits. Its address is where the datagrams go: nothing is resolved here.
        let (mut newest, mut private) = (None, false);
        for (id, anchor) in &self.anchors {
            let mut under = rules.iter().filter(|r| r.host == anchor.host && r.port == anchor.tcp_port).peekable();
            if under.peek().is_none() {
                continue;
            }
            if is_public(anchor.peer.ip()) || under.any(|r| r.allow_private) {
                if newest.is_none_or(|(other, _)| *id > other) {
                    newest = Some((*id, anchor));
                }
            } else {
                private = true;
            }
        }
        let Some((anchor_id, anchor)) = newest else {
            return refuse(if private { Refusal::PrivateAddress } else { Refusal::NoGameConnection });
        };
        if oversize {
            return refuse(Refusal::Oversize);
        }
        if !opens {
            return refuse(Refusal::NotRakNet);
        }
        if self.flows.len() >= limits.max_flows {
            return refuse(Refusal::FlowLimit);
        }
        let mut target = anchor.peer;
        target.set_port(port);
        let Ok(upstream) = open(target) else { return refuse(Refusal::Io) };
        let opened = Opened {
            id: self.flows_opened + 1,
            label: label.clone(),
            tied_to: anchor.label.clone(),
            target,
            closed: anchor.closed.clone(),
        };
        self.flows_opened += 1;
        let mut to_server = Bucket::full(limits.to_server, now);
        to_server.take(limits.to_server, now); // this datagram
        self.flows.insert(
            key,
            Flow {
                id: opened.id,
                anchor: anchor_id,
                label: label.clone(),
                target,
                upstream: upstream.clone(),
                heard: now,
                to_server,
                to_game: Bucket::full(limits.to_game, now),
                up: 1,
                down: 0,
                refused: 0,
                _pass: None,
            },
        );
        Verdict::Relay { upstream, flow: opened.id, label, opened: Some(opened) }
    }

    /// A datagram of `len` bytes from the game to the join pass (Bedrock). A flow that exists
    /// goes on as any flow does. A new one needs a datagram that may open a flow, room for it,
    /// and then `take` — which spends the approval and says where the approved server is —
    /// succeeding; nothing is spent on a datagram that would be refused anyway. The flow has no
    /// anchor: it ends when the game falls silent, and the approval is not given back.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pass_datagram(
        &mut self,
        now: Instant,
        limits: &Limits,
        key: FlowKey,
        len: usize,
        opens: bool,
        take: impl FnOnce() -> std::result::Result<(SocketAddr, String), Refusal>,
        open: impl FnOnce(SocketAddr) -> std::io::Result<U>,
    ) -> Verdict<U> {
        if let Some(verdict) = self.on_flow(now, limits, &key, len) {
            return verdict;
        }
        let label: Arc<str> = format!("{} (join pass)", key.dest).into();
        let refuse = |why| Verdict::Refuse { label: label.clone(), why };
        if len > limits.max_datagram {
            return refuse(Refusal::Oversize);
        }
        if !opens {
            return refuse(Refusal::NotRakNet);
        }
        if self.flows.len() >= limits.max_flows {
            return refuse(Refusal::FlowLimit);
        }
        let (target, tied_to) = match take() {
            Ok(found) => found,
            Err(why) => return refuse(why),
        };
        let Ok(upstream) = open(target) else { return refuse(Refusal::Io) };
        let (keep, closed) = watch::channel(());
        self.flows_opened += 1;
        let opened = Opened { id: self.flows_opened, label: label.clone(), tied_to, target, closed };
        let mut to_server = Bucket::full(limits.to_server, now);
        to_server.take(limits.to_server, now); // this datagram
        self.flows.insert(
            key,
            Flow {
                id: opened.id,
                anchor: PASS_ANCHOR,
                label: label.clone(),
                target,
                upstream: upstream.clone(),
                heard: now,
                to_server,
                to_game: Bucket::full(limits.to_game, now),
                up: 1,
                down: 0,
                refused: 0,
                _pass: Some(keep),
            },
        );
        Verdict::Relay { upstream, flow: opened.id, label, opened: Some(opened) }
    }

    /// A datagram of `len` bytes from the server, received on the socket of flow `id`.
    pub fn server_datagram(
        &mut self,
        now: Instant,
        limits: &Limits,
        key: &FlowKey,
        id: u64,
        len: usize,
    ) -> std::result::Result<(), Refusal> {
        let Some(flow) = self.flows.get_mut(key).filter(|f| f.id == id) else { return Err(Refusal::Stale) };
        let why = if len > limits.max_datagram {
            Refusal::Oversize
        } else if !flow.to_game.take(limits.to_game, now) {
            Refusal::RateToGame
        } else {
            flow.down += 1;
            return Ok(());
        };
        flow.refused += 1;
        Err(why)
    }

    /// A datagram that was admitted could not be handed to the kernel after all.
    pub fn unsent(&mut self, key: &FlowKey, id: u64, towards_server: bool) {
        if let Some(flow) = self.flows.get_mut(key).filter(|f| f.id == id) {
            let counter = if towards_server { &mut flow.up } else { &mut flow.down };
            *counter = counter.saturating_sub(1);
            flow.refused += 1;
        }
    }

    /// When flow `id` will have been idle for too long, unless the game sends again.
    /// `None`: there is no such flow (any more).
    pub fn idle_deadline(&self, limits: &Limits, key: &FlowKey, id: u64) -> Option<Instant> {
        self.flows.get(key).filter(|f| f.id == id).map(|f| f.heard + limits.idle)
    }

    /// End flow `id`, if it still exists.
    pub fn end(&mut self, key: &FlowKey, id: u64) -> Option<Ended> {
        if self.flows.get(key).is_some_and(|f| f.id == id) { self.flows.remove(key).map(Flow::ended) } else { None }
    }

    pub fn end_all(&mut self) -> Vec<Ended> {
        self.flows.drain().map(|(_, flow)| flow.ended()).collect()
    }

    pub fn flow_count(&self) -> usize {
        self.flows.len()
    }

    /// Where flow `key` sends to, and the id of the game connection it is tied to.
    pub fn flow_target(&self, key: &FlowKey) -> Option<(SocketAddr, u64)> {
        self.flows.get(key).map(|f| (f.target, f.anchor))
    }

    /// The address game connection `id` uses, while it is open.
    pub fn anchor_peer(&self, id: u64) -> Option<SocketAddr> {
        self.anchors.get(&id).map(|a| a.peer)
    }
}

impl<U> Flow<U> {
    fn ended(self) -> Ended {
        Ended { label: self.label, up: self.up, down: self.down, refused: self.refused }
    }
}

// ---------------------------------------------------------------- sockets

/// One of the wildcard UDP sockets inside the sandbox's network namespace: every datagram
/// the game sends to this port, whatever address it names, arrives here.
pub(super) struct GameSocket {
    socket: UdpSocket,
    port: u16,
}

struct Arrival {
    len: usize,
    /// The datagram was larger than the buffer; `len` is what fitted.
    truncated: bool,
    from: SocketAddrV4,
    /// The address it was sent to. `None`: the kernel did not say (never seen; refused).
    to: Option<Ipv4Addr>,
}

impl GameSocket {
    pub(super) fn new(fd: OwnedFd, port: u16) -> Result<Self> {
        // With every datagram, the address it was sent to: on a wildcard socket nothing
        // else says which server the game meant. Without it nothing could be relayed, so
        // failing to enable it fails the launch.
        setsockopt(&fd, sockopt::Ipv4PacketInfo, &true).ctx(|| "enabling packet information on a relay socket")?;
        let socket = std::net::UdpSocket::from(fd);
        socket.set_nonblocking(true).ctx(|| "configuring a relay socket")?;
        Ok(GameSocket { socket: UdpSocket::from_std(socket).ctx(|| "registering a relay socket")?, port })
    }

    async fn receive(&self, buf: &mut [u8]) -> std::io::Result<Arrival> {
        let fd = self.socket.as_raw_fd();
        self.socket
            .async_io(Interest::READABLE, || {
                let mut space = nix::cmsg_space!(libc::in_pktinfo);
                let mut iov = [IoSliceMut::new(&mut *buf)];
                let message = recvmsg::<SockaddrIn>(fd, &mut iov, Some(&mut space), MsgFlags::MSG_DONTWAIT)?;
                let from = message
                    .address
                    .map(SocketAddrV4::from)
                    .ok_or_else(|| std::io::Error::other("a datagram without a source address"))?;
                // A control buffer that was too small is reported as an error by `cmsgs`.
                let to = message.cmsgs().ok().and_then(|mut all| {
                    all.find_map(|c| match c {
                        ControlMessageOwned::Ipv4PacketInfo(info) => {
                            Some(Ipv4Addr::from(info.ipi_addr.s_addr.to_ne_bytes()))
                        }
                        _ => None,
                    })
                });
                Ok(Arrival { len: message.bytes, truncated: message.flags.contains(MsgFlags::MSG_TRUNC), from, to })
            })
            .await
    }

    /// Send `data` to `to`, from this port and from address `from` — the one the game had
    /// sent to. Never waits: a datagram that cannot be queued now is lost, as datagrams are.
    fn send(&self, data: &[u8], from: Ipv4Addr, to: SocketAddrV4) -> std::io::Result<()> {
        let info = libc::in_pktinfo {
            ipi_ifindex: 0,
            ipi_spec_dst: libc::in_addr { s_addr: u32::from_ne_bytes(from.octets()) },
            ipi_addr: libc::in_addr { s_addr: 0 },
        };
        sendmsg(
            self.socket.as_raw_fd(),
            &[IoSlice::new(data)],
            &[ControlMessage::Ipv4PacketInfo(&info)],
            MsgFlags::MSG_DONTWAIT,
            Some(&SockaddrIn::from(to)),
        )?;
        Ok(())
    }
}

/// A socket of this flow's own on the host side, connected to the server: the kernel hands
/// it datagrams from that address and port and from nowhere else.
fn open_upstream(target: SocketAddr) -> std::io::Result<Arc<UdpSocket>> {
    let any: SocketAddr = match target {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = std::net::UdpSocket::bind(any)?;
    socket.connect(target)?;
    socket.set_nonblocking(true)?;
    Ok(Arc::new(UdpSocket::from_std(socket)?))
}

// ---------------------------------------------------------------- the relay

/// The relay's state for one launch.
pub(super) struct Relay {
    limits: Limits,
    table: Mutex<Table<Arc<UdpSocket>>>,
    /// Refusals already written to the log: for each kind, the destinations.
    noted: Mutex<HashMap<Refusal, HashSet<Arc<str>>>>,
    /// A Bedrock session: flows open only with RakNet, and refusals are phrased for
    /// `[bedrock] servers`.
    bedrock: bool,
    /// Bedrock: the standing anchors of the server rules, held for the session.
    standing: Mutex<Vec<watch::Sender<()>>>,
}

impl Relay {
    pub(super) fn new(limits: Limits, bedrock: bool) -> Self {
        Relay {
            limits,
            table: Mutex::new(Table::default()),
            noted: Mutex::new(HashMap::new()),
            bedrock,
            standing: Mutex::new(Vec::new()),
        }
    }

    fn table(&self) -> std::sync::MutexGuard<'_, Table<Arc<UdpSocket>>> {
        self.table.lock().expect("udp relay lock")
    }
}

/// Count a refused datagram; say so in the log the first time for this destination and kind.
fn refuse(shared: &Shared, relay: &Relay, label: &Arc<str>, why: Refusal) {
    shared.stats.udp_refused.fetch_add(1, Ordering::Relaxed);
    if why != Refusal::Stale {
        note(shared, relay, label, why);
    }
}

/// One line in the log the first time something happens for this destination, and for
/// each kind of event for a bounded number of destinations.
fn note(shared: &Shared, relay: &Relay, label: &Arc<str>, why: Refusal) {
    let line = {
        let mut noted = relay.noted.lock().expect("udp log lock");
        let of_kind = noted.entry(why).or_default();
        if of_kind.len() >= MAX_NOTED_PER_KIND || !of_kind.insert(label.clone()) {
            return;
        }
        let last = if of_kind.len() == MAX_NOTED_PER_KIND {
            "; further destinations refused for this reason are counted, not logged"
        } else {
            ""
        };
        format!("egress: DENY  udp {label} ({}){last}", why.text(&relay.limits, relay.bedrock))
    };
    (shared.log)(line);
}

fn done(shared: &Shared, flow: &Ended, why: &str) {
    (shared.log)(format!(
        "egress: done  udp {} ↑{} ↓{} datagrams, {} refused ({why})",
        flow.label, flow.up, flow.down, flow.refused
    ));
}

/// A relayed game connection that datagrams may follow, for as long as it is open in both
/// directions. Dropping it ends the flows that follow it; the TCP relay drops it at the
/// first hang-up by either side.
pub(super) struct Anchor<'a> {
    shared: &'a Shared,
    id: u64,
    _open: watch::Sender<()>,
}

/// Called by the TCP relay when it has connected a game to its server under `rule`:
/// datagrams to the UDP ports named for that server may now follow, to the address this
/// connection reached.
pub(super) fn anchor<'a>(
    shared: &'a Shared,
    rule: &ServerRule,
    label: &str,
    peer: Option<SocketAddr>,
) -> Option<Anchor<'a>> {
    let relay = shared.udp.as_ref()?;
    let (id, open) = relay.table().open_anchor(rule.host.clone(), rule.port, peer?, label);
    Some(Anchor { shared, id, _open: open })
}

impl Drop for Anchor<'_> {
    fn drop(&mut self) {
        let Some(relay) = self.shared.udp.as_ref() else { return };
        let ended = relay.table().close_anchor(self.id);
        for flow in &ended {
            done(self.shared, flow, "its game connection closed");
        }
    }
}

/// How long one name lookup for a Bedrock server rule may take.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
/// Pauses between lookups of a name that did not resolve; then the relay gives up on it.
const RESOLVE_RETRIES: [u64; 6] = [5, 15, 30, 60, 120, 240];

/// Bedrock: make each server rule the anchor of the flows to it — a literal address at once, a
/// name once it has resolved on this machine (a task per name; a lookup that fails is tried
/// again a few times, then given up, and each outcome is logged). The address found is the
/// rule's for the session: there is no second lookup for a later datagram that a network
/// attacker could answer differently. A public address is preferred, IPv4 before IPv6 (the
/// game speaks IPv4 inside; a Bedrock server's IPv6 port is usually another one). For Java
/// this does nothing: its flows follow game connections.
pub(super) fn stand(shared: &Arc<Shared>) -> Vec<JoinHandle<()>> {
    let Some(relay) = shared.udp.as_ref() else { return Vec::new() };
    let mut tasks = Vec::new();
    for rule in shared.policy.raknet() {
        match &rule.host {
            HostSpec::Ip(ip) => register(shared, relay, rule, SocketAddr::new((*ip).into(), rule.port)),
            HostSpec::Name(name) => {
                let (shared, rule, name) = (shared.clone(), rule.clone(), name.clone());
                tasks.push(tokio::spawn(async move {
                    let Some(relay) = shared.udp.as_ref() else { return };
                    let mut failed = 0;
                    loop {
                        match resolve(&name, rule.port).await {
                            Ok(addr) => {
                                (shared.log)(format!(
                                    "egress: udp   Bedrock server {name}:{} resolved to {addr}{}",
                                    rule.port,
                                    if failed > 0 {
                                        format!(" (after {failed} failed lookup(s))")
                                    } else {
                                        String::new()
                                    }
                                ));
                                register(&shared, relay, &rule, addr);
                                return;
                            }
                            Err(why) => {
                                let pause = RESOLVE_RETRIES.get(failed).copied();
                                failed += 1;
                                // The first failure and the last one are logged, the retries
                                // in between are not.
                                let outcome = match pause {
                                    Some(seconds) if failed == 1 => {
                                        Some(format!("trying again in {seconds} s, then a few more times"))
                                    }
                                    Some(_) => None,
                                    None => Some(format!("given up after {failed} attempts")),
                                };
                                if let Some(outcome) = outcome {
                                    (shared.log)(format!(
                                        "egress: udp   Bedrock server {name}:{}: the name did not resolve ({}); \
                                         datagrams to it are refused until it does — {outcome}",
                                        rule.port,
                                        crate::util::sanitize_line(why.as_bytes(), 200)
                                    ));
                                }
                                match pause {
                                    Some(seconds) => tokio::time::sleep(Duration::from_secs(seconds)).await,
                                    None => return,
                                }
                            }
                        }
                    }
                }));
            }
        }
    }
    tasks
}

async fn resolve(name: &str, port: u16) -> std::result::Result<SocketAddr, String> {
    let mut addrs: Vec<SocketAddr> = timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((name, port)))
        .await
        .map_err(|_| "the lookup timed out".to_string())?
        .map_err(|e| e.to_string())?
        .collect();
    addrs.sort_by_key(SocketAddr::is_ipv6); // stable: the resolver's order within each family
    addrs.iter().find(|a| is_public(a.ip())).or(addrs.first()).copied().ok_or_else(|| "no addresses".to_string())
}

/// Register the standing anchor of `rule` at `addr`, held until the relay stops.
fn register(shared: &Shared, relay: &Relay, rule: &ServerRule, addr: SocketAddr) {
    let label = format!("Bedrock server {}:{}", rule.host, rule.port);
    if !is_public(addr.ip()) && !rule.allow_private {
        // Registered all the same: the refusal of each datagram then says why.
        (shared.log)(format!(
            "egress: udp   {label} is at a non-public address ({}): LAN servers are not supported for Bedrock in this \
             version, datagrams to it are refused",
            addr.ip()
        ));
    }
    let (_, open) = relay.table().open_anchor(rule.host.clone(), rule.port, addr, &label);
    relay.standing.lock().expect("udp standing lock").push(open);
}

/// The relay is shutting down: the flows that are still open report what they carried.
pub(super) fn stop(shared: &Shared) {
    let Some(relay) = shared.udp.as_ref() else { return };
    let ended = relay.table().end_all();
    for flow in &ended {
        done(shared, flow, "the relay stopped");
    }
}

/// Serve one of the sockets inside the sandbox's namespace until the task is aborted.
pub(super) async fn serve(shared: Arc<Shared>, socket: GameSocket) {
    let Some(relay) = shared.udp.as_ref() else { return };
    let socket = Arc::new(socket);
    // Dropping the JoinSet (when this task is aborted) ends every flow's way back.
    let mut flows = JoinSet::new();
    // One byte more than may be relayed: a datagram that fills it is too large.
    let mut buf = vec![0u8; relay.limits.max_datagram + 1];
    loop {
        tokio::select! {
            arrival = socket.receive(&mut buf) => match arrival {
                Ok(arrival) => {
                    let data = &buf[..arrival.len.min(relay.limits.max_datagram)];
                    if let Some((key, opened, upstream)) = from_game(&shared, relay, &socket, &arrival, data) {
                        flows.spawn(pump(shared.clone(), socket.clone(), key, opened, upstream));
                    }
                }
                // Not expected on an unconnected socket; do not spin if it happens.
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            },
            Some(_) = flows.join_next(), if !flows.is_empty() => {}
        }
    }
}

fn from_game(
    shared: &Shared,
    relay: &Relay,
    socket: &GameSocket,
    arrival: &Arrival,
    data: &[u8],
) -> Option<(FlowKey, Opened, Arc<UdpSocket>)> {
    let Some(to) = arrival.to else {
        let label: Arc<str> = format!("port {}", socket.port).into();
        refuse(shared, relay, &label, Refusal::Unassigned);
        return None;
    };
    let key = FlowKey { game: arrival.from, dest: SocketAddrV4::new(to, socket.port) };
    let len = if arrival.truncated { relay.limits.max_datagram + 1 } else { arrival.len };
    // Bedrock: only the start of a RakNet exchange opens a flow (Java: any datagram does).
    let opens = !relay.bedrock || raknet::opens_flow(data);
    let verdict = if shared.policy.bedrock_pass() == Some(key.dest) {
        relay.table().pass_datagram(Instant::now(), &relay.limits, key, len, opens, || take_pass(shared), open_upstream)
    } else {
        relay.table().game_datagram(
            Instant::now(),
            &relay.limits,
            &shared.policy,
            key,
            len,
            opens,
            |ip| shared.dest_of(ip),
            open_upstream,
        )
    };
    match verdict {
        Verdict::Refuse { label, why } => {
            refuse(shared, relay, &label, why);
            None
        }
        Verdict::Relay { upstream, flow, label, opened } => {
            if let Some(opened) = &opened {
                shared.stats.udp_flows.fetch_add(1, Ordering::Relaxed);
                let line = if relay.bedrock {
                    let first = raknet::parse(data).map(|m| m.describe()).unwrap_or_default();
                    format!(
                        "egress: ALLOW udp {label} → {} (flow {} from {}; {first}; {})",
                        opened.target, opened.id, key.game, opened.tied_to
                    )
                } else {
                    format!(
                        "egress: ALLOW udp {label} → {} (flow {} from {}; follows the game connection {})",
                        opened.target, opened.id, key.game, opened.tied_to
                    )
                };
                (shared.log)(line);
            }
            // Never waits: see `GameSocket::send`.
            match send(upstream.as_raw_fd(), data, MsgFlags::MSG_DONTWAIT) {
                Ok(_) => {
                    shared.stats.udp_relayed.fetch_add(1, Ordering::Relaxed);
                }
                Err(errno) => {
                    relay.table().unsent(&key, flow, true);
                    refuse(shared, relay, &label, unsent_because(errno));
                }
            }
            opened.map(|opened| (key, opened, upstream))
        }
    }
}

/// Where the join pass leads, spending the approval (see `services::Services::take_pass`).
fn take_pass(shared: &Shared) -> std::result::Result<(SocketAddr, String), Refusal> {
    match shared.services.as_ref() {
        Some(services) => services.take_pass(),
        None => Err(Refusal::Pass("joins are not brokered in this session")),
    }
}

/// Why the kernel did not take a datagram for the server. `ECONNREFUSED` from a send on a
/// connected datagram socket is not about that datagram: it is the ICMP "port unreachable"
/// that an earlier one drew, reported on the next call (and that call sends nothing). So it
/// is the server's address saying that nothing listens there — to be logged as that, not as
/// a fault of this machine. Every other error is logged as local, which most are (a full
/// buffer, no route, a local firewall); what a firewall on the way answers ("prohibited")
/// arrives under an error number that a missing route also has, and is not told apart here.
fn unsent_because(errno: nix::errno::Errno) -> Refusal {
    if errno == nix::errno::Errno::ECONNREFUSED { Refusal::Unreachable } else { Refusal::Io }
}

/// The way back for one flow: what its socket receives goes to the game, until the flow has
/// been idle for too long or its game connection closes.
async fn pump(
    shared: Arc<Shared>,
    socket: Arc<GameSocket>,
    key: FlowKey,
    mut opened: Opened,
    upstream: Arc<UdpSocket>,
) {
    let Some(relay) = shared.udp.as_ref() else { return };
    let label = opened.label.clone();
    let mut buf = vec![0u8; relay.limits.max_datagram + 1];
    loop {
        // (The flow may have been ended by its game connection closing: then there is
        // nothing left to do here, and that end has been logged.)
        let Some(deadline) = relay.table().idle_deadline(&relay.limits, &key, opened.id) else { return };
        tokio::select! {
            received = upstream.recv(&mut buf) => match received {
                Ok(len) => {
                    // (A statement of its own: the table must not stay locked for what follows.)
                    let verdict = relay.table().server_datagram(Instant::now(), &relay.limits, &key, opened.id, len);
                    match verdict {
                        Ok(()) => match socket.send(&buf[..len], *key.dest.ip(), key.game) {
                            Ok(()) => {
                                shared.stats.udp_relayed.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(_) => {
                                relay.table().unsent(&key, opened.id, false);
                                refuse(&shared, relay, &label, Refusal::Io);
                            }
                        },
                        Err(why) => refuse(&shared, relay, &label, why),
                    }
                }
                // A connected UDP socket reports ICMP errors for what was sent through it
                // ("port unreachable": nothing listens there). That datagram is lost, the
                // flow is not; the pause keeps an error that will not go away from spinning.
                Err(_) => {
                    note(&shared, relay, &label, Refusal::Unreachable);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
            _ = tokio::time::sleep_until(deadline.into()) => {
                let idle = relay.table().idle_deadline(&relay.limits, &key, opened.id).is_some_and(|d| d <= Instant::now());
                if idle {
                    break;
                }
            }
            // The sender is dropped when the game connection ends; `Anchor::drop` has
            // removed the flow and logged it.
            _ = opened.closed.changed() => return,
        }
    }
    let ended = relay.table().end(&key, opened.id);
    if let Some(ended) = ended {
        done(&shared, &ended, &format!("the game sent nothing for {} s", relay.limits.idle.as_secs_f32()));
    }
}

// ---------------------------------------------------------------- generated-input harness

/// For `fuzzing::udp`: drive the decision table with a script of arbitrary events — game
/// connections opening and closing, datagrams from the game and from the server, time
/// passing — and check what must hold whatever the game, the server and the clock do.
/// Each event is four bytes. Returns how many of the game's datagrams were relayed and how
/// many refused, so that a test can tell a script that does something from one that does not.
#[doc(hidden)]
pub fn fuzz_table(data: &[u8]) -> (u64, u64) {
    use crate::config::{InstanceNetwork, NetworkConfig, ServerEntry};

    let entry = |host: &str, port: u16, allow_private: bool, udp: &[u16]| ServerEntry::Full {
        host: host.into(),
        port: Some(port),
        allow_private,
        voice: None,
        udp: udp.to_vec(),
    };
    // Two servers with UDP ports (one of them allowed on private addresses), one without.
    let net = NetworkConfig {
        servers: vec![
            entry("voice.example.org", 25565, false, &[24454]),
            entry("lan.example.org", 25570, true, &[24454, 24455]),
            entry("plain.example.org", 25565, false, &[]),
            entry("203.0.113.7", 25565, false, &[24454]),
        ],
        ..Default::default()
    };
    let policy = Policy::new(&net, &InstanceNetwork::default(), None).expect("harness policy");
    let limits = Limits {
        max_flows: 3,
        idle: Duration::from_secs(5),
        max_datagram: 100,
        to_server: Rate { per_second: 4, burst: 4 },
        to_game: Rate { per_second: 8, burst: 8 },
    };
    let mut names = FakeIps::new();
    let hosts = ["voice.example.org", "lan.example.org", "plain.example.org", "api.minecraftservices.com"];
    let fake: Vec<Ipv4Addr> = hosts.iter().map(|h| names.assign(h).expect("room")).collect();
    let dests = [
        fake[0],
        fake[1],
        fake[2],
        fake[3],
        Ipv4Addr::new(203, 0, 113, 7),
        Ipv4Addr::new(127, 99, 0, 9),
        Ipv4Addr::LOCALHOST,
    ];
    let peers: [SocketAddr; 4] = [
        "198.51.100.10:25565".parse().expect("literal"),
        "192.168.1.20:25570".parse().expect("literal"),
        "[2001:db8::5]:30000".parse().expect("literal"),
        "127.0.0.1:25565".parse().expect("literal"),
    ];
    let rule_hosts = [
        (HostSpec::Name("voice.example.org".into()), 25565),
        (HostSpec::Name("lan.example.org".into()), 25570),
        (HostSpec::Name("plain.example.org".into()), 25565),
        (HostSpec::Ip(Ipv4Addr::new(203, 0, 113, 7)), 25565),
    ];

    let mut table: Table<SocketAddr> = Table::default();
    let start = Instant::now();
    let mut now = start;
    let mut anchors: Vec<(u64, watch::Sender<()>)> = Vec::new();
    // Per flow id: relayed towards the server, and when the flow opened.
    let mut sent: HashMap<u64, (u64, Instant)> = HashMap::new();
    let (mut relayed, mut refused) = (0u64, 0u64);
    for step in data.as_chunks::<4>().0.iter().take(4096) {
        let (op, a, b, c) = (step[0] % 8, step[1], step[2], step[3]);
        match op {
            // The clock moves on (by up to 2.55 s).
            0 => now += Duration::from_millis(u64::from(a) * 10),
            // A game connection opens…
            1 => {
                let (host, tcp_port) = rule_hosts[usize::from(a) % rule_hosts.len()].clone();
                anchors.push(table.open_anchor(host, tcp_port, peers[usize::from(b) % peers.len()], "game"));
            }
            // …or closes, and takes its flows with it.
            2 if !anchors.is_empty() => {
                let (id, _open) = anchors.remove(usize::from(a) % anchors.len());
                table.close_anchor(id);
                assert!(table.flows.values().all(|f| f.anchor != id), "a flow outlived its game connection");
            }
            // A datagram from the server on an existing flow.
            3 => {
                let Some((key, id)) =
                    table.flows.iter().nth(usize::from(a) % table.flows.len().max(1)).map(|(k, f)| (*k, f.id))
                else {
                    continue;
                };
                let len = usize::from(b) * 2;
                let verdict = table.server_datagram(now, &limits, &key, id, len);
                assert!(verdict.is_err() || len <= limits.max_datagram, "an oversize datagram reached the game");
            }
            // A flow is checked for idleness, as its task would.
            4 => {
                let idle: Vec<(FlowKey, u64)> =
                    table.flows.iter().filter(|(_, f)| f.heard + limits.idle <= now).map(|(k, f)| (*k, f.id)).collect();
                for (key, id) in idle {
                    assert!(table.end(&key, id).is_some());
                }
            }
            // A datagram from the game.
            _ => {
                let key = FlowKey {
                    game: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000 + u16::from(a % 6)),
                    dest: SocketAddrV4::new(
                        dests[usize::from(b) % dests.len()],
                        [24454, 24455, 24456][usize::from(c) % 3],
                    ),
                };
                let len = usize::from(c) * 2;
                let verdict = table.game_datagram(
                    now,
                    &limits,
                    &policy,
                    key,
                    len,
                    true,
                    |ip| if FakeIps::contains(ip) { names.name_of(ip).map(Dest::Name) } else { Some(Dest::Ip(ip)) },
                    Ok,
                );
                let Verdict::Relay { upstream, flow, opened, .. } = verdict else {
                    refused += 1;
                    continue;
                };
                relayed += 1;
                // Whatever was relayed: small enough, to a destination a rule names, and to
                // the address of a game connection that is open right now under that rule.
                assert!(len <= limits.max_datagram, "an oversize datagram was relayed");
                let dest = if FakeIps::contains(*key.dest.ip()) {
                    Dest::Name(names.name_of(*key.dest.ip()).expect("relayed to an address nobody was given"))
                } else {
                    Dest::Ip(*key.dest.ip())
                };
                let rules = policy.udp_rules(&dest, key.dest.port()).expect("relayed without a rule naming the port");
                let (target, anchor) = table.flow_target(&key).expect("relayed without a flow");
                assert_eq!(upstream, target);
                let tied = table.anchors.get(&anchor).expect("relayed for a game connection that has closed");
                assert_eq!(
                    (target.ip(), target.port()),
                    (tied.peer.ip(), key.dest.port()),
                    "not the connection's address"
                );
                let rule = rules.iter().find(|r| r.host == tied.host && r.port == tied.tcp_port);
                let rule = rule.expect("the game connection was made under another rule");
                assert!(
                    is_public(target.ip())
                        || rules.iter().any(|r| r.allow_private && r.host == rule.host && r.port == rule.port),
                    "relayed to a private address without allow_private"
                );
                assert!(table.flow_count() <= limits.max_flows, "more flows than the limit");
                if opened.is_some() {
                    sent.insert(flow, (0, now));
                }
                // The rate limit, over the whole life of the flow.
                let (count, since) = sent.get_mut(&flow).expect("a flow that was never opened");
                *count += 1;
                let allowed = u64::from(limits.to_server.burst)
                    + (now - *since).as_millis() as u64 * u64::from(limits.to_server.per_second) / 1000
                    + 1;
                assert!(*count <= allowed, "{count} datagrams relayed where the rate limit allows {allowed}");
            }
        }
    }
    (relayed, refused)
}

/// For `fuzzing::raknet`: the decision table of a Bedrock session, driven by a script of
/// arbitrary events — server rules resolving, joins being approved, datagrams from the game
/// (opening a RakNet exchange or not) to servers, to names that are none, to the pass, datagrams
/// from servers, time passing — checking what must hold whatever the game, the services and the
/// clock do: a new flow opens only with a RakNet opening, only to the address the launcher
/// resolved for a rule that names the port (never a private one) or to the one server an
/// approval named, and everything stays within the size, rate and flow limits. Each event is
/// four bytes. Returns how many of the game's datagrams were relayed and how many refused.
#[doc(hidden)]
pub fn fuzz_bedrock_table(data: &[u8]) -> (u64, u64) {
    use crate::bedrock::{BedrockConfig, BedrockServer, net::plan};

    let server = |host: &str, port: u16| BedrockServer { host: host.into(), port };
    let cfg = BedrockConfig {
        servers: vec![
            server("play.example.org", 19132),
            server("203.0.113.9", 19133),
            server("lan.example.org", 19132),
        ],
        ..BedrockConfig::default()
    };
    let policy = Policy::bedrock(&plan(&cfg, &[]), true, false).expect("harness policy");
    let pass = policy.bedrock_pass().expect("joins are brokered");
    let limits = Limits {
        max_flows: 3,
        idle: Duration::from_secs(5),
        max_datagram: 200,
        to_server: Rate { per_second: 4, burst: 4 },
        to_game: Rate { per_second: 8, burst: 8 },
    };
    let mut names = FakeIps::new();
    let fake: Vec<Ipv4Addr> = ["play.example.org", "lan.example.org", "login.live.com"]
        .iter()
        .map(|h| names.assign(h).expect("room"))
        .collect();
    let dests = [fake[0], fake[1], fake[2], Ipv4Addr::new(203, 0, 113, 9), *pass.ip(), Ipv4Addr::new(127, 99, 0, 9)];
    // Where each rule resolves (the last one to a private address), and the approved servers.
    let resolved: [(HostSpec, u16, SocketAddr); 3] = [
        (HostSpec::Name("play.example.org".into()), 19132, "198.51.100.10:19132".parse().expect("literal")),
        (HostSpec::Ip(Ipv4Addr::new(203, 0, 113, 9)), 19133, "203.0.113.9:19133".parse().expect("literal")),
        (HostSpec::Name("lan.example.org".into()), 19132, "192.168.1.5:19132".parse().expect("literal")),
    ];
    let approved: [SocketAddr; 2] =
        ["198.51.100.77:30000".parse().expect("literal"), "203.0.113.200:19132".parse().expect("literal")];
    let opening = raknet::Offline::UnconnectedPing { open_connections: false, time: 1, client_guid: 2 }.encode();
    let connected = [0x84u8, 0, 0, 0, 0x40, 0, 8];

    let mut table: Table<SocketAddr> = Table::default();
    let mut standing: Vec<(usize, watch::Sender<()>)> = Vec::new();
    let mut armed: Option<SocketAddr> = None;
    let mut now = Instant::now();
    let mut sent: HashMap<u64, (u64, Instant)> = HashMap::new();
    let (mut relayed, mut refused) = (0u64, 0u64);
    for step in data.as_chunks::<4>().0.iter().take(4096) {
        let (op, a, b, c) = (step[0] % 7, step[1], step[2], step[3]);
        match op {
            0 => now += Duration::from_millis(u64::from(a) * 10),
            // A rule's name resolves (once).
            1 => {
                let rule = usize::from(a) % resolved.len();
                if !standing.iter().any(|(r, _)| *r == rule) {
                    let (host, port, peer) = resolved[rule].clone();
                    standing.push((rule, table.open_anchor(host, port, peer, "Bedrock server").1));
                }
            }
            // The user approves a join.
            2 => armed = Some(approved[usize::from(a) % approved.len()]),
            // A datagram from the server on an existing flow.
            3 => {
                let Some((key, id)) =
                    table.flows.iter().nth(usize::from(a) % table.flows.len().max(1)).map(|(k, f)| (*k, f.id))
                else {
                    continue;
                };
                let len = usize::from(b) * 2;
                let verdict = table.server_datagram(now, &limits, &key, id, len);
                assert!(verdict.is_err() || len <= limits.max_datagram, "an oversize datagram reached the game");
            }
            // A flow is checked for idleness, as its task would.
            4 => {
                let idle: Vec<(FlowKey, u64)> =
                    table.flows.iter().filter(|(_, f)| f.heard + limits.idle <= now).map(|(k, f)| (*k, f.id)).collect();
                for (key, id) in idle {
                    assert!(table.end(&key, id).is_some());
                }
            }
            // A datagram from the game.
            _ => {
                let dest_ip = dests[usize::from(b) % dests.len()];
                let port = [19132, 19133, 19134][usize::from(c) % 3];
                let key = FlowKey {
                    game: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000 + u16::from(a % 6)),
                    dest: SocketAddrV4::new(dest_ip, port),
                };
                let opens = c & 0x80 != 0;
                let len = if opens { opening.len() } else { connected.len() } + usize::from(c & 0x7f) * 2;
                let existed = table.flows.contains_key(&key);
                let verdict = if key.dest == pass {
                    table.pass_datagram(
                        now,
                        &limits,
                        key,
                        len,
                        opens,
                        || armed.take().map(|t| (t, "approved".to_string())).ok_or(Refusal::Pass("none armed")),
                        Ok,
                    )
                } else {
                    table.game_datagram(
                        now,
                        &limits,
                        &policy,
                        key,
                        len,
                        opens,
                        |ip| if FakeIps::contains(ip) { names.name_of(ip).map(Dest::Name) } else { Some(Dest::Ip(ip)) },
                        Ok,
                    )
                };
                let Verdict::Relay { upstream, flow, opened, .. } = verdict else {
                    refused += 1;
                    continue;
                };
                relayed += 1;
                assert!(len <= limits.max_datagram, "an oversize datagram was relayed");
                assert!(table.flow_count() <= limits.max_flows, "more flows than the limit");
                let (target, anchor) = table.flow_target(&key).expect("relayed without a flow");
                assert_eq!(upstream, target);
                if opened.is_some() {
                    assert!(!existed && opens, "a flow opened without a RakNet opening");
                    if key.dest == pass {
                        assert_eq!(anchor, PASS_ANCHOR);
                        assert!(approved.contains(&target), "the pass led somewhere no approval named");
                    } else {
                        let tied = table.anchors.get(&anchor).expect("a flow without its standing anchor");
                        assert!(is_public(tied.peer.ip()), "a flow to a private address");
                        assert_eq!(target, SocketAddr::new(tied.peer.ip(), key.dest.port()));
                        let dest = if FakeIps::contains(*key.dest.ip()) {
                            Dest::Name(names.name_of(*key.dest.ip()).expect("an address somebody was given"))
                        } else {
                            Dest::Ip(*key.dest.ip())
                        };
                        let rules = policy.udp_rules(&dest, key.dest.port()).expect("a rule naming the port");
                        assert!(rules.iter().any(|r| r.host == tied.host && r.port == tied.tcp_port));
                    }
                    sent.insert(flow, (0, now));
                }
                let (count, since) = sent.get_mut(&flow).expect("a flow that was never opened");
                *count += 1;
                let allowed = u64::from(limits.to_server.burst)
                    + (now - *since).as_millis() as u64 * u64::from(limits.to_server.per_second) / 1000
                    + 1;
                assert!(*count <= allowed, "{count} datagrams relayed where the rate limit allows {allowed}");
            }
        }
    }
    (relayed, refused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InstanceNetwork, NetworkConfig, ServerEntry};

    fn policy(servers: Vec<ServerEntry>) -> Policy {
        Policy::new(&NetworkConfig { servers, ..Default::default() }, &InstanceNetwork::default(), None).unwrap()
    }

    fn server(host: &str, allow_private: bool, udp: &[u16]) -> ServerEntry {
        ServerEntry::Full { host: host.into(), port: None, allow_private, voice: None, udp: udp.to_vec() }
    }

    fn name(n: &str) -> HostSpec {
        HostSpec::Name(n.into())
    }

    /// The table with the stand-in for a socket that the harness uses: the target address.
    struct Bench {
        policy: Policy,
        limits: Limits,
        names: FakeIps,
        table: Table<SocketAddr>,
        now: Instant,
    }

    impl Bench {
        fn new(servers: Vec<ServerEntry>) -> Self {
            Bench {
                policy: policy(servers),
                limits: Limits::default(),
                names: FakeIps::new(),
                table: Table::default(),
                now: Instant::now(),
            }
        }

        fn key(&mut self, source_port: u16, host: &str, port: u16) -> FlowKey {
            let ip = host.parse().unwrap_or_else(|_| self.names.assign(host).unwrap());
            FlowKey { game: SocketAddrV4::new(Ipv4Addr::LOCALHOST, source_port), dest: SocketAddrV4::new(ip, port) }
        }

        fn send(&mut self, key: FlowKey, len: usize) -> std::result::Result<SocketAddr, Refusal> {
            let names = &self.names;
            match self.table.game_datagram(
                self.now,
                &self.limits,
                &self.policy,
                key,
                len,
                true,
                |ip| if FakeIps::contains(ip) { names.name_of(ip).map(Dest::Name) } else { Some(Dest::Ip(ip)) },
                Ok,
            ) {
                Verdict::Relay { upstream, .. } => Ok(upstream),
                Verdict::Refuse { why, .. } => Err(why),
            }
        }
    }

    #[test]
    fn the_limits_of_every_launch_are_the_documented_ones() {
        // docs/notes/voice-chat.md quotes these numbers.
        let limits = Limits::default();
        assert_eq!(limits.max_flows, 8);
        assert_eq!(limits.idle, Duration::from_secs(30));
        assert_eq!(limits.max_datagram, 1400);
        assert_eq!(limits.to_server, Rate { per_second: 100, burst: 100 });
        assert_eq!(limits.to_game, Rate { per_second: 500, burst: 500 });
        assert_eq!(MAX_PORTS, 16);
    }

    #[test]
    fn a_bucket_gives_its_burst_then_its_rate_and_never_more() {
        let rate = Rate { per_second: 10, burst: 5 };
        let start = Instant::now();
        let mut bucket = Bucket::full(rate, start);
        assert_eq!((0..20).filter(|_| bucket.take(rate, start)).count(), 5, "the burst, at once");
        assert!(!bucket.take(rate, start + Duration::from_millis(99)), "a tenth of a second is one token");
        assert!(bucket.take(rate, start + Duration::from_millis(100)));
        assert!(!bucket.take(rate, start + Duration::from_millis(100)));
        // Fractions are carried, not lost: 30 steps of 33.4 ms are one second, ten tokens.
        let mut bucket = Bucket::full(rate, start);
        while bucket.take(rate, start) {}
        let granted = (1..=30).filter(|i| bucket.take(rate, start + Duration::from_micros(33_400 * i))).count();
        assert_eq!(granted, 10);
        // A long silence is worth one burst, and a clock that steps back is worth nothing.
        let later = start + Duration::from_secs(3600);
        assert_eq!((0..20).filter(|_| bucket.take(rate, later)).count(), 5);
        assert!(!bucket.take(rate, start));
        // A rate of nothing is nothing, not a division by zero.
        let none = Rate { per_second: 0, burst: 0 };
        assert!(!Bucket::full(none, start).take(none, later));
    }

    #[test]
    fn a_datagram_goes_to_the_address_of_the_open_game_connection_and_nowhere_else() {
        let mut b = Bench::new(vec![server("play.example.org", false, &[24454])]);
        let key = b.key(40000, "play.example.org", 24454);
        assert_eq!(b.send(key, 100), Err(Refusal::NoGameConnection), "voice never precedes the game connection");
        assert_eq!(b.table.flow_count(), 0);

        // The game connection went to the SRV target on another port; voice follows its address.
        let peer: SocketAddr = "198.51.100.10:30123".parse().unwrap();
        let (id, open) = b.table.open_anchor(name("play.example.org"), 25565, peer, "play.example.org:25565");
        assert_eq!(b.send(key, 100), Ok("198.51.100.10:24454".parse().unwrap()));
        assert_eq!(b.send(key, 100), Ok("198.51.100.10:24454".parse().unwrap()));
        assert_eq!(b.table.flow_count(), 1, "one source and destination, one flow");
        assert_eq!(b.table.flow_target(&key), Some(("198.51.100.10:24454".parse().unwrap(), id)));

        // A second connection to another address (round-robin DNS) does not move the flow…
        let (newer, _open2) =
            b.table.open_anchor(name("play.example.org"), 25565, "198.51.100.99:25565".parse().unwrap(), "x");
        assert_eq!(b.send(key, 100), Ok("198.51.100.10:24454".parse().unwrap()));
        // …but a new flow follows the newest connection.
        let other = b.key(40001, "play.example.org", 24454);
        assert_eq!(b.send(other, 100), Ok("198.51.100.99:24454".parse().unwrap()));
        assert_eq!(b.table.flow_target(&other).unwrap().1, newer);

        // The first connection closes: its flow ends with it, the other one stays.
        drop(open);
        let ended = b.table.close_anchor(id);
        assert_eq!(ended, vec![Ended { label: "play.example.org:24454".into(), up: 3, down: 0, refused: 0 }]);
        assert_eq!(b.table.flow_count(), 1);
        // The same socket of the game may go on: a new flow, to the connection that is open.
        assert_eq!(b.send(key, 100), Ok("198.51.100.99:24454".parse().unwrap()));
    }

    #[test]
    fn every_kind_of_destination_that_no_rule_covers_is_refused() {
        let mut b = Bench::new(vec![
            server("voice.example.org", false, &[24454]),
            server("plain.example.org", false, &[]),
            server("203.0.113.7", false, &[24455]),
        ]);
        let peer: SocketAddr = "198.51.100.10:25565".parse().unwrap();
        // Game connections to all three, so that only the rules decide.
        let _a = b.table.open_anchor(name("voice.example.org"), 25565, peer, "a");
        let _b = b.table.open_anchor(name("plain.example.org"), 25565, peer, "b");
        let _c = b.table.open_anchor(HostSpec::Ip(Ipv4Addr::new(203, 0, 113, 7)), 25565, peer, "c");

        let cases = [
            (("voice.example.org", 24454), Ok(())),
            (("voice.example.org", 24455), Err(Refusal::PortNotListed)), // the other server's port
            (("plain.example.org", 24454), Err(Refusal::NoUdpRule)),
            (("api.minecraftservices.com", 24454), Err(Refusal::NotAServer)), // resolvable, not a game server
            (("203.0.113.7", 24455), Ok(())),
            (("203.0.113.7", 24454), Err(Refusal::PortNotListed)),
            (("203.0.113.8", 24455), Err(Refusal::NotAServer)), // the literal address next door
            (("127.0.0.1", 24454), Err(Refusal::NotAServer)),
            (("127.99.0.1", 24454), Err(Refusal::Unassigned)), // synthetic, never handed out
        ];
        for (source, ((host, port), expected)) in cases.into_iter().enumerate() {
            let key = b.key(41000 + source as u16, host, port);
            assert_eq!(b.send(key, 64).map(|_| ()), expected, "{host}:{port}");
        }
        assert_eq!(b.table.flow_count(), 2, "refused datagrams leave nothing behind");
    }

    #[test]
    fn the_game_connection_must_be_one_made_under_a_rule_that_names_the_port() {
        // Two servers on one name: only the rule for the second names a UDP port.
        let mut b = Bench::new(vec![
            ServerEntry::Short("example.org".into()),
            ServerEntry::Full {
                host: "example.org".into(),
                port: Some(25570),
                allow_private: false,
                voice: Some(24454),
                udp: vec![],
            },
        ]);
        let key = b.key(40000, "example.org", 24454);
        let _first = b.table.open_anchor(name("example.org"), 25565, "198.51.100.10:25565".parse().unwrap(), "first");
        assert_eq!(b.send(key, 64), Err(Refusal::NoGameConnection), "a connection to the other server opens nothing");
        let _second = b.table.open_anchor(name("example.org"), 25570, "198.51.100.20:25570".parse().unwrap(), "second");
        assert_eq!(b.send(key, 64), Ok("198.51.100.20:24454".parse().unwrap()));
    }

    #[test]
    fn private_addresses_need_the_flag_on_the_rule_that_names_the_port() {
        for private in ["192.168.1.20:25565", "127.0.0.1:25565", "10.0.0.5:25565", "[fd00::5]:25565", "[::1]:25565"] {
            let peer: SocketAddr = private.parse().unwrap();
            // The TCP relay let the connection through under a rule with the flag; the rule
            // that names the UDP port has none.
            let mut b =
                Bench::new(vec![server("lan.example.org", true, &[]), server("lan.example.org", false, &[24454])]);
            let key = b.key(40000, "lan.example.org", 24454);
            let _open = b.table.open_anchor(name("lan.example.org"), 25565, peer, "lan");
            assert_eq!(b.send(key, 64), Err(Refusal::PrivateAddress), "{private}");

            let mut b = Bench::new(vec![server("lan.example.org", true, &[24454])]);
            let key = b.key(40000, "lan.example.org", 24454);
            let _open = b.table.open_anchor(name("lan.example.org"), 25565, peer, "lan");
            let mut target = peer;
            target.set_port(24454);
            assert_eq!(b.send(key, 64), Ok(target), "{private}");
        }
    }

    #[test]
    fn size_rate_and_flow_limits_hold_in_both_directions() {
        let mut b = Bench::new(vec![server("play.example.org", false, &[24454])]);
        b.limits = Limits {
            max_flows: 2,
            idle: Duration::from_secs(30),
            max_datagram: 1400,
            to_server: Rate { per_second: 10, burst: 20 },
            to_game: Rate { per_second: 10, burst: 30 },
        };
        let _open =
            b.table.open_anchor(name("play.example.org"), 25565, "198.51.100.10:25565".parse().unwrap(), "game");
        let key = b.key(40000, "play.example.org", 24454);

        // Size: an oversize datagram opens no flow, and is refused on an open one.
        assert_eq!(b.send(key, 1401), Err(Refusal::Oversize));
        assert_eq!(b.table.flow_count(), 0);
        assert!(b.send(key, 1400).is_ok());
        assert_eq!(b.send(key, 1401), Err(Refusal::Oversize));
        let id = b.table.flows[&key].id;
        assert_eq!(b.table.server_datagram(b.now, &b.limits, &key, id, 1401), Err(Refusal::Oversize));
        assert_eq!(b.table.server_datagram(b.now, &b.limits, &key, id, 1400), Ok(()));

        // Rate: the burst (one token went on the first datagram), then nothing until time passes.
        let passed = (0..100).filter(|_| b.send(key, 100).is_ok()).count();
        assert_eq!(passed, 19);
        assert_eq!(b.send(key, 100), Err(Refusal::RateToServer));
        let passed = (0..100).filter(|_| b.table.server_datagram(b.now, &b.limits, &key, id, 100).is_ok()).count();
        assert_eq!(passed, 29);
        assert_eq!(b.table.server_datagram(b.now, &b.limits, &key, id, 100), Err(Refusal::RateToGame));
        b.now += Duration::from_secs(1);
        assert_eq!((0..100).filter(|_| b.send(key, 100).is_ok()).count(), 10, "one second, ten datagrams");
        assert_eq!((0..100).filter(|_| b.table.server_datagram(b.now, &b.limits, &key, id, 100).is_ok()).count(), 10);

        // Flows: a second one, not a third. Each flow has a bucket of its own.
        let second = b.key(40001, "play.example.org", 24454);
        assert!(b.send(second, 100).is_ok());
        let third = b.key(40002, "play.example.org", 24454);
        assert_eq!(b.send(third, 100), Err(Refusal::FlowLimit));
        assert_eq!(b.table.flow_count(), 2);

        // What the flow says when it ends: every datagram is in exactly one of the counts.
        let ended = b.table.end(&key, id).unwrap();
        assert_eq!(
            (ended.up, ended.down, ended.refused),
            (1 + 19 + 10, 1 + 29 + 10, 1 + 1 + 81 + 1 + 71 + 1 + 90 + 90)
        );
        // Its place is free again.
        assert!(b.send(third, 100).is_ok());
    }

    #[test]
    fn only_the_game_keeps_a_flow_alive_and_an_old_task_cannot_end_a_new_flow() {
        let mut b = Bench::new(vec![server("play.example.org", false, &[24454])]);
        let _open =
            b.table.open_anchor(name("play.example.org"), 25565, "198.51.100.10:25565".parse().unwrap(), "game");
        let key = b.key(40000, "play.example.org", 24454);
        assert!(b.send(key, 100).is_ok());
        let id = b.table.flows[&key].id;
        let opened_at = b.now;
        assert_eq!(b.table.idle_deadline(&b.limits, &key, id), Some(opened_at + Duration::from_secs(30)));

        // The server talking does not move the deadline…
        b.now += Duration::from_secs(20);
        assert!(b.table.server_datagram(b.now, &b.limits, &key, id, 100).is_ok());
        assert_eq!(b.table.idle_deadline(&b.limits, &key, id), Some(opened_at + Duration::from_secs(30)));
        // …the game talking does.
        assert!(b.send(key, 100).is_ok());
        assert_eq!(b.table.idle_deadline(&b.limits, &key, id), Some(b.now + Duration::from_secs(30)));

        // The flow ends and the same socket of the game starts another: what belonged to
        // the old one (its task, a late datagram from the server) does not touch the new one.
        assert!(b.table.end(&key, id).is_some());
        assert!(b.send(key, 100).is_ok());
        let new = b.table.flows[&key].id;
        assert_ne!(new, id);
        assert_eq!(b.table.idle_deadline(&b.limits, &key, id), None);
        assert_eq!(b.table.end(&key, id), None);
        assert_eq!(b.table.server_datagram(b.now, &b.limits, &key, id, 100), Err(Refusal::Stale));
        assert_eq!(b.table.flow_count(), 1);
    }

    #[test]
    fn a_refused_send_is_the_servers_answer_and_any_other_failure_is_this_machines() {
        use nix::errno::Errno;
        assert_eq!(unsent_because(Errno::ECONNREFUSED), Refusal::Unreachable);
        for local in [
            Errno::EAGAIN,
            Errno::ENOBUFS,
            Errno::EPERM,
            Errno::ENETUNREACH,
            Errno::EHOSTUNREACH,
            Errno::EMSGSIZE,
            Errno::EBADF,
        ] {
            assert_eq!(unsent_because(local), Refusal::Io, "{local}");
        }
        // The two are worded so that the log tells them apart.
        let limits = Limits::default();
        assert!(Refusal::Unreachable.text(&limits, false).contains("nothing listens there"));
        assert!(Refusal::Io.text(&limits, false).contains("operating system"));
    }

    #[test]
    fn the_harness_accepts_a_script_that_exercises_every_step() {
        // A game connection opens; a datagram; time passes; the server answers; a datagram
        // to a server without a connection; an idle check; the connection closes; a
        // datagram after it.
        let script = [
            [1, 0, 0, 0],
            [5, 0, 0, 0],
            [0, 50, 0, 0],
            [3, 0, 10, 0],
            [5, 1, 1, 1],
            [4, 0, 0, 0],
            [2, 0, 0, 0],
            [5, 0, 0, 0],
        ];
        assert_eq!(fuzz_table(script.as_flattened()), (1, 2));
        assert_eq!(fuzz_table(&[]), (0, 0));
        assert_eq!(fuzz_table(&[5, 0, 0]), (0, 0), "an incomplete event is no event");
    }

    // ------------------------------------------------------------ Bedrock

    /// A Bedrock session's table, with the plan's servers and the join pass.
    struct BedrockBench {
        policy: Policy,
        limits: Limits,
        names: FakeIps,
        table: Table<SocketAddr>,
        now: Instant,
    }

    impl BedrockBench {
        fn new(servers: &[(&str, u16)]) -> Self {
            let cfg = crate::bedrock::BedrockConfig {
                servers: servers
                    .iter()
                    .map(|(host, port)| crate::bedrock::BedrockServer { host: host.to_string(), port: *port })
                    .collect(),
                ..Default::default()
            };
            BedrockBench {
                policy: Policy::bedrock(&crate::bedrock::net::plan(&cfg, &[]), true, false).unwrap(),
                limits: Limits::raknet(),
                names: FakeIps::new(),
                table: Table::default(),
                now: Instant::now(),
            }
        }

        fn key(&mut self, source_port: u16, host: &str, port: u16) -> FlowKey {
            let ip = host.parse().unwrap_or_else(|_| self.names.assign(host).unwrap());
            FlowKey { game: SocketAddrV4::new(Ipv4Addr::LOCALHOST, source_port), dest: SocketAddrV4::new(ip, port) }
        }

        fn send(&mut self, key: FlowKey, len: usize, opens: bool) -> std::result::Result<SocketAddr, Refusal> {
            let names = &self.names;
            match self.table.game_datagram(
                self.now,
                &self.limits,
                &self.policy,
                key,
                len,
                opens,
                |ip| if FakeIps::contains(ip) { names.name_of(ip).map(Dest::Name) } else { Some(Dest::Ip(ip)) },
                Ok,
            ) {
                Verdict::Relay { upstream, .. } => Ok(upstream),
                Verdict::Refuse { why, .. } => Err(why),
            }
        }
    }

    #[test]
    fn a_bedrock_flow_goes_to_the_address_resolved_for_its_rule_and_opens_only_with_raknet() {
        let mut b = BedrockBench::new(&[("play.example.org", 19132), ("203.0.113.9", 19133)]);
        let key = b.key(40000, "play.example.org", 19132);
        assert_eq!(b.send(key, 33, true), Err(Refusal::NoGameConnection), "before the name has resolved");
        let resolved: SocketAddr = "198.51.100.10:19132".parse().unwrap();
        let _standing = b.table.open_anchor(name("play.example.org"), 19132, resolved, "Bedrock server");
        assert_eq!(b.send(key, 7, false), Err(Refusal::NotRakNet), "a connected datagram opens nothing");
        assert_eq!(b.table.flow_count(), 0);
        assert_eq!(b.send(key, 33, true), Ok(resolved));
        assert_eq!(b.send(key, 1200, false), Ok(resolved), "once open, a flow carries what the game sends");
        // A literal address is its own anchor; the address next door, another port, a name that
        // is no server, an address never handed out: refused, each for its reason.
        let literal: SocketAddr = "203.0.113.9:19133".parse().unwrap();
        let _literal =
            b.table.open_anchor(HostSpec::Ip(Ipv4Addr::new(203, 0, 113, 9)), 19133, literal, "Bedrock server");
        let k = b.key(40001, "203.0.113.9", 19133);
        assert_eq!(b.send(k, 33, true), Ok(literal));
        for (source, host, port, why) in [
            (40002, "play.example.org", 19133, Refusal::PortNotListed),
            (40003, "203.0.113.10", 19133, Refusal::NotAServer),
            (40004, "login.live.com", 19132, Refusal::NotAServer),
            (40005, "127.99.0.1", 19132, Refusal::Unassigned),
        ] {
            let k = b.key(source, host, port);
            assert_eq!(b.send(k, 33, true), Err(why), "{host}:{port}");
        }
        // The largest datagram of an MTU discovery and of a server that negotiated MTU 1500 pass;
        // one byte more does not.
        let k = b.key(40006, "play.example.org", 19132);
        assert_eq!(b.send(k, raknet::MAX_DATAGRAM + 1, true), Err(Refusal::Oversize));
        assert_eq!(b.send(k, raknet::MAX_PROBE, true), Ok(resolved));
        assert_eq!(b.send(k, raknet::MAX_DATAGRAM, false), Ok(resolved));
    }

    #[test]
    fn a_bedrock_server_at_a_private_address_is_refused_as_a_lan_server() {
        let mut b = BedrockBench::new(&[("lan.example.org", 19132)]);
        let _standing = b.table.open_anchor(name("lan.example.org"), 19132, "192.168.1.5:19132".parse().unwrap(), "x");
        let key = b.key(40000, "lan.example.org", 19132);
        assert_eq!(b.send(key, 33, true), Err(Refusal::PrivateAddress));
        assert!(Refusal::PrivateAddress.text(&b.limits, true).contains("LAN servers are not supported"));
    }

    #[test]
    fn the_pass_opens_a_flow_to_the_approved_server_and_spends_nothing_on_a_refused_datagram() {
        let mut b = BedrockBench::new(&[]);
        let pass = b.policy.bedrock_pass().unwrap();
        let key = FlowKey { game: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000), dest: pass };
        let target: SocketAddr = "198.51.100.77:30000".parse().unwrap();
        let taken = std::cell::Cell::new(0);
        let take = || {
            taken.set(taken.get() + 1);
            if taken.get() == 1 { Ok((target, "the approved join".to_string())) } else { Err(Refusal::Pass("used")) }
        };
        let pass_send = |table: &mut Table<SocketAddr>, key, len, opens| match table
            .pass_datagram(b.now, &b.limits, key, len, opens, take, Ok)
        {
            Verdict::Relay { upstream, opened, .. } => Ok((upstream, opened.map(|o| o.closed))),
            Verdict::Refuse { why, .. } => Err(why),
        };
        assert_eq!(pass_send(&mut b.table, key, 7, false).err(), Some(Refusal::NotRakNet));
        assert_eq!(pass_send(&mut b.table, key, raknet::MAX_DATAGRAM + 1, true).err(), Some(Refusal::Oversize));
        assert_eq!(taken.get(), 0, "nothing was spent on refused datagrams");
        let (upstream, closed) = pass_send(&mut b.table, key, 33, true).unwrap();
        assert_eq!(upstream, target);
        let closed = closed.expect("a new flow");
        assert_eq!(taken.get(), 1);
        assert_eq!(b.table.flow_target(&key), Some((target, PASS_ANCHOR)));
        // The same flow goes on without asking again; another flow asks, and is refused.
        assert_eq!(pass_send(&mut b.table, key, 900, false).map(|(u, _)| u), Ok(target));
        assert_eq!(taken.get(), 1);
        let other = FlowKey { game: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40001), dest: pass };
        assert_eq!(pass_send(&mut b.table, other, 33, true).err(), Some(Refusal::Pass("used")));
        // When the flow ends (the game fell silent), its task hears of it.
        let id = b.table.flows[&key].id;
        assert!(b.table.end(&key, id).is_some());
        assert!(closed.has_changed().is_err(), "the flow's watch is closed with it");
        assert!(Refusal::Pass("used").text(&b.limits, true).contains("join pass"));
    }

    #[test]
    fn the_raknet_limits_are_the_documented_ones() {
        // docs/notes/bedrock-net.md quotes these numbers.
        let limits = Limits::raknet();
        assert_eq!(limits.max_flows, 32);
        assert_eq!(limits.idle, Duration::from_secs(30));
        assert_eq!(limits.max_datagram, 1472);
        assert_eq!(limits.to_server, Rate { per_second: 500, burst: 500 });
        assert_eq!(limits.to_game, Rate { per_second: 4000, burst: 4000 });
        // Voice keeps its own.
        assert_eq!(Limits::default().max_datagram, 1400);
    }

    #[test]
    fn bedrock_refusals_say_what_to_add_and_java_refusals_are_unchanged() {
        let limits = Limits::raknet();
        assert!(Refusal::NotAServer.text(&limits, true).contains("[bedrock] servers"));
        assert!(Refusal::PortNotListed.text(&limits, true).contains("[bedrock] servers"));
        assert!(Refusal::NoGameConnection.text(&limits, true).contains("resolved"));
        assert!(Refusal::NotRakNet.text(&limits, true).contains("RakNet"));
        assert_eq!(Refusal::NotAServer.text(&limits, false), "not an allowlisted game server");
        assert!(Refusal::NoGameConnection.text(&limits, false).contains("game connection"));
        assert!(Refusal::PrivateAddress.text(&limits, false).contains("allow_private"));
    }

    #[test]
    fn the_bedrock_harness_accepts_a_script_that_exercises_every_step() {
        // A rule resolves; a RakNet opening; a connected datagram on the flow; a join is approved;
        // a flow to the pass; a second one; a name that is no server; a rule not yet resolved; it
        // resolves to a private address; the server answers; time passes; an idle check.
        let script = [
            [1, 0, 0, 0],
            [5, 0, 0, 0x81],
            [5, 0, 0, 0x03],
            [2, 0, 0, 0],
            [5, 1, 4, 0x81],
            [5, 2, 4, 0x81],
            [5, 3, 2, 0x81],
            [5, 4, 1, 0x81],
            [1, 2, 0, 0],
            [5, 4, 1, 0x81],
            [3, 0, 10, 0],
            [0, 255, 0, 0],
            [0, 255, 0, 0],
            [4, 0, 0, 0],
        ];
        assert_eq!(fuzz_bedrock_table(script.as_flattened()), (3, 4));
        assert_eq!(fuzz_bedrock_table(&[]), (0, 0));
    }
}
